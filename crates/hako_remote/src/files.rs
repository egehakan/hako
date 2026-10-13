//! The file methods of the remote protocol: the places browsing starts from, the listing of a
//! directory, the contents of a file, and replacing the contents of one. Everything but the
//! roots waits for the file system.

use std::fs::{self, File, Metadata, OpenOptions, Permissions};
use std::io::{self, ErrorKind, Read as _, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{iter, process};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::protocol::{
    ErrorCode, FsEntry, FsEntryKind, FsLinkTarget, FsList, FsRead, FsRoot, FsRootSource, FsRoots,
    FsWrite, FsWriteParams, MAX_FRAME_BYTES, RpcError, Workspace,
};

/// The longest file that is sent as text, and the longest text that is written.
pub const TEXT_LIMIT: u64 = 2 * 1024 * 1024;
pub const IMAGE_LIMIT: u64 = 5 * 1024 * 1024;
/// The most entries of one directory that are listed.
pub const LIST_LIMIT: usize = 20_000;

/// Room kept in a frame for what surrounds a file's text.
const FRAME_ENVELOPE_BYTES: usize = 4096;

/// How often a file is read again because it changed while it was read.
const READ_ATTEMPTS: usize = 3;

static TEMPORARY_FILES: AtomicU64 = AtomicU64::new(0);

/// The places browsing starts from: the folders, the home directory, and where the terminals
/// are, each path once.
pub fn roots(home: &str, workspace: &Workspace) -> FsRoots {
    let folders = workspace
        .windows
        .iter()
        .flat_map(|window| &window.folders)
        .map(|folder| FsRoot {
            path: folder.directory.clone(),
            name: folder.name.clone(),
            source: FsRootSource::Folder,
        });
    let home_root = FsRoot {
        path: home.to_owned(),
        name: "Home".to_owned(),
        source: FsRootSource::Home,
    };
    let terminals = workspace
        .windows
        .iter()
        .flat_map(|window| &window.tabs)
        .flat_map(|tab| &tab.panes)
        .filter_map(|pane| pane.cwd.as_ref())
        .map(|cwd| FsRoot {
            path: cwd.clone(),
            name: last_segment(cwd).to_owned(),
            source: FsRootSource::Terminal,
        });

    let mut roots: Vec<FsRoot> = Vec::new();
    for root in folders.chain(iter::once(home_root)).chain(terminals) {
        if roots.iter().all(|earlier| earlier.path != root.path) {
            roots.push(root);
        }
    }
    FsRoots {
        home: home.to_owned(),
        roots,
    }
}

/// The name of what `path` leads to, which for the root directory is the path itself.
fn last_segment(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((_, name)) if !name.is_empty() => name,
        Some(_) | None => path,
    }
}

pub fn list(path: &str) -> Result<FsList, RpcError> {
    list_at_most(path, LIST_LIMIT)
}

fn list_at_most(path: &str, limit: usize) -> Result<FsList, RpcError> {
    let directory = checked(path)?;
    let metadata = fs::metadata(directory).map_err(|error| failure(&error, path))?;
    if !metadata.is_dir() {
        return Err(RpcError::new(
            ErrorCode::NotADirectory,
            format!("Not a directory: {path}"),
        ));
    }
    let mut entries: Vec<FsEntry> = fs::read_dir(directory)
        .map_err(|error| failure(&error, path))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            // A name that is not UTF-8 cannot be put into JSON.
            let name = entry.file_name().into_string().ok()?;
            Some(describe(&entry.path(), name))
        })
        .collect();
    sort(&mut entries);
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    Ok(FsList { entries, truncated })
}

/// Puts directories first, and orders each group by name in lower case and then by name.
fn sort(entries: &mut [FsEntry]) {
    entries.sort_by_cached_key(|entry| {
        (
            !is_directory(entry),
            entry.name.to_lowercase(),
            entry.name.clone(),
        )
    });
}

fn is_directory(entry: &FsEntry) -> bool {
    match (entry.kind, entry.target) {
        (FsEntryKind::Dir, _) | (FsEntryKind::Symlink, Some(FsLinkTarget::Dir)) => true,
        (FsEntryKind::Symlink, Some(FsLinkTarget::File | FsLinkTarget::Other) | None)
        | (FsEntryKind::File | FsEntryKind::Other, _) => false,
    }
}

