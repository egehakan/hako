//! Undoing and redoing what the Project Explorer did to files. Nothing here replaces or
//! permanently deletes anything: a step that cannot be reversed without doing so stops where it
//! is and says why. Everything blocks on the disk, so callers run it off the main thread.

use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{fs, io};

use super::file_ops::{self, is_taken};
use super::trash;

/// What a file, link or folder looked like when the tree created it, to tell whether it has been
/// touched since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Fingerprint {
    /// The item itself plus, for a folder, everything in it.
    entries: u64,
    /// Total size of the files counted in `entries`.
    bytes: u64,
    /// The latest modification time among `entries`.
    modified: Option<SystemTime>,
}

impl Fingerprint {
    pub(super) fn of(path: &Path) -> io::Result<Self> {
        let mut fingerprint = Self {
            entries: 0,
            bytes: 0,
            modified: None,
        };
        fingerprint.add(path)?;
        Ok(fingerprint)
    }

    fn add(&mut self, path: &Path) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        self.entries += 1;
        self.modified = self.modified.max(metadata.modified().ok());
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                self.add(&entry?.path())?;
            }
        } else {
            self.bytes += metadata.len();
        }
        Ok(())
    }
}

/// An item the tree moved to the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TrashedItem {
    pub original: PathBuf,
    /// Where the trash put the item, as the trash reported it.
    pub in_trash: PathBuf,
}

/// An item the tree moved or renamed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MovedItem {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// How the tree made an item, which is what making it again takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Origin {
    CopyOf(PathBuf),
    /// A new empty file or folder at `path`, which lies inside the created item when folders
    /// had to be made on the way to it.
    New {
        path: PathBuf,
        is_folder: bool,
    },
}

/// An item the tree created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CreatedItem {
    /// The topmost thing that was created.
    pub path: PathBuf,
    pub origin: Origin,
    pub fingerprint: Fingerprint,
}

impl CreatedItem {
    /// The path the user asked for, which names the item in messages.
    fn requested_path(&self) -> &Path {
        match &self.origin {
            Origin::CopyOf(_) => &self.path,
            Origin::New { path, .. } => path,
        }
    }
}

/// One undoable step: everything a single action of the tree did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Step {
    Trashed(Vec<TrashedItem>),
    Moved(Vec<MovedItem>),
    Created(Vec<CreatedItem>),
}

impl Step {
    pub(super) fn is_empty(&self) -> bool {
        match self {
            Step::Trashed(items) => items.is_empty(),
            Step::Moved(items) => items.is_empty(),
            Step::Created(items) => items.is_empty(),
        }
    }

    /// What the step did, or with `undone` what undoing it did, in a line for the user.
    pub(super) fn summary(&self, undone: bool) -> String {
        match self {
            Step::Trashed(items) => {
                let label = items_label(items.iter().map(|item| item.original.as_path()));
                if undone {
                    format!("Put {label} back")
                } else {
                    format!("Moved {label} to the Trash")
                }
            }
            Step::Moved(items) => {
                if let [item] = items.as_slice()
                    && item.from.parent() == item.to.parent()
                {
                    let (from, to) = (name_of(&item.from), name_of(&item.to));
                    return if undone {
                        format!("Renamed “{to}” back to “{from}”")
                    } else {
                        format!("Renamed “{from}” to “{to}”")
                    };
                }
                let label = items_label(items.iter().map(|item| item.to.as_path()));
                if undone {
                    format!("Moved {label} back")
                } else {
                    format!("Moved {label}")
                }
            }
            Step::Created(items) => {
                let label = items_label(items.iter().map(CreatedItem::requested_path));
                let copied = items.iter().all(|item| match item.origin {
                    Origin::CopyOf(_) => true,
                    Origin::New { .. } => false,
                });
                if undone {
                    format!("Moved {label} to the Trash")
                } else if copied {
                    format!("Copied {label}")
                } else {
                    format!("Created {label}")
                }
            }
        }
    }
}

/// The quoted name of a single item, or how many there are.
fn items_label<'a>(mut paths: impl ExactSizeIterator<Item = &'a Path>) -> String {
    match (paths.len(), paths.next()) {
        (1, Some(path)) => format!("“{}”", name_of(path)),
        (count, _) => format!("{count} items"),
    }
}

