//! Selecting several items of the file tree and copying, cutting, pasting, duplicating, trashing
//! and dropping them.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use instant::Instant;
use repo_metadata::file_tree_store::FileTreeEntryState;
use warp_util::standardized_path::StandardizedPath;
use warpui::clipboard::ClipboardContent;
use warpui::modals::{AlertDialogWithCallbacks, ModalButton};
use warpui::{SingletonEntity as _, ViewContext};

use super::{FileTreeIdentifier, FileTreeItem, FileTreeView, PendingFocusTarget};
use crate::code::file_tree::FileTreeEvent;
use crate::code::file_tree::file_ops::{self, PasteKind, PasteOutcome};
use crate::code::file_tree::trash::{self, TrashError};
use crate::code::file_tree::undo::{
    self, CreatedItem, MovedItem, Origin, Reversal, Step, TrashedItem,
};
use crate::view_components::DismissibleToast;
use crate::workspace::ToastStack;

/// Set by "Don't ask again" on the confirmation shown before items are trashed. It lasts until
/// the app quits: what it skips can be undone from the trash.
static SKIP_TRASH_CONFIRMATION: AtomicBool = AtomicBool::new(false);

/// How many file operations a tree remembers for undoing.
const UNDO_LIMIT: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Undo,
    Redo,
}

impl FileTreeView {
    pub(super) fn item_at(&self, id: &FileTreeIdentifier) -> Option<&FileTreeItem> {
        self.root_directories.get(&id.root)?.items.get(id.index)
    }

    pub(super) fn is_item_selected(&self, id: &FileTreeIdentifier, item: &FileTreeItem) -> bool {
        if self.multi_selection.is_empty() {
            self.selected_item.as_ref() == Some(id)
        } else {
            self.multi_selection.contains(item.path())
        }
    }