fn describe(path: &Path, name: String) -> FsEntry {
    let Ok(own) = fs::symlink_metadata(path) else {
        return FsEntry {
            name,
            kind: FsEntryKind::Other,
            target: None,
            size: 0,
            mtime: 0,
        };
    };
    let file_type = own.file_type();
    if file_type.is_symlink() {
        return match fs::metadata(path) {
            Ok(target) => FsEntry {
                name,
                kind: FsEntryKind::Symlink,
                target: Some(if target.is_dir() {
                    FsLinkTarget::Dir
                } else if target.is_file() {
                    FsLinkTarget::File
                } else {
                    FsLinkTarget::Other
                }),
                size: size(&target),
                mtime: mtime(&target),
            },
            Err(_) => FsEntry {
                name,
                kind: FsEntryKind::Symlink,
                target: None,
                size: 0,
                mtime: mtime(&own),
            },
        };
    }
    FsEntry {
        name,
        kind: if file_type.is_dir() {
            FsEntryKind::Dir
        } else if file_type.is_file() {
            FsEntryKind::File
        } else {
            FsEntryKind::Other
        },
        target: None,
        size: size(&own),
        mtime: mtime(&own),
    }
}

fn size(metadata: &Metadata) -> u64 {
    if metadata.is_dir() { 0 } else { metadata.len() }
}

/// The time of the last change in whole milliseconds since the Unix epoch, rounded down.
pub(crate) fn mtime(metadata: &Metadata) -> i64 {
    let Ok(modified) = metadata.modified() else {
        return 0;
    };
    let milliseconds = match modified.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()),
        Err(before) => {
            i64::try_from(before.duration().as_nanos().div_ceil(1_000_000)).map(|ms| -ms)
        }
    };
    milliseconds.unwrap_or_default()
}

pub fn read(path: &str) -> Result<FsRead, RpcError> {
    let file = checked(path)?;
    let metadata = regular_file(file, path)?;
    let mime = image_mime(path);
    let limit = if mime.is_some() {
        IMAGE_LIMIT
    } else {
        TEXT_LIMIT
    };
    if metadata.len() > limit {
        return Ok(FsRead::TooLarge {
            size: metadata.len(),
            limit,
        });
    }

    let (bytes, metadata) = read_bytes(file, limit).map_err(|error| failure(&error, path))?;
    let size = bytes.len() as u64;
    if size > limit {
        return Ok(FsRead::TooLarge {
            size: size.max(metadata.len()),
            limit,
        });
    }
    let mtime = mtime(&metadata);
    if let Some(mime) = mime {
        return Ok(FsRead::Image {
            mime: mime.to_owned(),
            data: BASE64.encode(bytes),
            mtime,
            size,
        });
    }
    if bytes.contains(&0) {
        return Ok(FsRead::Binary { size });
    }
    let Ok(content) = String::from_utf8(bytes) else {
        return Ok(FsRead::Binary { size });
    };
    if json_len(&content) + FRAME_ENVELOPE_BYTES > MAX_FRAME_BYTES {
        return Ok(FsRead::TooLarge {
            size,
            limit: TEXT_LIMIT,
        });
    }
    Ok(FsRead::Text {
        content,
        mtime,
        size,
    })
}

/// The media type of a file that is sent as an image, which its name alone decides.
fn image_mime(path: &str) -> Option<&'static str> {
    let (_, extension) = last_segment(path).rsplit_once('.')?;
    match extension.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// Reads a file's first `limit` bytes and one more, which shows that there are more. The
/// metadata is the one the file had when the bytes were read.
fn read_bytes(path: &Path, limit: u64) -> io::Result<(Vec<u8>, Metadata)> {
    let mut attempts = READ_ATTEMPTS;
    loop {
        let mut file = File::open(path)?;
        let before = mtime(&file.metadata()?);
        let mut bytes = Vec::new();
        (&mut file).take(limit + 1).read_to_end(&mut bytes)?;
        let after = file.metadata()?;
        attempts -= 1;
        if mtime(&after) == before || attempts == 0 {
            return Ok((bytes, after));
        }
    }
}

/// How many bytes `text` takes as a string in JSON, where quotes, backslashes and control
/// characters are escaped.
fn json_len(text: &str) -> usize {
    let escaped: usize = text
        .bytes()
        .map(|byte| match byte {
            b'"' | b'\\' | 0x08 | b'\t' | b'\n' | 0x0C | b'\r' => 2,
            0x00..=0x1F => 6,
            _ => 1,
        })
        .sum();
    escaped + 2
}

