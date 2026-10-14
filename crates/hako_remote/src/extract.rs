//! Unpacking a zip file into a new directory beside where it is asked for, refusing every
//! archive that would put anything outside that directory.

use std::fmt::Display;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{BufReader, ErrorKind, Read as _, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate, TimeZone as _};
use zip::{DateTime, ZipArchive};

use crate::file_ops::{Deadline, remove_tree, set_raw_times, stopped};
use crate::files::{checked, failure, joined, last_segment, not_a_directory, regular_file};
use crate::protocol::{FsExtract, FsExtractParams, RpcError};
use crate::upload_names;

/// How much an archive may hold.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Unpacked, all entries together.
    pub bytes: u64,
    pub entries: u64,
}

pub const LIMITS: Limits = Limits {
    bytes: 1024 * 1024 * 1024,
    entries: 50_000,
};

const COPY_BUFFER_BYTES: usize = 1024 * 1024;
/// The longest link target that is read from an archive.
const MAX_LINK_BYTES: u64 = 4096;
/// How many numbered names are tried for the new directory before it is given up.
const NAME_ATTEMPTS: u32 = 10_000;

type Archive = ZipArchive<BufReader<File>>;

/// Unpacks the zip file at `params.path` into a new directory in `params.destination`, named
/// after the file. On any failure the new directory is removed again.
pub fn extract(
    params: &FsExtractParams,
    limits: Limits,
    deadline: &Deadline,
) -> Result<FsExtract, RpcError> {
    let path = params.path.as_str();
    let zip_file = checked(path)?;
    regular_file(zip_file, path)?;
    let destination = checked(&params.destination)?;
    let metadata =
        fs::metadata(destination).map_err(|error| failure(&error, &params.destination))?;
    if !metadata.is_dir() {
        return Err(not_a_directory(&params.destination));
    }
    let file = File::open(zip_file).map_err(|error| failure(&error, path))?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(damaged)?;
    let plan = plan(&mut archive, limits)?;

    let (name, directory) = new_directory(destination, &directory_name(last_segment(path)))
        .map_err(|error| failure(&error, &params.destination))?;
    match unpack(&mut archive, &plan, &directory, limits, deadline) {
        Ok(entries) => Ok(FsExtract {
            path: joined(&params.destination, &name),
            entries,
        }),
        Err(error) => {
            let _ = remove_tree(&directory);
            Err(error)
        }
    }
}

fn damaged(error: impl Display) -> RpcError {
    RpcError::failed(format!("The archive is damaged or not a zip file: {error}"))
}

fn outside(name: &str) -> RpcError {
    RpcError::bad_request(format!(
        "The archive has an entry that would land outside its directory: {name}"
    ))
}

/// The name of the directory a zip file called `zip_name` is unpacked into: its name without
/// the extension.
fn directory_name(zip_name: &str) -> String {
    let stem = match zip_name.rfind('.') {
        Some(dot) if dot > 0 => &zip_name[..dot],
        Some(_) | None => zip_name,
    };
    stem.to_owned()
}

fn new_directory(destination: &Path, stem: &str) -> std::io::Result<(String, PathBuf)> {
    for n in 1..=NAME_ATTEMPTS {
        let name = upload_names::numbered_directory(stem, n);
        let directory = destination.join(&name);
        match fs::create_dir(&directory) {
            Ok(()) => return Ok((name, directory)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(ErrorKind::AlreadyExists.into())
}

/// An entry that is unpacked.
#[derive(Debug)]
struct Planned {
    index: usize,
    /// The path in the new directory, one name after the other.
    components: Vec<String>,
    kind: PlannedKind,
    mode: Option<u32>,
    modified: Option<DateTime>,
}

#[derive(Debug)]
enum PlannedKind {
    Directory,
    File,
    Link(String),
}

/// Looks at every entry before anything is written, and refuses the archive for any that
/// cannot be unpacked safely.
fn plan(archive: &mut Archive, limits: Limits) -> Result<Vec<Planned>, RpcError> {
    if archive.len() as u64 > limits.entries {
        return Err(RpcError::too_large(
            format!("The archive holds more than {} entries", limits.entries),
            limits.entries,
        ));
    }
    let mut unpacked_bytes: u64 = 0;
    let mut plan = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(damaged)?;
        let name = entry.name().to_owned();
        if entry.encrypted() {
            return Err(RpcError::bad_request(
                "The archive is encrypted, and Hako does not unpack encrypted archives",
            ));
        }
        unpacked_bytes = unpacked_bytes.saturating_add(entry.size());
        if unpacked_bytes > limits.bytes {
            return Err(too_large(limits));
        }
        let components = components(&name).ok_or_else(|| outside(&name))?;
        let (is_dir, is_symlink, mode, modified) = (
            entry.is_dir(),
            entry.is_symlink(),
            entry.unix_mode(),
            entry.last_modified(),
        );
        drop(entry);
        if components.is_empty() || is_left_out(&components) {
            continue;
        }
        let kind = if is_dir {
            PlannedKind::Directory
        } else if is_symlink {
            let mut target = String::new();
            archive
                .by_index(index)
                .map_err(damaged)?
                .take(MAX_LINK_BYTES)
                .read_to_string(&mut target)
                .map_err(damaged)?;
            if !link_stays_inside(&target, components.len() - 1) {
                return Err(outside(&name));
            }
            PlannedKind::Link(target)
        } else {
            PlannedKind::File
        };
        plan.push(Planned {
            index,
            components: components.into_iter().map(str::to_owned).collect(),
            kind,
            mode,
            modified,
        });
    }
    Ok(plan)
}

fn too_large(limits: Limits) -> RpcError {
    RpcError::too_large(
        format!(
            "The archive would unpack to more than {} bytes",
            limits.bytes
        ),
        limits.bytes,
    )
}

/// The names on the path of an entry called `name`, or `None` when the path is absolute or
/// climbs out with `..`.
fn components(name: &str) -> Option<Vec<&str>> {
    let has_drive =
        name.as_bytes().get(1) == Some(&b':') && name.as_bytes()[0].is_ascii_alphabetic();
    let is_absolute = name.starts_with(['/', '\\']) || has_drive;
    if is_absolute || name.contains('\0') {
        return None;
    }
    let mut components = Vec::new();
    for component in name.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => return None,
            component => components.push(component),
        }
    }
    Some(components)
}

