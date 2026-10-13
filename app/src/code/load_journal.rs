//! Remembers which files were being loaded into an editor when the app last stopped.
//!
//! Editor tabs are restored on launch, so a file whose load takes the app down would take it
//! down again on every launch after. The journal is what breaks that loop: a file is written
//! down before it is loaded and crossed out once it is on screen, and a file still written down
//! when the app starts is one the session restore leaves unloaded.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

const JOURNAL_FILE_NAME: &str = "editor_loads.json";

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Entries {
    /// Files whose load has started and not finished.
    #[serde(default)]
    loading: BTreeSet<PathBuf>,
    /// Files whose load an earlier run did not finish, until they are opened or closed by hand.
    #[serde(default)]
    interrupted: BTreeSet<PathBuf>,
}

pub struct LoadJournal {
    /// Where the journal is kept, or `None` for one that lasts only as long as the process.
    file: Option<PathBuf>,
    entries: Entries,
}

impl LoadJournal {
    /// Opens the journal kept at `file`. What the run before left as loading is from now on
    /// interrupted, and nothing is loading.
    pub fn open(file: Option<PathBuf>) -> Self {
        let mut entries = file
            .as_deref()
            .and_then(|file| fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice::<Entries>(&bytes).ok())
            .unwrap_or_default();
        let unfinished = std::mem::take(&mut entries.loading);
        let found_unfinished = !unfinished.is_empty();
        for path in &unfinished {
            log::warn!(
                "{} was being loaded into an editor when the app last stopped",
                path.display()
            );
        }
        entries.interrupted.extend(unfinished);

        let journal = Self { file, entries };
        if found_unfinished {
            journal.persist();
        }
        journal
    }

    /// Notes that `path` is about to be loaded. This is on disk before it returns.
    pub fn begin(&mut self, path: &Path) {
        if self.entries.loading.insert(path.to_path_buf()) {
            self.persist();
        }
    }

    /// Notes that the load of `path` ended, well or badly, with the app still running.
    pub fn finish(&mut self, path: &Path) {
        if self.entries.loading.remove(path) {
            self.persist();
        }
    }

    /// Notes that the app is stopping in an orderly way, which no load can have caused.
    pub fn finish_all(&mut self) {
        if !self.entries.loading.is_empty() {
            self.entries.loading.clear();
            self.persist();
        }
    }

    /// Whether an earlier run stopped while loading `path`.
    pub fn was_interrupted(&self, path: &Path) -> bool {
        self.entries.interrupted.contains(path)
    }

    /// Forgets that the load of `path` was interrupted, once the user has dealt with it.
    pub fn forget_interruption(&mut self, path: &Path) {
        if self.entries.interrupted.remove(path) {
            self.persist();
        }
    }

    /// Writes the journal where the next run finds it, through a rename so that it is never
    /// found half written. A journal that cannot be written is one the next run does not have,
    /// which costs the protection and nothing else.
    fn persist(&self) {
        let Some(file) = &self.file else {
            return;
        };
        let result = (|| -> std::io::Result<()> {
            if let Some(directory) = file.parent() {
                fs::create_dir_all(directory)?;
            }
            let temporary = file.with_extension("json.tmp");
            // Not synced: the journal has to outlive the process, which the page cache does, and
            // a sync would hold up every file that is opened by tens of milliseconds.
            fs::File::create(&temporary)?.write_all(&serde_json::to_vec(&self.entries)?)?;
            fs::rename(&temporary, file)
        })();
        if let Err(err) = result {
            log::warn!("Could not write {}: {err}", file.display());
        }
    }
}

/// The journal of this process. Tests get one that is not kept on disk.
static JOURNAL: LazyLock<Mutex<LoadJournal>> = LazyLock::new(|| {
    let directory = warp_core::paths::state_dir();
    let on_disk =
        !cfg!(test) && !cfg!(target_family = "wasm") && !directory.as_os_str().is_empty();
    let file = on_disk.then(|| directory.join(JOURNAL_FILE_NAME));
    Mutex::new(LoadJournal::open(file))
});

pub fn begin(path: &Path) {
    JOURNAL.lock().begin(path);
}

pub fn finish(path: &Path) {
    JOURNAL.lock().finish(path);
}

pub fn finish_all() {
    JOURNAL.lock().finish_all();
}

pub fn was_interrupted(path: &Path) -> bool {
    JOURNAL.lock().was_interrupted(path)
}

pub fn forget_interruption(path: &Path) {
    JOURNAL.lock().forget_interruption(path);
}

/// Makes `path` a file whose load the run before did not survive.
#[cfg(test)]
pub fn interrupt_for_test(path: &Path) {
    JOURNAL.lock().entries.interrupted.insert(path.to_path_buf());
}

#[cfg(test)]
#[path = "load_journal_tests.rs"]
mod tests;
