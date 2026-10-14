//! The transfer methods of the remote protocol: a file read in pieces, and a file written in
//! pieces beside where it goes, which appears under its real name only once every byte has
//! arrived. Every method waits for the file system.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, ErrorKind};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{FileExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::files::{checked, failure, mtime, regular_file};
use crate::protocol::{
    ErrorCode, FsDownload, FsDownloadParams, FsOnConflict, FsTransferInfo, FsUploadBegin,
    FsUploadBeginParams, FsUploadChunk, FsUploadChunkParams, FsUploadDestination, FsUploadEnd,
    FsUploadIdParams, RpcError,
};
use crate::upload_names::{self, UploadsAreaName};
use crate::uploads_area;

/// The longest file that is sent in either direction.
pub const MAX_BYTES: u64 = 200 * 1024 * 1024;
/// The most bytes in one piece, before base64.
pub const CHUNK_BYTES: u64 = 1024 * 1024;
/// How long an unfinished upload is kept after the last request that named it.
pub const UNFINISHED_KEPT_FOR: Duration = Duration::from_secs(15 * 60);
/// How long the result of a finished upload is kept for a client that asks for it again.
pub const RESULT_KEPT_FOR: Duration = Duration::from_secs(10 * 60);

const TEMPORARY_PREFIX: &str = ".hako-upload-";
const UPLOADS_AREA: &str = "uploads";
/// Lists the temporary files of the unfinished uploads, which outlives the app when it does not
/// get to remove them itself.
const UNFINISHED_RECORD: &str = "unfinished-uploads.json";
const MAX_ID_CHARS: usize = 64;
/// How many numbered names are tried for a file before it is given up.
const NAME_ATTEMPTS: u32 = 10_000;
/// How many ids are tried for a temporary file whose name is somehow taken.
const ID_ATTEMPTS: usize = 16;

pub fn info() -> FsTransferInfo {
    FsTransferInfo {
        max_bytes: MAX_BYTES,
        chunk_bytes: CHUNK_BYTES,
    }
}

/// Reads one piece of a file, from nothing kept between two calls.
pub fn download(params: &FsDownloadParams) -> Result<FsDownload, RpcError> {
    let path = params.path.as_str();
    let file_path = checked(path)?;
    let expected = match (params.expected_size, params.expected_mtime) {
        (Some(size), Some(mtime)) => Some((size, mtime)),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            return Err(RpcError::bad_request(
                "expected_size and expected_mtime must both be numbers or both be null",
            ));
        }
    };
    regular_file(file_path, path)?;
    // Not to wait forever on a pipe that took the file's place a moment ago.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(file_path)
        .map_err(|error| failure(&error, path))?;
    let before = file.metadata().map_err(|error| failure(&error, path))?;
    if !before.is_file() {
        return Err(RpcError::failed(format!("Not a regular file: {path}")));
    }
    let size = before.len();
    if size > MAX_BYTES {
        return Err(too_large(size));
    }
    let state = (size, mtime(&before));
    if expected.is_some_and(|expected| expected != state) {
        return Err(changed(path));
    }
    if !(1..=CHUNK_BYTES).contains(&params.length) {
        return Err(RpcError::bad_request(format!(
            "length must be from 1 to {CHUNK_BYTES}"
        )));
    }
    if params.offset > size {
        return Err(RpcError::bad_request(format!(
            "offset {} is beyond the end of the file, at {size}",
            params.offset
        )));
    }

    let mut bytes = vec![0; params.length.min(size - params.offset) as usize];
    match file.read_exact_at(&mut bytes, params.offset) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Err(changed(path)),
        Err(error) => return Err(failure(&error, path)),
    }
    let after = file.metadata().map_err(|error| failure(&error, path))?;
    if (after.len(), mtime(&after)) != state {
        return Err(changed(path));
    }
    Ok(FsDownload {
        data: BASE64.encode(bytes),
        size,
        mtime: state.1,
    })
}

fn changed(path: &str) -> RpcError {
    RpcError::new(
        ErrorCode::Conflict,
        format!("Changed on the Mac while it was being downloaded: {path}"),
    )
}

fn too_large(size: u64) -> RpcError {
    RpcError::new(
        ErrorCode::TooLarge,
        format!("The file is {size} bytes, over the limit of {MAX_BYTES}"),
    )
}