    /// The selected items, in the order the tree shows them.
    pub(super) fn selected_ids(&self) -> Vec<FileTreeIdentifier> {
        if self.multi_selection.is_empty() {
            return self.selected_item.iter().cloned().collect();
        }
        self.displayed_root_directories()
            .flat_map(|(root, root_dir)| {
                root_dir
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| self.multi_selection.contains(item.path()))
                    .map(|(index, _)| FileTreeIdentifier {
                        root: root.clone(),
                        index,
                    })
            })
            .collect()
    }

    /// The items an action invoked on the item at `id` applies to: every selected item when that
    /// item is one of several selected, and the item alone otherwise.
    pub(super) fn ids_acted_on_with(&self, id: &FileTreeIdentifier) -> Vec<FileTreeIdentifier> {
        let in_multi_selection = self
            .item_at(id)
            .is_some_and(|item| self.multi_selection.contains(item.path()));
        if in_multi_selection {
            self.selected_ids()
        } else {
            vec![id.clone()]
        }
    }

    fn id_for_path(&self, path: &StandardizedPath) -> Option<FileTreeIdentifier> {
        self.displayed_directories
            .iter()
            .find_map(|root| self.find_item_id(root, path))
    }

    /// Makes `paths` the selection. `primary` becomes the item keyboard navigation continues
    /// from; without one, the current such item is kept as long as it is still selected.
    fn set_selection(
        &mut self,
        paths: HashSet<StandardizedPath>,
        primary: Option<FileTreeIdentifier>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.pending_focus_target = None;
        let keeps_primary = self
            .selected_item_std_path()
            .is_some_and(|path| paths.contains(&path));
        if let Some(primary) = primary {
            self.selected_item = Some(primary);
        } else if !keeps_primary {
            self.selected_item = paths.iter().find_map(|path| self.id_for_path(path));
        }
        self.multi_selection = if paths.len() > 1 {
            paths
        } else {
            HashSet::new()
        };
        ctx.notify();
    }

    pub(super) fn toggle_item_selection(
        &mut self,
        id: &FileTreeIdentifier,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(path) = self.item_at(id).map(|item| item.path().clone()) else {
            return;
        };
        let mut selected = if self.multi_selection.is_empty() {
            self.selected_item_std_path().into_iter().collect()
        } else {
            self.multi_selection.clone()
        };
        let primary = if selected.remove(&path) {
            None
        } else {
            selected.insert(path.clone());
            self.selection_anchor = Some(path);
            Some(id.clone())
        };
        self.set_selection(selected, primary, ctx);
    }

    pub(super) fn extend_selection_to(
        &mut self,
        id: &FileTreeIdentifier,
        ctx: &mut ViewContext<Self>,
    ) {
        let anchor = self
            .selection_anchor
            .as_ref()
            .and_then(|path| self.id_for_path(path))
            .or_else(|| self.selected_item.clone())
            .unwrap_or_else(|| id.clone());
        let (Some(from), Some(to)) = (
            self.identifier_to_global_index(&anchor),
            self.identifier_to_global_index(id),
        ) else {
            return;
        };
        let paths = (from.min(to)..=from.max(to))
            .filter_map(|index| self.identifier_from_global_index(index))
            .filter_map(|id| self.item_at(&id).map(|item| item.path().clone()))
            .collect();
        self.selection_anchor = self.item_at(&anchor).map(|item| item.path().clone());
        self.set_selection(paths, Some(id.clone()), ctx);
    }

    /// The local paths an operation on the selection acts on, without those inside another
    /// selected folder, which the operation on that folder covers. The roots of the tree are left
    /// out unless `include_roots` is set, for the operations that would take a root away.
    fn selected_local_paths(&self, include_roots: bool) -> Vec<PathBuf> {
        let paths = self
            .selected_ids()
            .iter()
            .filter(|id| !self.is_remote_item(id) && (include_roots || id.index != 0))
            .filter_map(|id| self.item_at(id))
            .map(|item| item.path().to_local_path_lossy())
            .collect();
        file_ops::top_level_paths(paths)
    }

    /// The folder an item dropped or pasted on the item at `id` lands in: that item when it is a
    /// folder, and the folder holding it otherwise. Without an item, it is the first local root
    /// of the tree.
    pub(super) fn folder_for_drop_on(
        &self,
        id: Option<&FileTreeIdentifier>,
    ) -> Option<StandardizedPath> {
        let Some(id) = id else {
            return self
                .displayed_root_directories()
                .find(|(_, root_dir)| !root_dir.is_remote())
                .map(|(root, _)| root.clone());
        };
        if self.is_remote_item(id) {
            return None;
        }
        let item = self.item_at(id)?;
        let is_dir = matches!(item, FileTreeItem::DirectoryHeader { .. });
        let folder = file_ops::paste_target_dir(
            Some((&item.path().to_local_path_lossy(), is_dir)),
            &id.root.to_local_path_lossy(),
        );
        StandardizedPath::try_from_local(&folder).ok()
    }

    /// Puts the selected items on the pasteboard as files. With `cut`, pasting them in the tree
    /// moves them.
    pub(super) fn copy_selection(&mut self, cut: bool, ctx: &mut ViewContext<Self>) {
        let paths = self.selected_local_paths(!cut);
        if paths.is_empty() {
            return;
        }
        self.cut_paths = if cut {
            paths
                .iter()
                .filter_map(|path| StandardizedPath::try_from_local(path).ok())
                .collect()
        } else {
            HashSet::new()
        };
        let paths: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        ctx.clipboard().write(ClipboardContent {
            plain_text: paths.join("\n"),
            paths: Some(paths),
            ..Default::default()
        });
        ctx.notify();
    }

    /// Pastes the files on the pasteboard into the folder the selection stands for. They are
    /// moved when they are the items last cut in this tree, and copied otherwise, which covers
    /// files copied in another app.
    pub(super) fn paste_from_clipboard(&mut self, ctx: &mut ViewContext<Self>) {
        let sources: Vec<PathBuf> = ctx
            .clipboard()
            .read()
            .paths
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();
        if sources.is_empty() {
            return;
        }
        let Some(target) = self.folder_for_drop_on(self.selected_item.as_ref()) else {
            return;
        };

        let pasted: HashSet<StandardizedPath> = sources
            .iter()
            .filter_map(|path| StandardizedPath::try_from_local(path).ok())
            .collect();
        let kind = if !self.cut_paths.is_empty() && pasted == self.cut_paths {
            PasteKind::Move
        } else {
            PasteKind::Copy
        };
        // A cut is good for one paste, and is over once something else has been copied.
        self.cut_paths.clear();
        self.start_paste(sources, target, kind, ctx);
    }

    pub(super) fn duplicate_selection(&mut self, ctx: &mut ViewContext<Self>) {
        let sources = self.selected_local_paths(false);
        if sources.is_empty() {
            return;
        }
        ctx.spawn(
            async move { file_ops::duplicate(&sources) },
            |me, outcome, ctx| me.finish_paste("duplicate", PasteKind::Copy, outcome, ctx),
        );
    }

    /// Moves the item dragged from `dragged` into the folder that `target` stands for, along with
    /// the rest of the selection when the dragged item is part of it.
    pub(super) fn drop_items_on(
        &mut self,
        dragged: &FileTreeIdentifier,
        target: &FileTreeIdentifier,
        ctx: &mut ViewContext<Self>,
    ) {
        self.drop_folder = None;
        ctx.notify();
        if dragged == target || self.is_remote_item(dragged) {
            return;
        }
        let Some(dragged_path) = self.item_at(dragged).map(|item| item.path().clone()) else {
            return;
        };
        let sources = if self.multi_selection.contains(&dragged_path) {
            self.selected_local_paths(false)
        } else if dragged.index == 0 {
            Vec::new()
        } else {
            vec![dragged_path.to_local_path_lossy()]
        };
        let Some(folder) = self.folder_for_drop_on(Some(target)) else {
            return;
        };
        self.start_paste(sources, folder, PasteKind::Move, ctx);
    }

    /// Copies files dragged in from another app into the folder that `target` stands for.
    pub(super) fn drop_external_files(
        &mut self,
        paths: &[String],
        target: Option<&FileTreeIdentifier>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.drop_folder = None;
        ctx.notify();
        let Some(folder) = self.folder_for_drop_on(target) else {
            return;
        };
        let sources = paths.iter().map(PathBuf::from).collect();
        self.start_paste(sources, folder, PasteKind::Copy, ctx);
    }

    fn start_paste(
        &mut self,
        sources: Vec<PathBuf>,
        folder: StandardizedPath,
        kind: PasteKind,
        ctx: &mut ViewContext<Self>,
    ) {
        if sources.is_empty() {
            return;
        }
        let folder = folder.to_local_path_lossy();
        let action = match kind {
            PasteKind::Copy => "copy",
            PasteKind::Move => "move",
        };
        ctx.spawn(
            async move { file_ops::paste(&sources, &folder, kind) },
            move |me, outcome, ctx| me.finish_paste(action, kind, outcome, ctx),
        );
    }

    /// Reports the items that could not be put in place, lets the editors of moved files follow
    /// them, selects the items put in place and remembers the operation for undoing. `action`
    /// names what was attempted in the message about the ones that failed.
    fn finish_paste(
        &mut self,
        action: &str,
        kind: PasteKind,
        outcome: PasteOutcome,
        ctx: &mut ViewContext<Self>,
    ) {
        if !outcome.failed.is_empty() {
            self.show_error(file_ops::failure_message(action, &outcome.failed), ctx);
        }

        let step = match kind {
            PasteKind::Move => Step::Moved(
                outcome
                    .placed
                    .into_iter()
                    // An item moved into the folder it was already in went nowhere.
                    .filter(|item| item.source != item.destination)
                    .map(|item| MovedItem {
                        from: item.source,
                        to: item.destination,
                    })
                    .collect(),
            ),
            PasteKind::Copy => Step::Created(
                outcome
                    .placed
                    .into_iter()
                    .filter_map(|item| {
                        Some(CreatedItem {
                            path: item.destination,
                            origin: Origin::CopyOf(item.source),
                            fingerprint: item.fingerprint?,
                        })
                    })
                    .collect(),
            ),
        };
        self.show_step_in_tree(&step, Direction::Redo, ctx);
        self.remember(step, ctx);
    }

    /// Remembers a file operation the tree just did so that it can be undone, and says what it
    /// did. Doing something new forgets what had been undone.
    pub(super) fn remember(&mut self, step: Step, ctx: &mut ViewContext<Self>) {
        if step.is_empty() {
            return;
        }
        self.redo_stack.clear();
        self.show_done(step.summary(false), true, ctx);
        Self::push_bounded(&mut self.undo_stack, step);
    }

    fn push_bounded(stack: &mut Vec<Step>, step: Step) {
        stack.push(step);
        if stack.len() > UNDO_LIMIT {
            stack.remove(0);
        }
    }

    /// Takes back the last file operation done from the tree.
    pub fn undo(&mut self, ctx: &mut ViewContext<Self>) {
        self.reverse(Direction::Undo, ctx);
    }

    /// Does again the file operation last undone.
    pub(super) fn redo(&mut self, ctx: &mut ViewContext<Self>) {
        self.reverse(Direction::Redo, ctx);
    }

    fn reverse(&mut self, direction: Direction, ctx: &mut ViewContext<Self>) {
        if self.is_reversing {
            return;
        }
        let stack = match direction {
            Direction::Undo => &mut self.undo_stack,
            Direction::Redo => &mut self.redo_stack,
        };
        let Some(step) = stack.pop() else {
            return;
        };
        self.is_reversing = true;
        ctx.spawn(
            async move {
                match direction {
                    Direction::Undo => undo::undo(step),
                    Direction::Redo => undo::redo(step),
                }
            },
            move |me, reversal, ctx| me.finish_reversal(direction, reversal, ctx),
        );
    }

    fn finish_reversal(
        &mut self,
        direction: Direction,
        reversal: Reversal,
        ctx: &mut ViewContext<Self>,
    ) {
        self.is_reversing = false;
        let Reversal {
            reversed,
            remaining,
            problem,
        } = reversal;

        // What could not be reversed can be tried again once what was in the way is gone.
        if let Some(remaining) = remaining {
            match direction {
                Direction::Undo => self.undo_stack.push(remaining),
                Direction::Redo => self.redo_stack.push(remaining),
            }
        }
        if let Some(problem) = problem {
            let action = match direction {
                Direction::Undo => "undo",
                Direction::Redo => "redo",
            };
            self.show_error(format!("Couldn’t {action}: {problem}"), ctx);
        }
        let Some(step) = reversed else {
            return;
        };

        self.show_step_in_tree(&step, direction, ctx);
        match direction {
            Direction::Undo => {
                self.show_done(step.summary(true), false, ctx);
                Self::push_bounded(&mut self.redo_stack, step);
            }
            Direction::Redo => {
                self.show_done(step.summary(false), true, ctx);
                Self::push_bounded(&mut self.undo_stack, step);
            }
        }
    }

    /// Brings the tree and the open editors in line with `step`, which was just done or done
    /// again (`Direction::Redo`) or undone (`Direction::Undo`), without waiting for the tree to
    /// notice the change on disk.
    fn show_step_in_tree(
        &mut self,
        step: &Step,
        direction: Direction,
        ctx: &mut ViewContext<Self>,
    ) {
        let mut appeared = Vec::new();
        let mut gone = Vec::new();
        match (step, direction) {
            (Step::Trashed(items), Direction::Undo) => {
                appeared.extend(items.iter().map(|item| item.original.clone()));
            }
            (Step::Trashed(items), Direction::Redo) => {
                for item in items {
                    gone.push(item.original.clone());
                    ctx.emit(FileTreeEvent::FileDeleted {
                        path: item.original.clone(),
                    });
                }
            }
            (Step::Moved(items), _) => {
                for item in items {
                    let (old_path, new_path) = match direction {
                        Direction::Undo => (&item.to, &item.from),
                        Direction::Redo => (&item.from, &item.to),
                    };
                    gone.push(old_path.clone());
                    appeared.push(new_path.clone());
                    ctx.emit(FileTreeEvent::FileRenamed {
                        old_path: old_path.clone(),
                        new_path: new_path.clone(),
                    });
                }
            }
            (Step::Created(items), Direction::Undo) => {
                for item in items {
                    gone.push(item.path.clone());
                    ctx.emit(FileTreeEvent::FileDeleted {
                        path: item.path.clone(),
                    });
                }
            }
            (Step::Created(items), Direction::Redo) => {
                appeared.extend(items.iter().map(|item| item.path.clone()));
            }
        }

        for path in gone {
            if let Ok(path) = StandardizedPath::try_from_local(&path) {
                // Nothing is left to select once the tree catches up.
                if self
                    .pending_focus_target
                    .as_ref()
                    .is_some_and(|target| target.path.starts_with(&path))
                {
                    self.pending_focus_target = None;
                }
                self.multi_selection.remove(&path);
                self.cut_paths.retain(|cut| !cut.starts_with(&path));
                self.paths_awaiting_removal.insert(path, Instant::now());
            }
        }
        let appeared = appeared
            .iter()
            .filter_map(|path| StandardizedPath::try_from_local(path).ok())
            .collect();
        self.reveal_new_items(appeared, ctx);
    }

    /// Expands the tree down to `paths` and selects them. The ones the tree does not show yet
    /// are selected once it has caught up with the disk.
    pub(super) fn reveal_new_items(
        &mut self,
        paths: Vec<StandardizedPath>,
        ctx: &mut ViewContext<Self>,
    ) {
        for path in &paths {
            self.paths_awaiting_removal.remove(path);
        }
        let root = paths
            .first()
            .and_then(|first| self.find_deepest_root_for_file(first));
        let (Some(first), Some(root)) = (paths.first().cloned(), root) else {
            self.rebuild_flattened_items();
            ctx.notify();
            return;
        };

        // Putting something into a folder opens it, even one collapsed by hand.
        if let Some(collapsed) = self.explicitly_collapsed.get_mut(&root) {
            collapsed.retain(|folder| !first.starts_with(folder));
        }
        self.expand_ancestors_to_path(&root, &first, ctx);
        self.rebuild_flattened_items();

        self.selection_anchor = Some(first.clone());
        self.multi_selection = if paths.len() > 1 {
            paths.into_iter().collect()
        } else {
            HashSet::new()
        };
        self.pending_focus_target = Some(PendingFocusTarget {
            root,
            path: first,
            scrolled: false,
            expands_ancestors: true,
        });
        self.apply_pending_focus_target();
        ctx.notify();
    }

    /// Whether the folder at `path` is known to hold nothing. A folder whose contents the tree
    /// has not loaded is not.
    fn is_known_empty_folder(&self, path: &StandardizedPath) -> bool {
        self.root_directories.values().any(|root_dir| {
            matches!(
                root_dir.entry.get(path),
                Some(FileTreeEntryState::Directory(directory)) if directory.loaded
            ) && root_dir.entry.child_paths(path).next().is_none()
        })
    }

    /// Whether trashing `ids` is confirmed first: it is for several items and for a folder that
    /// may hold something.
    fn needs_trash_confirmation(&self, ids: &[FileTreeIdentifier]) -> bool {
        match ids {
            [] => false,
            [id] => self.item_at(id).is_some_and(|item| match item {
                FileTreeItem::File { .. } => false,
                FileTreeItem::DirectoryHeader { directory, .. } => {
                    !self.is_known_empty_folder(&directory.path)
                }
            }),
            _ => true,
        }
    }

    /// Moves the selected items to the trash, after asking when that takes more than one file or
    /// empty folder with it.
    pub(super) fn delete_selection(&mut self, ctx: &mut ViewContext<Self>) {
        let ids: Vec<FileTreeIdentifier> = self
            .selected_ids()
            .into_iter()
            .filter(|id| !self.is_remote_item(id) && id.index != 0)
            .collect();
        let paths = self.selected_local_paths(false);
        if paths.is_empty() {
            return;
        }
        if !self.needs_trash_confirmation(&ids) || SKIP_TRASH_CONFIRMATION.load(Ordering::Relaxed) {
            self.trash_paths(paths, ctx);
            return;
        }

        let message = match paths.as_slice() {
            [path] => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("Move “{name}” and its contents to the Trash?")
            }
            _ => format!("Move {} items to the Trash?", paths.len()),
        };
        ctx.show_native_platform_modal(AlertDialogWithCallbacks::for_view(
            message,
            "You can put them back from the Trash.",
            vec![
                ModalButton::for_view("Move to Trash", move |me: &mut Self, ctx| {
                    me.trash_paths(paths, ctx);
                }),
                ModalButton::for_view("Cancel", |_, _| {}),
            ],
            |_, _| SKIP_TRASH_CONFIRMATION.store(true, Ordering::Relaxed),
        ));
    }

    /// Takes `paths` out of the tree at once and moves them to the trash in the background. The
    /// ones that could not be trashed are put back.
    fn trash_paths(&mut self, paths: Vec<PathBuf>, ctx: &mut ViewContext<Self>) {
        let started = Instant::now();
        self.paths_being_trashed.extend(
            paths
                .iter()
                .filter_map(|path| StandardizedPath::try_from_local(path).ok()),
        );
        self.multi_selection.clear();
        self.rebuild_flattened_items();
        // The selection went with the items; keyboard navigation continues from their folder.
        if self.selected_item.is_none() {
            self.selected_item = paths
                .first()
                .and_then(|path| path.parent())
                .and_then(|parent| StandardizedPath::try_from_local(parent).ok())
                .and_then(|parent| self.id_for_path(&parent));
        }
        ctx.notify();
        let hidden_after = started.elapsed();

        ctx.spawn(
            async move {
                paths
                    .into_iter()
                    .map(|path| {
                        let result = trash::move_to_trash(&path);
                        (path, result)
                    })
                    .collect::<Vec<_>>()
            },
            move |me, results, ctx| {
                log::info!(
                    "Moved {} item(s) to the trash in {:?}; they left the tree after {hidden_after:?}",
                    results.len(),
                    started.elapsed(),
                );
                me.finish_trash(results, ctx);
            },
        );
    }

    fn finish_trash(
        &mut self,
        results: Vec<(PathBuf, Result<PathBuf, TrashError>)>,
        ctx: &mut ViewContext<Self>,
    ) {
        let mut trashed = Vec::new();
        let mut failures = Vec::new();
        for (path, result) in results {
            if let Ok(std_path) = StandardizedPath::try_from_local(&path) {
                self.paths_being_trashed.remove(&std_path);
            }
            match result {
                Ok(in_trash) => trashed.push(TrashedItem {
                    original: path,
                    in_trash,
                }),
                Err(error) => {
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    failures.push(format!("“{name}”: {error}"));
                }
            }
        }
        if !failures.is_empty() {
            self.show_error(
                format!("Couldn’t move to the Trash {}", failures.join("; ")),
                ctx,
            );
        }
        let step = Step::Trashed(trashed);
        self.show_step_in_tree(&step, Direction::Redo, ctx);
        self.remember(step, ctx);
    }

    /// Says that a file operation went through, with a button to undo it when `can_undo` is set.
    fn show_done(&self, message: String, can_undo: bool, ctx: &mut ViewContext<Self>) {
        let window_id = ctx.window_id();
        let mut toast = DismissibleToast::default(message)
            .with_object_id(format!("file_tree_operation_{}", ctx.view_id()));
        if can_undo {
            toast = toast.with_action_button(self.undo_button.clone());
        }
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(toast, window_id, ctx);
        });
    }

    pub(super) fn show_error(&self, message: String, ctx: &mut ViewContext<Self>) {
        log::warn!("{message}");
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(DismissibleToast::error(message), window_id, ctx);
        });
    }
}

#[cfg(test)]
#[path = "operations_tests.rs"]
mod tests;