pub fn write(params: &FsWriteParams) -> Result<FsWrite, RpcError> {
    let path = params.path.as_str();
    let given = checked(path)?;
    if params.content.len() as u64 > TEXT_LIMIT {
        return Err(RpcError::new(
            ErrorCode::TooLarge,
            format!("The text is over the limit of {TEXT_LIMIT} bytes"),
        ));
    }
    // Through a link, the file it points to is the one replaced, and the link stays a link.
    let file = fs::canonicalize(given).map_err(|error| failure(&error, path))?;
    let metadata = regular_file(&file, path)?;
    if params
        .expected_mtime
        .is_some_and(|expected| expected != mtime(&metadata))
    {
        return Err(RpcError::new(
            ErrorCode::Conflict,
            format!("Changed on the Mac since it was read: {path}"),
        ));
    }
    // Renaming over a file takes only the right to change its directory, so a file that may not
    // be written to is turned away here.
    OpenOptions::new()
        .write(true)
        .open(&file)
        .map_err(|error| failure(&error, path))?;
    replace(&file, params.content.as_bytes(), metadata.permissions())
        .map_err(|error| failure(&error, path))?;
    let metadata = fs::metadata(&file).map_err(|error| failure(&error, path))?;
    Ok(FsWrite {
        mtime: mtime(&metadata),
        size: metadata.len(),
    })
}

/// Puts `bytes` in the place of `file` in one step, so that the file is whole at every moment.
fn replace(file: &Path, bytes: &[u8], permissions: Permissions) -> io::Result<()> {
    let directory = file
        .parent()
        .ok_or_else(|| io::Error::from(ErrorKind::InvalidInput))?;
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = directory.join(format!(
        ".hako-write-{}-{since_epoch}-{}",
        process::id(),
        TEMPORARY_FILES.fetch_add(1, Ordering::Relaxed),
    ));
    let replaced =
        write_new(&temporary, bytes, permissions).and_then(|()| fs::rename(&temporary, file));
    if replaced.is_err() {
        // Nothing to do about a temporary file that cannot be removed, or was never made.
        let _ = fs::remove_file(&temporary);
    }
    replaced
}

fn write_new(path: &Path, bytes: &[u8], permissions: Permissions) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    // Before the contents, so that nobody can read them who may not read the file itself.
    file.set_permissions(permissions)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The path as the file system takes it, if it has the one form the protocol accepts: absolute,
/// with single slashes, and without `.`, `..` or a slash at the end.
pub(crate) fn checked(path: &str) -> Result<&Path, RpcError> {
    if is_normal(path) {
        Ok(Path::new(path))
    } else {
        Err(RpcError::bad_request(
            "path must be absolute, with single slashes and without . or .. or a slash at the end",
        ))
    }
}

/// Whether `path` has the one form the protocol accepts for an absolute path.
pub(crate) fn is_normal(path: &str) -> bool {
    let is_absolute = path == "/"
        || path.strip_prefix('/').is_some_and(|rest| {
            rest.split('/')
                .all(|segment| !matches!(segment, "" | "." | ".."))
        });
    is_absolute && !path.contains('\0')
}

/// The metadata of the regular file at `file`, which links are followed to.
pub(crate) fn regular_file(file: &Path, path: &str) -> Result<Metadata, RpcError> {
    let metadata = fs::metadata(file).map_err(|error| failure(&error, path))?;
    if metadata.is_dir() {
        return Err(RpcError::new(
            ErrorCode::IsADirectory,
            format!("Is a directory: {path}"),
        ));
    }
    if !metadata.is_file() {
        return Err(RpcError::failed(format!("Not a regular file: {path}")));
    }
    Ok(metadata)
}

/// What to answer when the file system refuses, with its own words for the owner.
pub(crate) fn failure(error: &io::Error, path: &str) -> RpcError {
    let code = match error.kind() {
        ErrorKind::NotFound | ErrorKind::NotADirectory => ErrorCode::NotFound,
        ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        // A link that leads back to itself points nowhere.
        _ if error.raw_os_error() == Some(libc::ELOOP) => ErrorCode::NotFound,
        _ => ErrorCode::Failed,
    };
    let said = error.to_string();
    let said = said.split(" (os error").next().unwrap_or(&said);
    RpcError::new(code, format!("{said}: {path}"))
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