/// The uploads of the app, which belong to it rather than to a connection.
pub struct Transfers {
    /// Holds the uploads area and the record of unfinished uploads.
    data_dir: PathBuf,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    open: HashMap<String, Upload>,
    finished: HashMap<String, Finished>,
    /// Counts the ids handed out, which makes each one new.
    ids: u64,
}

struct Upload {
    /// The temporary file, open for writing.
    file: File,
    temporary: PathBuf,
    place: Place,
    size: u64,
    received: u64,
    last_named: Instant,
}

/// Where a finished upload is put, and under what name.
enum Place {
    Directory {
        /// As the client sent it.
        path: String,
        name: String,
        on_conflict: FsOnConflict,
    },
    UploadsArea {
        directory: PathBuf,
        name: UploadsAreaName,
    },
}

struct Finished {
    result: FsUploadEnd,
    at: Instant,
}

impl Transfers {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            state: Mutex::default(),
        }
    }

    /// Removes what uploads that were unfinished when the app last quit left behind, and the
    /// files of the uploads area that have been there for too long.
    pub fn start(&self, now: SystemTime, today: &str) {
        let state = self.lock();
        self.remove_leftovers(&state);
        uploads_area::clean(&self.uploads_area(), now, today);
    }

    /// Deletes the files of the uploads area that have been there for too long.
    pub fn clean_uploads_area(&self, now: SystemTime, today: &str) {
        // Holding the state keeps the clean-up from removing a day's directory under an upload
        // that is being begun in it.
        let _state = self.lock();
        uploads_area::clean(&self.uploads_area(), now, today);
    }

    /// Gives up on the uploads that no request has named for too long, and forgets the results
    /// that have been kept for long enough.
    pub fn expire(&self, now: Instant) {
        let mut state = self.lock();
        self.drop_expired(&mut state, now);
    }

    /// Removes the temporary file of every unfinished upload.
    pub fn abandon_all(&self) {
        let mut state = self.lock();
        for upload in state.open.values() {
            let _ = fs::remove_file(&upload.temporary);
        }
        state.open.clear();
        self.save_record(&state);
    }

    pub fn begin(
        &self,
        params: FsUploadBeginParams,
        now: Instant,
        today: &str,
    ) -> Result<FsUploadBegin, RpcError> {
        if !upload_names::is_valid(&params.name) {
            return Err(RpcError::bad_request(
                "name must be one path segment of at most 255 bytes, without / or NUL",
            ));
        }
        if params.size > MAX_BYTES {
            return Err(too_large(params.size));
        }
        let mut state = self.lock();
        self.drop_expired(&mut state, now);

        let (directory, shown, place) = match params.destination {
            FsUploadDestination::Dir { path } => {
                let directory = checked(&path)?.to_path_buf();
                let on_conflict = params.on_conflict.unwrap_or_default();
                check_directory(&directory, &path, &params.name, on_conflict)?;
                let place = Place::Directory {
                    path: path.clone(),
                    name: params.name,
                    on_conflict,
                };
                (directory, path, place)
            }
            FsUploadDestination::Uploads => {
                let area = self.uploads_area();
                let shown = area.to_string_lossy().into_owned();
                let directory = uploads_area::day_directory(&area, today)
                    .map_err(|error| failure(&error, &shown))?;
                let place = Place::UploadsArea {
                    directory: directory.clone(),
                    name: upload_names::for_uploads_area(&params.name),
                };
                (directory, shown, place)
            }
        };
        let (id, file, temporary) =
            create_temporary(&mut state, &directory).map_err(|error| failure(&error, &shown))?;
        state.open.insert(
            id.clone(),
            Upload {
                file,
                temporary,
                place,
                size: params.size,
                received: 0,
                last_named: now,
            },
        );
        self.save_record(&state);
        Ok(FsUploadBegin { upload_id: id })
    }

    pub fn chunk(
        &self,
        params: FsUploadChunkParams,
        now: Instant,
    ) -> Result<FsUploadChunk, RpcError> {
        check_id(&params.upload_id)?;
        let mut state = self.lock();
        self.drop_expired(&mut state, now);
        let upload = state
            .open
            .get_mut(&params.upload_id)
            .ok_or_else(no_upload)?;
        upload.last_named = now;
        let bytes = decode_piece(&params.data)?;
        let end = params
            .offset
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| RpcError::bad_request("offset is out of range"))?;
        if end <= upload.received {
            // A piece whose answer was lost on its way.
            return Ok(FsUploadChunk {
                received: upload.received,
            });
        }
        if params.offset != upload.received {
            return Err(RpcError::bad_request(format!(
                "The piece at {} does not follow the {} bytes that have arrived",
                params.offset, upload.received
            )));
        }
        if end > upload.size {
            return Err(RpcError::bad_request(format!(
                "The piece goes past the {} bytes that were announced",
                upload.size
            )));
        }
        match upload.file.write_all_at(&bytes, params.offset) {
            Ok(()) => {
                upload.received = end;
                Ok(FsUploadChunk { received: end })
            }
            Err(error) => {
                let shown = upload.temporary.to_string_lossy().into_owned();
                if let Some(upload) = state.open.remove(&params.upload_id) {
                    let _ = fs::remove_file(&upload.temporary);
                }
                self.save_record(&state);
                Err(RpcError::failed(failure(&error, &shown).message))
            }
        }
    }

    pub fn end(&self, params: FsUploadIdParams, now: Instant) -> Result<FsUploadEnd, RpcError> {
        check_id(&params.upload_id)?;
        let mut state = self.lock();
        self.drop_expired(&mut state, now);
        if let Some(finished) = state.finished.get(&params.upload_id) {
            return Ok(finished.result.clone());
        }
        let upload = state
            .open
            .get_mut(&params.upload_id)
            .ok_or_else(no_upload)?;
        upload.last_named = now;
        if upload.received < upload.size {
            return Err(RpcError::bad_request(format!(
                "{} of {} bytes have arrived",
                upload.received, upload.size
            )));
        }
        let Some(upload) = state.open.remove(&params.upload_id) else {
            return Err(no_upload());
        };
        let outcome = put_in_place(&upload);
        if outcome.is_err() {
            let _ = fs::remove_file(&upload.temporary);
        }
        self.save_record(&state);
        if let Ok(result) = &outcome {
            state.finished.insert(
                params.upload_id,
                Finished {
                    result: result.clone(),
                    at: now,
                },
            );
        }
        outcome
    }

    pub fn cancel(&self, params: FsUploadIdParams, now: Instant) -> Result<(), RpcError> {
        check_id(&params.upload_id)?;
        let mut state = self.lock();
        self.drop_expired(&mut state, now);
        if let Some(upload) = state.open.remove(&params.upload_id) {
            let _ = fs::remove_file(&upload.temporary);
            self.save_record(&state);
        }
        Ok(())
    }

    fn uploads_area(&self) -> PathBuf {
        self.data_dir.join(UPLOADS_AREA)
    }

    fn record(&self) -> PathBuf {
        self.data_dir.join(UNFINISHED_RECORD)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn drop_expired(&self, state: &mut State, now: Instant) {
        let open = state.open.len();
        state.open.retain(|_, upload| {
            let kept = now.duration_since(upload.last_named) < UNFINISHED_KEPT_FOR;
            if !kept {
                let _ = fs::remove_file(&upload.temporary);
            }
            kept
        });
        state
            .finished
            .retain(|_, finished| now.duration_since(finished.at) < RESULT_KEPT_FOR);
        if state.open.len() != open {
            self.save_record(state);
        }
    }

    /// Removes the temporary files the record lists that no upload of this run owns.
    fn remove_leftovers(&self, state: &State) {
        let leftovers: Vec<PathBuf> = fs::read(self.record())
            .ok()
            .and_then(|record| serde_json::from_slice(&record).ok())
            .unwrap_or_default();
        for leftover in leftovers {
            let is_temporary = leftover
                .file_name()
                .is_some_and(|name| name.as_bytes().starts_with(TEMPORARY_PREFIX.as_bytes()));
            let owned = state
                .open
                .values()
                .any(|upload| upload.temporary == leftover);
            let is_file = fs::symlink_metadata(&leftover).is_ok_and(|metadata| metadata.is_file());
            if is_temporary && !owned && is_file {
                let _ = fs::remove_file(&leftover);
            }
        }
        self.save_record(state);
    }

    /// Writes down the temporary files of the unfinished uploads.
    fn save_record(&self, state: &State) {
        let record = self.record();
        let saved = if state.open.is_empty() {
            match fs::remove_file(&record) {
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                removed => removed,
            }
        } else {
            let temporaries: Vec<&Path> = state
                .open
                .values()
                .map(|upload| upload.temporary.as_path())
                .collect();
            serde_json::to_vec(&temporaries)
                .map_err(io::Error::other)
                .and_then(|contents| {
                    let written = record.with_extension("json.new");
                    fs::write(&written, contents)?;
                    fs::rename(&written, &record)
                })
        };
        if let Err(error) = saved {
            log::warn!("[Remote access] could not keep the record of unfinished uploads: {error}");
        }
    }
}

