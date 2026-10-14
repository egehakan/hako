//! The file methods of the remote protocol that change what is where: describing one entry
//! without following it, making an empty file, renaming, copying and moving items into a
//! directory, and moving them to the Trash. Every one waits for the file system, and the ones
//! that can take long stop at a [`Deadline`].

use std::ffi::CString;
use std::fs::{self, DirBuilder, File, Metadata, OpenOptions, Permissions};
use std::io::{self, ErrorKind, Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{
    DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _, symlink,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{process, thread};

use crate::files::{
    already_exists, checked, failure, joined, last_segment, mtime, not_a_directory,
};
use crate::protocol::{
    ErrorCode, FsCopyParams, FsCreateFile, FsEntryKind, FsItemResult, FsItemResults, FsOnConflict,
    FsPath, FsRenameParams, FsStat, FsTrashParams, RpcError,
};
use crate::upload_names;

/// How long a request that can take long is worked on.
pub const WORK_LIMIT: Duration = Duration::from_secs(240);
/// The most items one request acts on.
pub const MAX_ITEMS: usize = 500;

const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const TEMPORARY_PREFIX: &str = ".hako-copy-";
/// How many numbered names are tried for an item before it is given up.
const NAME_ATTEMPTS: u32 = 10_000;
const NAME_RULE: &str = "A name must be 1 to 255 bytes, not . or .., and without / or NUL";

static TEMPORARY_NAMES: AtomicU64 = AtomicU64::new(0);

/// When work on a request stops.
#[derive(Clone, Debug)]
pub struct Deadline {
    at: Instant,
    /// How many more checks pass, in tests that stop work part of the way.
    #[cfg(test)]
    checks_left: Option<std::sync::Arc<AtomicU64>>,
}

impl Deadline {
    /// [`WORK_LIMIT`] from now.
    pub fn from_now() -> Self {
        Self::at(Instant::now() + WORK_LIMIT)
    }

    pub fn at(at: Instant) -> Self {
        Self {
            at,
            #[cfg(test)]
            checks_left: None,
        }
    }

    /// A deadline that passes once it has been checked `checks` times.
    #[cfg(test)]
    pub(crate) fn after_checks(checks: u64) -> Self {
        Self {
            checks_left: Some(std::sync::Arc::new(AtomicU64::new(checks))),
            ..Self::from_now()
        }
    }

    pub(crate) fn check(&self) -> Result<(), Trouble> {
        #[cfg(test)]
        if let Some(checks_left) = &self.checks_left {
            let passes = checks_left
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                    left.checked_sub(1)
                })
                .is_ok();
            return if passes {
                Ok(())
            } else {
                Err(Trouble::Stopped)
            };
        }
        if Instant::now() < self.at {
            Ok(())
        } else {
            Err(Trouble::Stopped)
        }
    }
}

/// Why work on an item stopped.
#[derive(Debug)]
pub(crate) enum Trouble {
    /// The file system refused, at the path of the source it was working on.
    Io(io::Error, PathBuf),
    Stopped,
}

impl Trouble {
    pub(crate) fn at(path: &Path) -> impl FnOnce(io::Error) -> Self + '_ {
        move |error| Self::Io(error, path.to_path_buf())
    }

    pub(crate) fn into_rpc(self) -> RpcError {
        match self {
            Self::Io(error, path) => failure(&error, &path.to_string_lossy()),
            Self::Stopped => stopped(),
        }
    }
}

pub(crate) fn stopped() -> RpcError {
    RpcError::failed(format!("Stopped after {} seconds", WORK_LIMIT.as_secs()))
}

/// Describes the entry at `path` itself: a link is reported as a link, with what is written
/// in it.
pub fn stat(path: &str) -> Result<FsStat, RpcError> {
    let entry = checked(path)?;
    let metadata = fs::symlink_metadata(entry).map_err(|error| failure(&error, path))?;
    let file_type = metadata.file_type();
    let (kind, target) = if file_type.is_symlink() {
        let target = fs::read_link(entry).map_err(|error| failure(&error, path))?;
        (
            FsEntryKind::Symlink,
            Some(target.to_string_lossy().into_owned()),
        )
    } else if file_type.is_dir() {
        (FsEntryKind::Dir, None)
    } else if file_type.is_file() {
        (FsEntryKind::File, None)
    } else {
        (FsEntryKind::Other, None)
    };
    Ok(FsStat {
        kind,
        size: if file_type.is_dir() {
            0
        } else {
            metadata.len()
        },
        mtime: mtime(&metadata),
        target,
    })
}