/// The result of undoing or redoing a step.
#[derive(Debug, Default)]
pub(super) struct Reversal {
    /// The items that were reversed, as the step the opposite action applies to.
    pub reversed: Option<Step>,
    /// The items left as they were, starting with the one that could not be reversed.
    pub remaining: Option<Step>,
    /// Why the items in `remaining` were left, in words for the user.
    pub problem: Option<String>,
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Applies `reverse` to `items` in order and stops at the first one it fails for.
fn reverse_each<T>(
    items: Vec<T>,
    mut reverse: impl FnMut(&mut T) -> Result<(), String>,
    into_step: impl Fn(Vec<T>) -> Step,
) -> Reversal {
    let mut reversed = Vec::with_capacity(items.len());
    let mut remaining = Vec::new();
    let mut problem = None;
    for mut item in items {
        if problem.is_some() {
            remaining.push(item);
            continue;
        }
        match reverse(&mut item) {
            Ok(()) => reversed.push(item),
            Err(reason) => {
                problem = Some(reason);
                remaining.push(item);
            }
        }
    }
    Reversal {
        reversed: (!reversed.is_empty()).then(|| into_step(reversed)),
        remaining: (!remaining.is_empty()).then(|| into_step(remaining)),
        problem,
    }
}

/// Moves `from` to `to` when `from` is still there and nothing has taken `to`.
fn move_if_free(from: &Path, to: &Path) -> Result<(), String> {
    if !is_taken(from) {
        return Err(format!("“{}” is no longer there", name_of(from)));
    }
    if is_taken(to) {
        return Err(format!(
            "“{}” can’t go back: something else is at {} now",
            name_of(from),
            to.display()
        ));
    }
    file_ops::move_item(from, to)
        .map_err(|error| format!("“{}” couldn’t be moved: {error}", name_of(from)))
}

fn put_back(item: &mut TrashedItem) -> Result<(), String> {
    let name = name_of(&item.original);
    if !is_taken(&item.in_trash) {
        return Err(format!("“{name}” is no longer in the Trash"));
    }
    if is_taken(&item.original) {
        return Err(format!(
            "“{name}” can’t be put back: something else is at {} now",
            item.original.display()
        ));
    }
    file_ops::move_item(&item.in_trash, &item.original)
        .map_err(|error| format!("“{name}” couldn’t be put back: {error}"))
}

fn trash_again(item: &mut TrashedItem) -> Result<(), String> {
    let name = name_of(&item.original);
    if !is_taken(&item.original) {
        return Err(format!("“{name}” is no longer there"));
    }
    item.in_trash = trash::move_to_trash(&item.original)
        .map_err(|error| format!("“{name}” couldn’t be moved to the Trash: {error}"))?;
    Ok(())
}

/// Trashes an item the tree created, unless it has been touched since.
fn trash_if_untouched(item: &mut CreatedItem) -> Result<(), String> {
    let name = name_of(item.requested_path());
    if !is_taken(&item.path) {
        return Err(format!("“{name}” is no longer there"));
    }
    let untouched = Fingerprint::of(&item.path).is_ok_and(|current| current == item.fingerprint);
    if !untouched {
        return Err(format!(
            "“{name}” has changed since it was created, so it was left in place"
        ));
    }
    trash::move_to_trash(&item.path)
        .map(|_| ())
        .map_err(|error| format!("“{name}” couldn’t be moved to the Trash: {error}"))
}

fn create_again(item: &mut CreatedItem) -> Result<(), String> {
    let name = name_of(item.requested_path());
    if is_taken(&item.path) {
        return Err(format!(
            "“{name}” can’t be created again: something else is at {} now",
            item.path.display()
        ));
    }
    let created = match &item.origin {
        Origin::CopyOf(source) => {
            if !is_taken(source) {
                return Err(format!(
                    "“{name}” can’t be copied again: “{}” is no longer there",
                    name_of(source)
                ));
            }
            file_ops::copy_recursively(source, &item.path).map_err(|error| error.to_string())
        }
        Origin::New { path, is_folder } => path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| {
                if *is_folder {
                    fs::create_dir(path)
                } else {
                    fs::File::create_new(path).map(|_| ())
                }
            })
            .map_err(|error| error.to_string()),
    };
    created.map_err(|error| format!("“{name}” couldn’t be created again: {error}"))?;
    item.fingerprint = Fingerprint::of(&item.path)
        .map_err(|error| format!("“{name}” couldn’t be read: {error}"))?;
    Ok(())
}

/// Takes back what `step` did: trashed items return to where they were, moved items move back,
/// and created items go to the trash as long as they are as the tree left them.
pub(super) fn undo(step: Step) -> Reversal {
    match step {
        Step::Trashed(items) => reverse_each(items, put_back, Step::Trashed),
        Step::Moved(items) => reverse_each(
            items,
            |item| move_if_free(&item.to, &item.from),
            Step::Moved,
        ),
        Step::Created(items) => reverse_each(items, trash_if_untouched, Step::Created),
    }
}

/// Does again what `step` did, after it was undone.
pub(super) fn redo(step: Step) -> Reversal {
    match step {
        Step::Trashed(items) => reverse_each(items, trash_again, Step::Trashed),
        Step::Moved(items) => reverse_each(
            items,
            |item| move_if_free(&item.from, &item.to),
            Step::Moved,
        ),
        Step::Created(items) => reverse_each(items, create_again, Step::Created),
    }
}

#[cfg(test)]
#[path = "undo_tests.rs"]
mod tests;
