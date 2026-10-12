//! Filesystem work behind the Project Explorer's copy, cut, paste, duplicate and drop. Everything
//! here blocks on the disk, so callers run it off the main thread.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::{fs, io};

use super::undo::Fingerprint;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PasteKind {
    Copy,
    Move,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum FileOpError {
    #[error("a folder can’t be copied into itself")]
    CopyIntoItself,
    #[error("a folder can’t be moved into itself")]
    MoveIntoItself,
    #[error("it no longer exists")]
    SourceMissing,
    #[error("it is neither a file, a folder nor a link")]
    UnsupportedFileType,
    #[error("{0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, thiserror::Error)]
pub(super) enum CreateError {
    #[error("a name is needed")]
    EmptyName,
    #[error("“{0}” can’t be used as a name")]
    InvalidName(String),
    #[error("“{0}” already exists")]
    AlreadyExists(String),
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// A file or folder the tree created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NewItem {
    pub path: PathBuf,
    /// The topmost thing that had to be created: `path` itself, or the first of the folders
    /// made on the way to it.
    pub created_root: PathBuf,
    /// What `created_root` looked like once made.
    pub fingerprint: Fingerprint,
}

/// Creates an empty file, or with `is_folder` a folder, called `name` in `parent`. A name with
/// slashes in it is a path below `parent`, whose missing folders are created too. Nothing is
/// created when the name is taken.
pub(super) fn create_new(
    parent: &Path,
    name: &str,
    is_folder: bool,
) -> Result<NewItem, CreateError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CreateError::EmptyName);
    }
    let relative = Path::new(name);
    let stays_below_parent = relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)));
    if !stays_below_parent || name.contains('\0') {
        return Err(CreateError::InvalidName(name.to_owned()));
    }
    let path = parent.join(relative);
    if is_taken(&path) {
        return Err(CreateError::AlreadyExists(name.to_owned()));
    }

    let mut created_root = parent.to_path_buf();
    for component in relative.components() {
        created_root.push(component);
        if !is_taken(&created_root) {
            break;
        }
    }

    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    let created = if is_folder {
        fs::create_dir(&path)
    } else {
        fs::File::create_new(&path).map(|_| ())
    };
    created.map_err(|error| match error.kind() {
        io::ErrorKind::AlreadyExists => CreateError::AlreadyExists(name.to_owned()),
        _ => CreateError::Io(error),
    })?;
    let fingerprint = Fingerprint::of(&created_root)?;
    Ok(NewItem {
        path,
        created_root,
        fingerprint,
    })
}

/// An item put in place by a paste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PlacedItem {
    pub source: PathBuf,
    pub destination: PathBuf,
    /// What a copy looked like once made. `None` for an item that was moved.
    pub fingerprint: Option<Fingerprint>,
}

/// An item a paste could not put in place.
#[derive(Debug)]
pub(super) struct FailedItem {
    pub source: PathBuf,
    pub error: FileOpError,
}

#[derive(Debug, Default)]
pub(super) struct PasteOutcome {
    pub placed: Vec<PlacedItem>,
    pub failed: Vec<FailedItem>,
}

/// Splits a file name into the part a copy suffix is appended to and the extension kept after it,
/// dot included. A folder name and a dotfile's name have no extension.
fn split_extension(name: &str, is_dir: bool) -> (&str, &str) {
    if is_dir {
        return (name, "");
    }
    let Some(dot) = name.rfind('.').filter(|dot| *dot > 0) else {
        return (name, "");
    };
    let (stem, extension) = name.split_at(dot);
    // `archive.tar.gz` copies to `archive copy.tar.gz`, as it does in Finder.
    match stem.strip_suffix(".tar").filter(|rest| !rest.is_empty()) {
        Some(rest) => name.split_at(rest.len()),
        None => (stem, extension),
    }
}