/// Makes an empty regular file at `path`, in a directory that is already there.
pub fn create_file(path: &str) -> Result<FsCreateFile, RpcError> {
    let file = checked(path)?;
    let Some(parent) = file.parent() else {
        return Err(already_exists(path));
    };
    check_name(last_segment(path))?;
    let within = parent.to_string_lossy();
    let metadata = fs::metadata(parent).map_err(|error| failure(&error, &within))?;
    if !metadata.is_dir() {
        return Err(not_a_directory(&within));
    }
    let created = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o666)
        .open(file)
        .map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                already_exists(path)
            } else {
                failure(&error, path)
            }
        })?;
    let metadata = created.metadata().map_err(|error| failure(&error, path))?;
    Ok(FsCreateFile {
        mtime: mtime(&metadata),
    })
}

/// Gives the entry at `path` the name `name` in the same directory.
pub fn rename(params: &FsRenameParams, home: &Path) -> Result<FsPath, RpcError> {
    let path = params.path.as_str();
    let entry = checked(path)?;
    check_name(&params.name)?;
    let Some(parent) = entry.parent() else {
        return Err(RpcError::bad_request(
            "The root directory cannot be renamed",
        ));
    };
    if is_same_place(entry, home) {
        return Err(RpcError::bad_request(
            "The home directory cannot be renamed",
        ));
    }
    let own = fs::symlink_metadata(entry).map_err(|error| failure(&error, path))?;
    let renamed = joined(parent_of(path), &params.name);
    let old_name = last_segment(path);
    if old_name == params.name {
        return Ok(FsPath {
            path: path.to_owned(),
        });
    }
    let target = parent.join(&params.name);
    // On a file system that ignores case the new name already leads to the entry itself.
    let is_case_change = old_name.to_lowercase() == params.name.to_lowercase()
        && fs::symlink_metadata(&target)
            .is_ok_and(|there| there.dev() == own.dev() && there.ino() == own.ino())
        && !is_listed(parent, &params.name);
    let outcome = if is_case_change {
        fs::rename(entry, &target)
    } else {
        rename_exclusively(entry, &target)
    };
    outcome.map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            already_exists(&renamed)
        } else {
            failure(&error, path)
        }
    })?;
    Ok(FsPath { path: renamed })
}

/// Whether `directory` holds an entry of exactly `name`, byte for byte.
fn is_listed(directory: &Path, name: &str) -> bool {
    fs::read_dir(directory).is_ok_and(|mut entries| {
        entries
            .any(|entry| entry.is_ok_and(|entry| entry.file_name().as_bytes() == name.as_bytes()))
    })
}

fn check_name(name: &str) -> Result<(), RpcError> {
    if upload_names::is_valid(name) {
        Ok(())
    } else {
        Err(RpcError::bad_request(NAME_RULE))
    }
}

/// The directory part of a path in the protocol's form.
fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(slash) => &path[..slash],
    }
}

/// `path` with the links on the way to it resolved, but not a link at its end.
fn resolved(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| path.to_path_buf()),
        (Some(_) | None, _) => path.to_path_buf(),
    }
}

/// Whether `entry` is the directory at `place`, by its path or through links on the way.
fn is_same_place(entry: &Path, place: &Path) -> bool {
    entry == place || fs::canonicalize(place).is_ok_and(|place| resolved(entry) == place)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyKind {
    Copy,
    Move,
}

/// How an item is put into the destination directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// Under its own name, which is free.
    Free,
    /// Under the first free numbered name.
    KeepBoth,
    /// Over the file or link of its name.
    Replace,
}

