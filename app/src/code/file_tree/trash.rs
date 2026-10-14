//! Moving files to the system trash. This blocks on the disk, so callers run it off the main
//! thread.

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum TrashError {
    #[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
    #[error("{0}")]
    Platform(String),
    #[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
    #[error("its path is not valid Unicode")]
    InvalidPath,
    #[cfg_attr(any(target_os = "macos", test), allow(dead_code))]
    #[error("moving items to the trash is not supported on this platform")]
    Unsupported,
}

/// Moves the file, folder or link at `path` to the trash and returns where the trash put it,
/// from where it can be put back. A link is trashed itself, not what it points to.
#[cfg(all(target_os = "macos", not(test)))]
pub(crate) fn move_to_trash(path: &Path) -> Result<PathBuf, TrashError> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};

    let path = path.to_str().ok_or(TrashError::InvalidPath)?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let mut trashed_url = None;
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, Some(&mut trashed_url))
        .map_err(|error| TrashError::Platform(error.localizedDescription().to_string()))?;
    trashed_url
        .and_then(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()))
        .ok_or_else(|| TrashError::Platform("the Trash did not say where it went".to_owned()))
}

/// There is no trash to move to on this platform; nothing is deleted.
#[cfg(all(not(target_os = "macos"), not(test)))]
pub(crate) fn move_to_trash(_path: &Path) -> Result<PathBuf, TrashError> {
    Err(TrashError::Unsupported)
}

/// Name of the folder that stands in for the trash in tests, next to the item trashed.
#[cfg(test)]
pub(super) const TEST_TRASH_DIR: &str = ".trash-for-tests";

/// Marks an item that cannot be trashed in tests when its name starts with it.
#[cfg(test)]
pub(super) const TEST_UNTRASHABLE_PREFIX: &str = "untrashable";

/// Keeps tests out of the real trash: the item moves into [`TEST_TRASH_DIR`] next to it, under
/// a name of its own when its name is taken there, as in the real trash.
#[cfg(test)]
pub(crate) fn move_to_trash(path: &Path) -> Result<PathBuf, TrashError> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(TrashError::InvalidPath);
    };
    if name.to_string_lossy().starts_with(TEST_UNTRASHABLE_PREFIX) {
        return Err(TrashError::Platform("it is in use".to_owned()));
    }
    let trash = parent.join(TEST_TRASH_DIR);
    std::fs::create_dir_all(&trash).map_err(|error| TrashError::Platform(error.to_string()))?;
    let in_trash = super::file_ops::free_destination(&trash, name, path.is_dir());
    std::fs::rename(path, &in_trash).map_err(|error| TrashError::Platform(error.to_string()))?;
    Ok(in_trash)
}