/// Checks at the start of an upload that `directory` is one, and what is there under `name`.
fn check_directory(
    directory: &Path,
    path: &str,
    name: &str,
    on_conflict: FsOnConflict,
) -> Result<(), RpcError> {
    let metadata = fs::metadata(directory).map_err(|error| failure(&error, path))?;
    if !metadata.is_dir() {
        return Err(RpcError::new(
            ErrorCode::NotADirectory,
            format!("Not a directory: {path}"),
        ));
    }
    let shown = joined(path, name);
    match fs::symlink_metadata(directory.join(name)) {
        Ok(existing) => match on_conflict {
            FsOnConflict::Fail => Err(taken(&shown)),
            FsOnConflict::Replace if existing.is_dir() => Err(is_a_directory(&shown)),
            FsOnConflict::Replace | FsOnConflict::Rename => Ok(()),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(failure(&error, &shown)),
    }
}

/// Creates an empty temporary file in `directory` for the user only, under a new id.
fn create_temporary(state: &mut State, directory: &Path) -> io::Result<(String, File, PathBuf)> {
    for _ in 0..ID_ATTEMPTS {
        state.ids += 1;
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = format!("{since_epoch:x}-{:x}", state.ids);
        let temporary = directory.join(format!("{TEMPORARY_PREFIX}{id}"));
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        // The umask may have taken away more than it should.
        if let Err(error) = file.set_permissions(Permissions::from_mode(0o600)) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        return Ok((id, file, temporary));
    }
    Err(io::Error::from(ErrorKind::AlreadyExists))
}

/// Gives the finished file its permissions and its name.
fn put_in_place(upload: &Upload) -> Result<FsUploadEnd, RpcError> {
    let shown_temporary = upload.temporary.to_string_lossy();
    let own = upload
        .file
        .metadata()
        .map_err(|error| failure(&error, &shown_temporary))?;
    let on_disk = fs::symlink_metadata(&upload.temporary)
        .map_err(|error| failure(&error, &shown_temporary))?;
    let is_whole =
        own.len() == upload.size && on_disk.ino() == own.ino() && on_disk.dev() == own.dev();
    if !is_whole {
        return Err(RpcError::failed(format!(
            "The upload did not arrive whole: {shown_temporary}"
        )));
    }
    upload
        .file
        .sync_all()
        .map_err(|error| failure(&error, &shown_temporary))?;

    let (directory, name) = match &upload.place {
        Place::Directory {
            path,
            name,
            on_conflict,
        } => {
            let target = Path::new(path).join(name);
            let shown = joined(path, name);
            let mode = match on_conflict {
                FsOnConflict::Replace => replaced_mode(&target, &shown)?,
                FsOnConflict::Fail | FsOnConflict::Rename => new_file_mode(),
            };
            set_mode(upload, mode)?;
            let name = match on_conflict {
                FsOnConflict::Fail => {
                    place_exclusively(&upload.temporary, &target).map_err(|error| {
                        if error.kind() == ErrorKind::AlreadyExists {
                            taken(&shown)
                        } else {
                            failure(&error, &shown)
                        }
                    })?;
                    name.clone()
                }
                FsOnConflict::Replace => {
                    fs::rename(&upload.temporary, &target).map_err(|error| {
                        if error.raw_os_error() == Some(libc::EISDIR) {
                            is_a_directory(&shown)
                        } else {
                            failure(&error, &shown)
                        }
                    })?;
                    name.clone()
                }
                FsOnConflict::Rename => {
                    place_under_first_free_name(&upload.temporary, Path::new(path), path, |n| {
                        upload_names::numbered(name, n)
                    })?
                }
            };
            (path.clone(), name)
        }
        Place::UploadsArea { directory, name } => {
            set_mode(upload, 0o600)?;
            let shown = directory.to_string_lossy().into_owned();
            let name = place_under_first_free_name(&upload.temporary, directory, &shown, |n| {
                name.numbered(n)
            })?;
            (shown, name)
        }
    };
    let placed = upload
        .file
        .metadata()
        .map_err(|error| failure(&error, &joined(&directory, &name)))?;
    Ok(FsUploadEnd {
        path: joined(&directory, &name),
        name,
        size: placed.len(),
        mtime: mtime(&placed),
    })
}

/// The permission bits a file that replaces what is at `target` gets: those of the file there.
fn replaced_mode(target: &Path, shown: &str) -> Result<u32, RpcError> {
    match fs::symlink_metadata(target) {
        Ok(existing) if existing.is_dir() => Err(is_a_directory(shown)),
        Ok(existing) if existing.is_file() => Ok(existing.permissions().mode() & 0o777),
        Ok(_) => Ok(new_file_mode()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(new_file_mode()),
        Err(error) => Err(failure(&error, shown)),
    }
}

fn set_mode(upload: &Upload, mode: u32) -> Result<(), RpcError> {
    upload
        .file
        .set_permissions(Permissions::from_mode(mode))
        .map_err(|error| failure(&error, &upload.temporary.to_string_lossy()))
}

/// The permission bits of a new file: `0644` less the umask.
fn new_file_mode() -> u32 {
    static MODE: OnceLock<u32> = OnceLock::new();
    *MODE.get_or_init(|| {
        // The umask can only be read by setting it. The strictest one stands in for the moment
        // until it is put back, so that nothing created then is more open than it should be.
        // SAFETY: `umask` only swaps the process's mask.
        let umask = unsafe {
            let umask = libc::umask(0o077);
            libc::umask(umask);
            umask
        };
        0o644 & !u32::from(umask)
    })
}

/// Places `temporary` at the first name `name_for(1)`, `name_for(2)`, … in `directory` that is
/// free, and answers with that name.
fn place_under_first_free_name(
    temporary: &Path,
    directory: &Path,
    shown: &str,
    name_for: impl Fn(u32) -> String,
) -> Result<String, RpcError> {
    for n in 1..=NAME_ATTEMPTS {
        let name = name_for(n);
        match place_exclusively(temporary, &directory.join(&name)) {
            Ok(()) => return Ok(name),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(failure(&error, &joined(shown, &name))),
        }
    }
    Err(RpcError::failed(format!(
        "No free name was found in {shown}"
    )))
}

/// Renames `from` to `to` in a way that fails with [`ErrorKind::AlreadyExists`] rather than
/// replace anything at `to`, even something that appears there at the same moment.
fn place_exclusively(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;

        let from_c = CString::new(from.as_os_str().as_bytes())?;
        let to_c = CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: both are NUL-terminated strings that live for the whole call.
        let renamed =
            unsafe { libc::renamex_np(from_c.as_ptr(), to_c.as_ptr(), libc::RENAME_EXCL) };
        if renamed == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        // A file system that cannot rename this way can still link.
        if !matches!(error.raw_os_error(), Some(libc::ENOTSUP | libc::EINVAL)) {
            return Err(error);
        }
    }
    fs::hard_link(from, to)?;
    // The file is in place either way; a second name left behind is removed at the next start.
    let _ = fs::remove_file(from);
    Ok(())
}

/// The path of `name` in the directory at `path`, as the protocol writes paths.
fn joined(path: &str, name: &str) -> String {
    if path == "/" {
        format!("/{name}")
    } else {
        format!("{path}/{name}")
    }
}

fn check_id(id: &str) -> Result<(), RpcError> {
    let is_valid = (1..=MAX_ID_CHARS).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if is_valid {
        Ok(())
    } else {
        Err(RpcError::bad_request(
            "upload_id must be 1 to 64 ASCII letters, digits, - and _",
        ))
    }
}

/// The bytes of a piece, which has to be strict base64 of 1 to [`CHUNK_BYTES`] bytes.
fn decode_piece(data: &str) -> Result<Vec<u8>, RpcError> {
    let longest = CHUNK_BYTES.div_ceil(3) * 4;
    if data.len() as u64 > longest {
        return Err(RpcError::bad_request(format!(
            "A piece may hold at most {CHUNK_BYTES} bytes"
        )));
    }
    let bytes = BASE64
        .decode(data)
        .map_err(|_| RpcError::bad_request("data must be base64 with padding"))?;
    if bytes.is_empty() {
        return Err(RpcError::bad_request("A piece may not be empty"));
    }
    Ok(bytes)
}

fn no_upload() -> RpcError {
    RpcError::not_found("No such upload: it was cancelled, it expired, or Hako was restarted")
}

fn taken(shown: &str) -> RpcError {
    RpcError::new(ErrorCode::Conflict, format!("Already exists: {shown}"))
}

fn is_a_directory(shown: &str) -> RpcError {
    RpcError::new(ErrorCode::IsADirectory, format!("Is a directory: {shown}"))
}

#[cfg(test)]
#[path = "transfers_tests.rs"]
mod tests;