/// What is done with an item whose name in the destination is taken by `existing`, which is
/// a directory or not, or free when `None`.
pub(crate) fn placement(
    existing: Option<bool>,
    source_is_dir: bool,
    is_onto_itself: bool,
    on_conflict: FsOnConflict,
) -> Result<Placement, &'static str> {
    let Some(existing_is_dir) = existing else {
        return Ok(Placement::Free);
    };
    match on_conflict {
        FsOnConflict::Fail => Err("Already exists"),
        FsOnConflict::Rename => Ok(Placement::KeepBoth),
        FsOnConflict::Replace if is_onto_itself => Err("An item cannot replace itself"),
        FsOnConflict::Replace if existing_is_dir || source_is_dir => {
            Err("Only a file or a link replaces a file or a link")
        }
        FsOnConflict::Replace => Ok(Placement::Replace),
    }
}

/// Copies or moves each of the sources into the destination directory, each on its own, and
/// tells what became of each.
pub fn copy_or_move(
    params: &FsCopyParams,
    kind: CopyKind,
    deadline: &Deadline,
) -> Result<FsItemResults, RpcError> {
    check_count(params.sources.len(), "sources")?;
    let destination = checked(&params.destination)?;
    let metadata =
        fs::metadata(destination).map_err(|error| failure(&error, &params.destination))?;
    if !metadata.is_dir() {
        return Err(not_a_directory(&params.destination));
    }
    check_writable(destination, &params.destination)?;
    let place =
        fs::canonicalize(destination).map_err(|error| failure(&error, &params.destination))?;
    let into = Destination {
        path: &params.destination,
        directory: destination,
        place: &place,
    };
    let on_conflict = params.on_conflict.unwrap_or_default();
    let results = params
        .sources
        .iter()
        .map(
            |source| match place_one(source, &into, on_conflict, kind, deadline) {
                Ok(placed) => placed,
                Err(error) => FsItemResult {
                    source: source.clone(),
                    path: None,
                    error: Some(error),
                },
            },
        )
        .collect();
    Ok(FsItemResults { results })
}

fn check_count(count: usize, field: &str) -> Result<(), RpcError> {
    if (1..=MAX_ITEMS).contains(&count) {
        Ok(())
    } else {
        Err(RpcError::bad_request(format!(
            "{field} must hold 1 to {MAX_ITEMS} paths"
        )))
    }
}

/// Answers `permission_denied` for a directory nothing may be put into.
fn check_writable(directory: &Path, path: &str) -> Result<(), RpcError> {
    let directory_c = CString::new(directory.as_os_str().as_bytes())
        .map_err(|_| RpcError::bad_request("A path may not contain NUL"))?;
    // SAFETY: a NUL-terminated string that lives for the whole call.
    if unsafe { libc::access(directory_c.as_ptr(), libc::W_OK | libc::X_OK) } == 0 {
        Ok(())
    } else {
        Err(failure(&io::Error::last_os_error(), path))
    }
}

struct Destination<'a> {
    /// As the client sent it.
    path: &'a str,
    directory: &'a Path,
    /// With every link resolved.
    place: &'a Path,
}

