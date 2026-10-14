//! Folder downloads: a directory zipped into the app's archives area, from where it is
//! downloaded like any other file, and the clean-up that keeps that area from growing.

use std::fmt::Display;
use std::fs::{self, DirBuilder, File, Metadata, OpenOptions};
use std::io::{self, ErrorKind, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Datelike as _, Local, TimeZone as _, Timelike as _};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use crate::file_ops::{Deadline, remove_tree, stopped};
use crate::files::{checked, failure, last_segment};
use crate::protocol::{FsArchive, FsArchiveParams, RpcError};
use crate::upload_names::MAX_NAME_BYTES;

/// The directory of the data directory that holds the archives.
pub const ARCHIVES_AREA: &str = "archives";
/// How long an archive is kept after it was made.
pub const KEPT_FOR: Duration = Duration::from_secs(60 * 60);

/// Directories that hold only what can be fetched or built again, left out unless asked for.
const HEAVY_DIRECTORIES: &[&str] = &[
    ".git",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".turbo",
    ".gradle",
    "Pods",
    "DerivedData",
];
/// Files that mark a directory called `target` as a build's output.
const BUILD_OUTPUT_MARKERS: &[&str] = &["CACHEDIR.TAG", ".rustc_info.json"];

const COPY_BUFFER_BYTES: usize = 1024 * 1024;
/// How many names are tried for an archive's own directory before it is given up.
const DIRECTORY_ATTEMPTS: usize = 16;

static DIRECTORY_NAMES: AtomicU64 = AtomicU64::new(0);

/// Zips the directory at `params.path` into a new directory of `area`, as long as the archive
/// stays within `limit` bytes.
pub fn create(
    params: &FsArchiveParams,
    area: &Path,
    limit: u64,
    deadline: &Deadline,
) -> Result<FsArchive, RpcError> {
    let path = params.path.as_str();
    let directory = checked(path)?;
    let metadata = fs::metadata(directory).map_err(|error| failure(&error, path))?;
    if !metadata.is_dir() {
        return Err(RpcError::bad_request(format!(
            "Only a directory is archived; a file is downloaded as it is: {path}"
        )));
    }
    let entries = fs::read_dir(directory).map_err(|error| failure(&error, path))?;
    drop(entries);

    let name = match last_segment(path) {
        "/" => "Root",
        name => name,
    };
    let file_name = zip_name(name);
    let shown_area = area.to_string_lossy().into_owned();
    let own_directory = new_directory(area).map_err(|error| failure(&error, &shown_area))?;
    let archive = own_directory.join(&file_name);
    let shown = archive.to_string_lossy().into_owned();

    let made = write_archive(
        directory,
        name,
        &archive,
        params.skip_heavy.unwrap_or(true),
        limit,
        deadline,
    );
    let skipped = match made {
        Ok(skipped) => skipped,
        Err(error) => {
            let _ = remove_tree(&own_directory);
            return Err(error);
        }
    };
    let size = fs::metadata(&archive)
        .map_err(|error| failure(&error, &shown))?
        .len();
    if size > limit {
        let _ = remove_tree(&own_directory);
        return Err(too_large(limit));
    }
    Ok(FsArchive {
        path: shown,
        name: file_name,
        size,
        skipped,
    })
}

/// `name` with `.zip` after it, shortened to keep it within [`MAX_NAME_BYTES`].
fn zip_name(name: &str) -> String {
    let mut end = name.len().min(MAX_NAME_BYTES - ".zip".len());
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}.zip", &name[..end])
}

/// Makes a directory of its own in `area` for one archive, for the user only.
fn new_directory(area: &Path) -> io::Result<PathBuf> {
    DirBuilder::new().recursive(true).mode(0o700).create(area)?;
    for _ in 0..DIRECTORY_ATTEMPTS {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = area.join(format!(
            "{since_epoch:x}-{:x}",
            DIRECTORY_NAMES.fetch_add(1, Ordering::Relaxed)
        ));
        match DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::from(ErrorKind::AlreadyExists))
}

fn too_large(limit: u64) -> RpcError {
    RpcError::too_large(
        format!("The archive would be over the limit of {limit} bytes"),
        limit,
    )
}

fn zip_failure(error: impl Display) -> RpcError {
    RpcError::failed(format!("The archive could not be written: {error}"))
}

/// Writes the archive and answers with how many directories were left out.
fn write_archive(
    directory: &Path,
    name: &str,
    archive: &Path,
    skip_heavy: bool,
    limit: u64,
    deadline: &Deadline,
) -> Result<u64, RpcError> {
    let shown = archive.to_string_lossy();
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(archive)
        .map_err(|error| failure(&error, &shown))?;
    let probe = file.try_clone().map_err(|error| failure(&error, &shown))?;
    let mut work = Archiving {
        writer: ZipWriter::new(file),
        probe,
        skip_heavy,
        skipped: 0,
        limit,
        deadline: deadline.clone(),
        buffer: vec![0; COPY_BUFFER_BYTES],
    };
    let metadata = fs::metadata(directory).map_err(|error| failure(&error, &shown))?;
    work.writer
        .add_directory(
            format!("{name}/"),
            options(&metadata).compression_method(CompressionMethod::Stored),
        )
        .map_err(zip_failure)?;
    work.add_children(directory, &format!("{name}/"))?;
    work.writer.finish().map_err(zip_failure)?;
    Ok(work.skipped)
}