/// Splits the copy suffix off a stem: `report copy 3` is copy 3 of `report`, `report copy` is
/// copy 1, and anything else is copy 0 of itself.
fn split_copy_suffix(stem: &str) -> (&str, usize) {
    if let Some(base) = stem.strip_suffix(" copy").filter(|base| !base.is_empty()) {
        return (base, 1);
    }
    if let Some((rest, number)) = stem.rsplit_once(' ')
        && let Ok(number) = number.parse::<usize>()
        && number >= 2
        && let Some(base) = rest.strip_suffix(" copy").filter(|base| !base.is_empty())
    {
        return (base, number);
    }
    (stem, 0)
}

/// The name Finder gives the `copy_number`th copy of `base`: `base copy`, `base copy 2`, and so
/// on, ahead of `extension`.
fn copy_name(base: &str, extension: &str, copy_number: usize) -> String {
    if copy_number <= 1 {
        format!("{base} copy{extension}")
    } else {
        format!("{base} copy {copy_number}{extension}")
    }
}

/// Whether anything, a dangling link included, sits at `path`.
pub(super) fn is_taken(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Where an item called `name` goes in `target_dir` without replacing what is there: under its
/// own name when that is free, and otherwise under the first free copy name.
pub(super) fn free_destination(target_dir: &Path, name: &OsStr, is_dir: bool) -> PathBuf {
    let destination = target_dir.join(name);
    if !is_taken(&destination) {
        return destination;
    }
    let name = name.to_string_lossy();
    let (stem, extension) = split_extension(&name, is_dir);
    let (base, taken_number) = split_copy_suffix(stem);
    let mut copy_number = taken_number + 1;
    loop {
        let candidate = target_dir.join(copy_name(base, extension, copy_number));
        if !is_taken(&candidate) {
            return candidate;
        }
        copy_number += 1;
    }
}

/// Whether `path` is `ancestor` or lies somewhere beneath it.
pub(super) fn is_same_or_inside(path: &Path, ancestor: &Path) -> bool {
    path.starts_with(ancestor)
}

/// Drops the duplicates and the paths that lie inside another path of the list, which the
/// operation on that other path already covers.
pub(super) fn top_level_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut kept: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for path in &paths {
        let covered = paths
            .iter()
            .any(|other| other != path && is_same_or_inside(path, other));
        if !covered && !kept.contains(path) {
            kept.push(path.clone());
        }
    }
    kept
}

/// The folder a paste lands in: the selected folder, the folder holding the selected file, or
/// `root` when nothing is selected. `selected` is the selected path and whether it is a folder.
pub(super) fn paste_target_dir(selected: Option<(&Path, bool)>, root: &Path) -> PathBuf {
    match selected {
        Some((path, true)) => path.to_path_buf(),
        Some((path, false)) => path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.to_path_buf()),
        None => root.to_path_buf(),
    }
}

/// Resolves links in `path`, falling back to the path as given when it cannot be resolved.
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Copies the file, folder or link at `source` to `destination`, which must not exist. A link
/// is copied as a link rather than as what it points to, inside a copied folder too.
pub(super) fn copy_recursively(source: &Path, destination: &Path) -> Result<(), FileOpError> {
    let metadata = fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        copy_link(source, destination)?;
    } else if file_type.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_recursively(&entry.path(), &destination.join(entry.file_name()))?;
        }
        // Last, so that a read-only folder can still be filled.
        fs::set_permissions(destination, metadata.permissions())?;
    } else if file_type.is_file() {
        fs::copy(source, destination)?;
    } else {
        return Err(FileOpError::UnsupportedFileType);
    }
    Ok(())
}

#[cfg(unix)]
fn copy_link(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(source)?, destination)
}