fn place_one(
    source_path: &str,
    into: &Destination,
    on_conflict: FsOnConflict,
    kind: CopyKind,
    deadline: &Deadline,
) -> Result<FsItemResult, RpcError> {
    let source = checked(source_path)?;
    deadline.check().map_err(Trouble::into_rpc)?;
    let Some(parent) = source.parent() else {
        return Err(RpcError::bad_request(
            "The root directory cannot be copied or moved",
        ));
    };
    let own = fs::symlink_metadata(source).map_err(|error| failure(&error, source_path))?;
    let file_type = own.file_type();
    if !(file_type.is_dir() || file_type.is_file() || file_type.is_symlink()) {
        return Err(RpcError::failed(format!(
            "Not a file, a directory or a link: {source_path}"
        )));
    }
    let parent_place =
        fs::canonicalize(parent).map_err(|error| failure(&error, parent_of(source_path)))?;
    let name = last_segment(source_path);
    let source_place = parent_place.join(name);
    if file_type.is_dir() && into.place.starts_with(&source_place) {
        return Err(RpcError::bad_request(format!(
            "A directory cannot go into itself: {source_path}"
        )));
    }
    let is_in_place = into.place == parent_place;
    if kind == CopyKind::Move && is_in_place {
        return Ok(FsItemResult {
            source: source_path.to_owned(),
            path: Some(source_path.to_owned()),
            error: None,
        });
    }

    let target = into.directory.join(name);
    let shown = joined(into.path, name);
    let existing = match fs::symlink_metadata(&target) {
        Ok(existing) => Some(existing.is_dir()),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(failure(&error, &shown)),
    };
    let placement = placement(existing, file_type.is_dir(), is_in_place, on_conflict)
        .map_err(|reason| RpcError::new(ErrorCode::Conflict, format!("{reason}: {shown}")))?;
    let numbered = |n| {
        if file_type.is_dir() {
            upload_names::numbered_directory(name, n)
        } else {
            upload_names::numbered(name, n)
        }
    };

    if kind == CopyKind::Move {
        let placing = Placing {
            name,
            placement,
            numbered: &numbered,
        };
        match put(source, into, &placing) {
            Ok(placed) => {
                return Ok(FsItemResult {
                    source: source_path.to_owned(),
                    path: Some(joined(into.path, &placed)),
                    error: None,
                });
            }
            Err(error) if error.kind() == ErrorKind::CrossesDevices => {}
            Err(error) => return Err(placing_failure(&error, &shown, source_path)),
        }
    }

    Ok(copy_then_place(
        source,
        source_path,
        into,
        Placing {
            name,
            placement,
            numbered: &numbered,
        },
        kind,
        deadline,
    ))
}

/// Where in the destination an item goes.
struct Placing<'a> {
    name: &'a str,
    placement: Placement,
    numbered: &'a dyn Fn(u32) -> String,
}

/// Copies an item beside where it goes and then puts it there, so that nothing of it appears
/// under its name before it is whole. A move then removes the original.
fn copy_then_place(
    source: &Path,
    source_path: &str,
    into: &Destination,
    placing: Placing,
    kind: CopyKind,
    deadline: &Deadline,
) -> FsItemResult {
    let failed = |error| FsItemResult {
        source: source_path.to_owned(),
        path: None,
        error: Some(error),
    };
    let temporary = temporary_in(into.directory);
    if let Err(trouble) = copy_entry(source, &temporary, deadline) {
        let _ = remove_tree(&temporary);
        return failed(trouble.into_rpc());
    }
    let placed = match put(&temporary, into, &placing) {
        Ok(placed) => joined(into.path, &placed),
        Err(error) => {
            let _ = remove_tree(&temporary);
            let shown = joined(into.path, placing.name);
            return failed(placing_failure(&error, &shown, source_path));
        }
    };
    let removed = match kind {
        CopyKind::Copy => Ok(()),
        CopyKind::Move => remove_tree(source),
    };
    FsItemResult {
        source: source_path.to_owned(),
        error: removed.err().map(|error| {
            RpcError::failed(format!(
                "Copied to {placed}, but the original could not be removed: {}",
                failure(&error, source_path).message
            ))
        }),
        path: Some(placed),
    }
}

fn placing_failure(error: &io::Error, shown: &str, source_path: &str) -> RpcError {
    match error.kind() {
        ErrorKind::AlreadyExists => already_exists(shown),
        _ if error.raw_os_error() == Some(libc::EISDIR) => RpcError::new(
            ErrorCode::Conflict,
            format!("Only a file or a link replaces a file or a link: {shown}"),
        ),
        _ => failure(error, source_path),
    }
}

/// A name in `directory` for a copy that is not finished, which nothing else uses.
fn temporary_in(directory: &Path) -> PathBuf {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    directory.join(format!(
        "{TEMPORARY_PREFIX}{}-{since_epoch:x}-{:x}",
        process::id(),
        TEMPORARY_NAMES.fetch_add(1, Ordering::Relaxed),
    ))
}

