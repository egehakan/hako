//! The uploads area: a directory of the app's own with one directory per day, for files that are
//! handed to a program by their path, and the clean-up that keeps it from growing.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long a file stays in the uploads area.
pub const KEPT_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The name of the directory of the day it is now, by the computer's clock and time zone.
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// The directory of `day` under `area`, created with what is missing for the user only.
pub fn day_directory(area: &Path, day: &str) -> io::Result<PathBuf> {
    let directory = area.join(day);
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)?;
    Ok(directory)
}

/// Deletes the files in the day directories of `area` that were last changed more than
/// [`KEPT_FOR`] before `now`, and the day directories other than `today`'s that are left empty.
/// Nothing else is touched.
pub fn clean(area: &Path, now: SystemTime, today: &str) {
    let Ok(days) = fs::read_dir(area) else {
        return;
    };
    for day in days.flatten() {
        let is_day_directory = day.file_name().to_str().is_some_and(is_day_name)
            && day.file_type().is_ok_and(|file_type| file_type.is_dir());
        if is_day_directory {
            clean_day(&day.path(), now, day.file_name() != today);
        }
    }
}

fn clean_day(directory: &Path, now: SystemTime, remove_when_empty: bool) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let expired = metadata
            .modified()
            .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age > KEPT_FOR));
        if metadata.is_file() && expired {
            let _ = fs::remove_file(entry.path());
        }
    }
    if remove_when_empty {
        // Fails, as it should, when anything is left in the directory.
        let _ = fs::remove_dir(directory);
    }
}

/// Whether `name` has the form `YYYY-MM-DD`.
fn is_day_name(name: &str) -> bool {
    name.len() == 10
        && name.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

#[cfg(test)]
#[path = "uploads_area_tests.rs"]
mod tests;