struct Archiving {
    writer: ZipWriter<File>,
    /// The archive's file, through which its size is watched while it is written.
    probe: File,
    skip_heavy: bool,
    skipped: u64,
    limit: u64,
    deadline: Deadline,
    buffer: Vec<u8>,
}

impl Archiving {
    /// Adds what is in `directory` under `prefix`, which ends with `/`. What cannot be read is
    /// left out.
    fn add_children(&mut self, directory: &Path, prefix: &str) -> Result<(), RpcError> {
        let Ok(entries) = fs::read_dir(directory) else {
            return Ok(());
        };
        let mut entries: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
            .collect();
        entries.sort();
        for (name, path) in entries {
            self.check()?;
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            let file_type = metadata.file_type();
            let entry_name = format!("{prefix}{name}");
            if file_type.is_symlink() {
                let Some(target) = fs::read_link(&path)
                    .ok()
                    .and_then(|target| target.to_str().map(str::to_owned))
                else {
                    continue;
                };
                self.writer
                    .add_symlink(entry_name, target, link_options(&metadata))
                    .map_err(zip_failure)?;
            } else if file_type.is_dir() {
                if self.skip_heavy && is_heavy(&name, &path) {
                    self.skipped += 1;
                    continue;
                }
                if fs::read_dir(&path).is_err() {
                    continue;
                }
                self.writer
                    .add_directory(
                        format!("{entry_name}/"),
                        options(&metadata).compression_method(CompressionMethod::Stored),
                    )
                    .map_err(zip_failure)?;
                self.add_children(&path, &format!("{entry_name}/"))?;
            } else if file_type.is_file() {
                self.add_file(&path, entry_name, &metadata)?;
            }
        }
        Ok(())
    }

    fn add_file(
        &mut self,
        path: &Path,
        entry_name: String,
        metadata: &Metadata,
    ) -> Result<(), RpcError> {
        let Ok(mut file) = File::open(path) else {
            return Ok(());
        };
        let options = options(metadata)
            .compression_method(CompressionMethod::Deflated)
            .large_file(metadata.len() >= u64::from(u32::MAX));
        self.writer
            .start_file(entry_name, options)
            .map_err(zip_failure)?;
        loop {
            self.check()?;
            let read = match file.read(&mut self.buffer) {
                Ok(0) => return Ok(()),
                Ok(read) => read,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    // A file that cannot be read to its end is left out like one that cannot
                    // be opened.
                    return self.writer.abort_file().map_err(zip_failure);
                }
            };
            self.writer
                .write_all(&self.buffer[..read])
                .map_err(zip_failure)?;
        }
    }

    /// Stops at the deadline, and when the archive has grown past the limit.
    fn check(&self) -> Result<(), RpcError> {
        self.deadline.check().map_err(|_| stopped())?;
        let size = self.probe.metadata().map_err(zip_failure)?.len();
        if size > self.limit {
            return Err(too_large(self.limit));
        }
        Ok(())
    }
}

/// Whether the directory at `path`, called `name`, holds only what can be fetched or built
/// again.
fn is_heavy(name: &str, path: &Path) -> bool {
    HEAVY_DIRECTORIES.contains(&name)
        || (name == "target"
            && BUILD_OUTPUT_MARKERS
                .iter()
                .any(|marker| fs::symlink_metadata(path.join(marker)).is_ok()))
}

fn options(metadata: &Metadata) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .unix_permissions(metadata.permissions().mode() & 0o777)
        .last_modified_time(zip_time(metadata))
}

fn link_options(metadata: &Metadata) -> SimpleFileOptions {
    SimpleFileOptions::default().last_modified_time(zip_time(metadata))
}

/// The modification time in the computer's time zone, as zip keeps it: to two seconds, and
/// from 1980 to 2107.
fn zip_time(metadata: &Metadata) -> DateTime {
    metadata
        .modified()
        .ok()
        .and_then(|modified| {
            let since_epoch = modified.duration_since(UNIX_EPOCH).ok()?;
            let local = Local
                .timestamp_opt(i64::try_from(since_epoch.as_secs()).ok()?, 0)
                .single()?;
            DateTime::from_date_and_time(
                u16::try_from(local.year()).ok()?,
                local.month() as u8,
                local.day() as u8,
                local.hour() as u8,
                local.minute() as u8,
                local.second() as u8,
            )
            .ok()
        })
        .unwrap_or_default()
}

/// Deletes the archives in `area` made more than `kept_for` before `now`, each with the
/// directory of its own. Nothing else is touched.
pub fn clean(area: &Path, now: SystemTime, kept_for: Duration) {
    let Ok(entries) = fs::read_dir(area) else {
        return;
    };
    for entry in entries.flatten() {
        let is_own_directory = entry.file_name().to_str().is_some_and(is_directory_name)
            && entry.file_type().is_ok_and(|file_type| file_type.is_dir());
        let is_expired = entry.metadata().is_ok_and(|metadata| {
            metadata.modified().is_ok_and(|modified| {
                now.duration_since(modified)
                    .is_ok_and(|age| age >= kept_for)
            })
        });
        if is_own_directory && is_expired {
            let _ = remove_tree(&entry.path());
        }
    }
}

/// Whether `name` has the form of the name of an archive's own directory.
fn is_directory_name(name: &str) -> bool {
    name.split_once('-').is_some_and(|(time, count)| {
        [time, count]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
    })
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