/// Renames `from` into the destination as `placement` says, and answers with the name it got.
fn put(from: &Path, into: &Destination, placing: &Placing) -> io::Result<String> {
    let name = placing.name;
    match placing.placement {
        Placement::Free => {
            rename_exclusively(from, &into.directory.join(name))?;
            Ok(name.to_owned())
        }
        Placement::Replace => {
            fs::rename(from, into.directory.join(name))?;
            Ok(name.to_owned())
        }
        Placement::KeepBoth => {
            for n in 2..=NAME_ATTEMPTS {
                let numbered = (placing.numbered)(n);
                match rename_exclusively(from, &into.directory.join(&numbered)) {
                    Ok(()) => return Ok(numbered),
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::from(ErrorKind::AlreadyExists))
        }
    }
}

/// Renames `from` to `to` in a way that fails with [`ErrorKind::AlreadyExists`] rather than
/// replace anything at `to`.
pub(crate) fn rename_exclusively(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let from_c = CString::new(from.as_os_str().as_bytes())?;
        let to_c = CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: both are NUL-terminated strings that live for the whole call.
        let renamed =
            unsafe { libc::renamex_np(from_c.as_ptr(), to_c.as_ptr(), libc::RENAME_EXCL) };
        if renamed == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::ENOTSUP)) {
            return Err(error);
        }
    }
    // Where the file system cannot do it in one step, a name taken in between is replaced.
    if fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::from(ErrorKind::AlreadyExists));
    }
    fs::rename(from, to)
}

/// Copies the file, directory or link at `source` to `target`, which must not exist, with its
/// permissions and modification time. Links are copied as links, never followed, and what is
/// neither a file, a directory nor a link inside a directory is left out.
pub(crate) fn copy_entry(source: &Path, target: &Path, deadline: &Deadline) -> Result<(), Trouble> {
    deadline.check()?;
    let metadata = fs::symlink_metadata(source).map_err(Trouble::at(source))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let link = fs::read_link(source).map_err(Trouble::at(source))?;
        symlink(link, target).map_err(Trouble::at(source))?;
    } else if file_type.is_dir() {
        DirBuilder::new()
            .mode(0o700)
            .create(target)
            .map_err(Trouble::at(source))?;
        for entry in fs::read_dir(source).map_err(Trouble::at(source))? {
            let entry = entry.map_err(Trouble::at(source))?;
            let path = entry.path();
            let is_copied = entry.file_type().is_ok_and(|file_type| {
                file_type.is_dir() || file_type.is_file() || file_type.is_symlink()
            });
            if is_copied {
                copy_entry(&path, &target.join(entry.file_name()), deadline)?;
            }
        }
        // Last, so that a directory that may not be written to could still be filled.
        fs::set_permissions(target, metadata.permissions()).map_err(Trouble::at(source))?;
    } else if file_type.is_file() {
        copy_file(source, target, &metadata, deadline)?;
    } else {
        return Ok(());
    }
    set_times(target, &metadata).map_err(Trouble::at(source))
}

fn copy_file(
    source: &Path,
    target: &Path,
    metadata: &Metadata,
    deadline: &Deadline,
) -> Result<(), Trouble> {
    #[cfg(target_os = "macos")]
    {
        if clone_file(source, target).is_ok() {
            return Ok(());
        }
    }
    let mut from = File::open(source).map_err(Trouble::at(source))?;
    let mut to = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(target)
        .map_err(Trouble::at(source))?;
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
    loop {
        deadline.check()?;
        let read = match from.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(Trouble::Io(error, source.to_path_buf())),
        };
        to.write_all(&buffer[..read]).map_err(Trouble::at(source))?;
    }
    to.set_permissions(metadata.permissions())
        .map_err(Trouble::at(source))
}