#[cfg(windows)]
fn copy_link(source: &Path, destination: &Path) -> io::Result<()> {
    let target = fs::read_link(source)?;
    if fs::metadata(source).is_ok_and(|metadata| metadata.is_dir()) {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

#[cfg(not(any(unix, windows)))]
fn copy_link(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

fn remove_recursively(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Renames `source` to `destination` without replacing anything: where the system can do that in
/// one step it fails if `destination` has appeared since the caller looked.
#[cfg(target_os = "macos")]
fn rename_without_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    // SAFETY: both arguments are valid NUL-terminated strings that outlive the call.
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn rename_without_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    if is_taken(destination) {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    fs::rename(source, destination)
}

/// Moves `source` to `destination`, which must not exist, copying and then removing the
/// original when the two are on different volumes.
pub(super) fn move_item(source: &Path, destination: &Path) -> Result<(), FileOpError> {
    match rename_without_replacing(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            copy_recursively(source, destination)?;
            remove_recursively(source)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn paste_one(
    source: &Path,
    target_dir: &Path,
    kind: PasteKind,
) -> Result<(PathBuf, Option<Fingerprint>), FileOpError> {
    let metadata = fs::symlink_metadata(source).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => FileOpError::SourceMissing,
        _ => FileOpError::Io(error),
    })?;
    let is_dir = metadata.is_dir();
    if is_dir && is_same_or_inside(&canonical(target_dir), &canonical(source)) {
        return Err(match kind {
            PasteKind::Copy => FileOpError::CopyIntoItself,
            PasteKind::Move => FileOpError::MoveIntoItself,
        });
    }
    let Some(name) = source.file_name() else {
        return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
    };

    match kind {
        PasteKind::Copy => {
            let destination = free_destination(target_dir, name, is_dir);
            copy_recursively(source, &destination)?;
            let fingerprint = Fingerprint::of(&destination).ok();
            Ok((destination, fingerprint))
        }
        PasteKind::Move => {
            // An item already in the folder it is moved to stays as it is.
            if source
                .parent()
                .is_some_and(|parent| canonical(parent) == canonical(target_dir))
            {
                return Ok((source.to_path_buf(), None));
            }
            let destination = free_destination(target_dir, name, is_dir);
            move_item(source, &destination)?;
            Ok((destination, None))
        }
    }
}

/// Copies or moves `sources` into `target_dir`, never replacing what is there: an item whose
/// name is taken lands under a copy name. Each item succeeds or fails on its own.
pub(super) fn paste(sources: &[PathBuf], target_dir: &Path, kind: PasteKind) -> PasteOutcome {
    let mut outcome = PasteOutcome::default();
    for source in sources {
        match paste_one(source, target_dir, kind) {
            Ok((destination, fingerprint)) => outcome.placed.push(PlacedItem {
                source: source.clone(),
                destination,
                fingerprint,
            }),
            Err(error) => outcome.failed.push(FailedItem {
                source: source.clone(),
                error,
            }),
        }
    }
    outcome
}

/// Copies each of `sources` next to itself under a copy name.
pub(super) fn duplicate(sources: &[PathBuf]) -> PasteOutcome {
    let mut outcome = PasteOutcome::default();
    for source in sources {
        let Some(parent) = source.parent() else {
            continue;
        };
        let single = paste(std::slice::from_ref(source), parent, PasteKind::Copy);
        outcome.placed.extend(single.placed);
        outcome.failed.extend(single.failed);
    }
    outcome
}

/// One line per failed item for the message shown after `action` (for example "copy") went
/// wrong for some of them.
pub(super) fn failure_message(action: &str, failed: &[FailedItem]) -> String {
    let describe = |item: &FailedItem| {
        let name = item
            .source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| item.source.display().to_string());
        format!("“{name}”: {}", item.error)
    };
    match failed {
        [item] => format!("Couldn’t {action} {}", describe(item)),
        _ => {
            let details: Vec<String> = failed.iter().map(describe).collect();
            format!(
                "Couldn’t {action} {} items. {}",
                failed.len(),
                details.join("; ")
            )
        }
    }
}

#[cfg(test)]
#[path = "file_ops_tests.rs"]
mod tests;