/// What macOS adds to the archives it makes for itself.
fn is_left_out(components: &[&str]) -> bool {
    components.contains(&"__MACOSX") || components.last() == Some(&".DS_Store")
}

/// Whether a link `depth` directories below the new directory that points at `target` points
/// inside it. Climbing out with `..` is allowed only at the start of `target`, through the
/// directories the link is in, which cannot be links themselves.
fn link_stays_inside(target: &str, depth: usize) -> bool {
    if target.is_empty() || target.starts_with('/') || target.contains('\0') {
        return false;
    }
    let mut climbed = 0;
    let mut has_descended = false;
    for component in target.split('/') {
        match component {
            "" | "." => {}
            ".." if has_descended => return false,
            ".." => climbed += 1,
            _ => has_descended = true,
        }
    }
    climbed <= depth
}

/// Writes the planned entries into `root` and answers with how many there were.
fn unpack(
    archive: &mut Archive,
    plan: &[Planned],
    root: &Path,
    limits: Limits,
    deadline: &Deadline,
) -> Result<u64, RpcError> {
    let mut written = 0;
    let mut unpacked_bytes: u64 = 0;
    let mut directory_modes = Vec::new();
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
    for planned in plan {
        deadline.check().map_err(|_| stopped())?;
        let Some((last, parents)) = planned.components.split_last() else {
            continue;
        };
        let mut parent = root.to_path_buf();
        for component in parents {
            parent.push(component);
            ensure_directory(&parent)?;
        }
        let target = parent.join(last);
        let shown = target.to_string_lossy().into_owned();
        let failed_at = |error: std::io::Error| {
            if error.kind() == ErrorKind::AlreadyExists {
                RpcError::failed(format!("The archive holds one entry twice: {shown}"))
            } else {
                failure(&error, &shown)
            }
        };
        match &planned.kind {
            PlannedKind::Directory => {
                ensure_directory(&target)?;
                if let Some(mode) = planned.mode {
                    directory_modes.push((target.clone(), mode & 0o777));
                }
            }
            PlannedKind::Link(link) => symlink(link, &target).map_err(failed_at)?,
            PlannedKind::File => {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .mode(0o666)
                    .open(&target)
                    .map_err(failed_at)?;
                let mut entry = archive.by_index(planned.index).map_err(damaged)?;
                loop {
                    deadline.check().map_err(|_| stopped())?;
                    let read = match entry.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => read,
                        Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                        Err(error) => return Err(damaged(error)),
                    };
                    unpacked_bytes += read as u64;
                    if unpacked_bytes > limits.bytes {
                        return Err(too_large(limits));
                    }
                    file.write_all(&buffer[..read]).map_err(failed_at)?;
                }
                if let Some(mode) = planned.mode {
                    file.set_permissions(Permissions::from_mode(mode & 0o777))
                        .map_err(failed_at)?;
                }
                drop(file);
                if let Some(seconds) = planned.modified.and_then(seconds_since_epoch) {
                    let _ = set_raw_times(&target, (seconds, 0), (seconds, 0));
                }
            }
        }
        written += 1;
    }
    // Last, so that a directory that may not be written to could still be filled.
    for (directory, mode) in directory_modes.iter().rev() {
        fs::set_permissions(directory, Permissions::from_mode(*mode))
            .map_err(|error| failure(&error, &directory.to_string_lossy()))?;
    }
    Ok(written)
}

/// Makes the directory at `path` unless it is there, and refuses to go through anything else
/// that is there, a link above all.
fn ensure_directory(path: &Path) -> Result<(), RpcError> {
    let shown = path.to_string_lossy();
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(RpcError::bad_request(format!(
            "The archive would write through a link or into a file: {shown}"
        ))),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| failure(&error, &shown))
        }
        Err(error) => Err(failure(&error, &shown)),
    }
}

/// A time zip keeps, which is in the computer's time zone, in seconds since the epoch.
fn seconds_since_epoch(time: DateTime) -> Option<i64> {
    let local = NaiveDate::from_ymd_opt(
        i32::from(time.year()),
        u32::from(time.month()),
        u32::from(time.day()),
    )?
    .and_hms_opt(
        u32::from(time.hour()),
        u32::from(time.minute()),
        u32::from(time.second()),
    )?;
    Some(Local.from_local_datetime(&local).earliest()?.timestamp())
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