/// A copy that shares the file's blocks until either changes, on file systems that can make
/// one. It keeps the file's permissions, times and extended attributes.
#[cfg(target_os = "macos")]
fn clone_file(source: &Path, target: &Path) -> io::Result<()> {
    const CLONE_NOFOLLOW: u32 = 0x0001;
    let source_c = CString::new(source.as_os_str().as_bytes())?;
    let target_c = CString::new(target.as_os_str().as_bytes())?;
    // SAFETY: both are NUL-terminated strings that live for the whole call.
    let cloned = unsafe { libc::clonefile(source_c.as_ptr(), target_c.as_ptr(), CLONE_NOFOLLOW) };
    if cloned == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Gives the entry at `path`, or the link itself, the access and modification times in
/// `metadata`.
fn set_times(path: &Path, metadata: &Metadata) -> io::Result<()> {
    set_raw_times(
        path,
        (metadata.atime(), metadata.atime_nsec()),
        (metadata.mtime(), metadata.mtime_nsec()),
    )
}

/// Sets the times of `path`, or of the link itself, from seconds and nanoseconds since the
/// epoch.
pub(crate) fn set_raw_times(
    path: &Path,
    accessed: (i64, i64),
    modified: (i64, i64),
) -> io::Result<()> {
    let path_c = CString::new(path.as_os_str().as_bytes())?;
    let time = |(seconds, nanoseconds): (i64, i64)| libc::timespec {
        tv_sec: seconds as libc::time_t,
        tv_nsec: nanoseconds as libc::c_long,
    };
    let times = [time(accessed), time(modified)];
    // SAFETY: a NUL-terminated string and an array of two times, both alive for the whole call.
    let set = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            path_c.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if set == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Removes the file, link or whole directory at `path`, also where a directory in it may not
/// be written to, which the owner of what is removed may change.
pub(crate) fn remove_tree(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return fs::remove_file(path);
    }
    let mode = metadata.permissions().mode();
    if mode & 0o700 != 0o700 {
        fs::set_permissions(path, Permissions::from_mode(mode | 0o700))?;
    }
    for entry in fs::read_dir(path)? {
        remove_tree(&entry?.path())?;
    }
    fs::remove_dir(path)
}

/// Moves each of `paths` to the Trash with `move_to_trash`, which answers with where the item
/// went, never the home directory, the root directory or a folder's own directory.
pub fn trash(
    params: &FsTrashParams,
    home: &Path,
    folders: &[String],
    move_to_trash: impl Fn(&Path) -> Result<PathBuf, String>,
) -> Result<FsItemResults, RpcError> {
    check_count(params.paths.len(), "paths")?;
    let results = params
        .paths
        .iter()
        .map(|path| {
            let outcome = trash_one(path, home, folders, &move_to_trash);
            FsItemResult {
                source: path.clone(),
                error: outcome.as_ref().err().cloned(),
                path: outcome.ok(),
            }
        })
        .collect();
    Ok(FsItemResults { results })
}

fn trash_one(
    path: &str,
    home: &Path,
    folders: &[String],
    move_to_trash: impl Fn(&Path) -> Result<PathBuf, String>,
) -> Result<String, RpcError> {
    let entry = checked(path)?;
    if path == "/" {
        return Err(RpcError::bad_request(
            "The root directory cannot be moved to the Trash",
        ));
    }
    if is_same_place(entry, home) {
        return Err(RpcError::bad_request(
            "The home directory cannot be moved to the Trash",
        ));
    }
    if folders
        .iter()
        .any(|folder| is_same_place(entry, Path::new(folder)))
    {
        return Err(RpcError::bad_request(format!(
            "The directory of a folder cannot be moved to the Trash: {path}"
        )));
    }
    fs::symlink_metadata(entry).map_err(|error| failure(&error, path))?;
    let in_trash = move_to_trash(entry).map_err(RpcError::failed)?;
    Ok(in_trash.to_string_lossy().into_owned())
}

/// Runs `work` on a thread of its own, so that long work does not hold up what else waits for
/// a thread, and answers with what it made.
pub async fn on_own_thread<R: Send + 'static>(
    work: impl FnOnce() -> R + Send + 'static,
) -> Result<R, RpcError> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    thread::Builder::new()
        .name("hako-remote-files".to_owned())
        .spawn(move || {
            let _ = sender.send(work());
        })
        .map_err(|error| RpcError::failed(format!("The work could not be started: {error}")))?;
    receiver
        .await
        .map_err(|_| RpcError::failed("The work ended without an answer"))
}

#[cfg(test)]
#[path = "file_ops_tests.rs"]
mod tests;
