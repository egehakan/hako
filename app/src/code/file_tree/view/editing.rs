//! Module for utilities related to editing items in the file tree.

#[cfg(test)]
#[path = "editing_tests.rs"]
mod tests;

use std::cmp::Ordering;
use std::sync::Arc;

use repo_metadata::file_tree_store::{FileTreeDirectoryEntryState, FileTreeEntryState};
use repo_metadata::{FileMetadata, FileTreeEntry};
use warp_util::standardized_path::StandardizedPath;
use warpui::ViewContext;
use warpui::elements::MouseStateHandle;

use super::{FileTreeIdentifier, FileTreeItem, FileTreeView};
use crate::code::editor_management::TabOpenMode;
use crate::code::file_tree::FileTreeEvent;
use crate::code::file_tree::file_ops::{self, CreateError, NewItem};
use crate::code::file_tree::undo::{CreatedItem, MovedItem, Origin, Step};
use crate::code::file_tree::view::{PendingEdit, PendingEditKind};
use crate::send_telemetry_from_ctx;
use crate::server::telemetry::TelemetryEvent;
use crate::util::openable_file_type::EditorLayout;

/// Custom ordering function for items in the file tree.
///
/// Directories are ordered first, sorted by natural (numeric-aware) order.
/// Files are ordered second, sorted by natural (numeric-aware) order.
/// Within each group, dotfiles (entries starting with a dot) are ordered first.
pub(super) fn sort_entries_for_file_tree(
    entry_1: &StandardizedPath,
    entry_2: &StandardizedPath,
    entry_map: &FileTreeEntry,
) -> Ordering {
    use std::cmp::Ordering;

    // Entries missing from the map sort before present entries, and compare
    // equal to each other. Using the same `Ordering` on both sides would
    // violate antisymmetry and cause `sorted_by` to panic with
    // "user-provided comparison function does not correctly implement a total order".
    let (entry_1, entry_2) = match (entry_map.get(entry_1), entry_map.get(entry_2)) {
        (None, None) => return Ordering::Equal,
        (None, Some(_)) => return Ordering::Less,
        (Some(_), None) => return Ordering::Greater,
        (Some(e1), Some(e2)) => (e1, e2),
    };

    let is_dir_1 = matches!(entry_1, FileTreeEntryState::Directory(_));
    let is_dir_2 = matches!(entry_2, FileTreeEntryState::Directory(_));

    // Order directories before any files.
    match (is_dir_1, is_dir_2) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        // Both are same type, continue with alphabetical sort.
        _ => {}
    }

    // Same antisymmetry requirement for missing file names.
    let (name_1, name_2) = match (entry_1.path().file_name(), entry_2.path().file_name()) {
        (None, None) => return Ordering::Equal,
        (None, Some(_)) => return Ordering::Less,
        (Some(_), None) => return Ordering::Greater,
        (Some(n1), Some(n2)) => (n1, n2),
    };

    let starts_with_dot_1 = name_1.starts_with('.');
    let starts_with_dot_2 = name_2.starts_with('.');

    // Items starting with "." come first.
    match (starts_with_dot_1, starts_with_dot_2) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => alphanumeric_sort::compare_str(name_1, name_2),
    }
}

impl FileTreeView {
    /// Starts naming a new file or folder, per `kind`, in the folder at the given identifier, or
    /// next to the file at it.
    pub(super) fn start_creating(
        &mut self,
        id: &FileTreeIdentifier,
        kind: PendingEditKind,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.pending_edit.is_some() {
            self.cancel_pending_edit(ctx);
        }
        // A new item next to a file goes into the folder that holds the file.
        let folder_id = match self.item_at(id) {
            Some(FileTreeItem::DirectoryHeader { .. }) => Some(id.clone()),
            Some(FileTreeItem::File { metadata, .. }) => metadata
                .path
                .parent()
                .and_then(|parent| self.find_item_id(&id.root, &parent)),
            None => None,
        };
        let Some(id) = folder_id else {
            return;
        };
        let (path, depth) = match self.item_at(&id) {
            Some(FileTreeItem::DirectoryHeader {
                directory, depth, ..
            }) => (directory.path.clone(), *depth),
            Some(FileTreeItem::File { .. }) | None => return,
        };

        // Ensure the parent directory is expanded before creating an item beneath it.
        if !self.is_folder_expanded(&id.root, &path) {
            self.toggle_folder_expansion(&id.root, &path, ctx);
        }

        // Create a dummy FileTreeItem for the item we are about to create--we'll replace
        // this with something real once the user types in the actual name.
        let new_item_index = id.index + 1;
        let Some(placeholder) = Self::new_item_placeholder(kind, &path, depth + 1) else {
            return;
        };
        let Some(root_dir) = self.root_directories.get_mut(&id.root) else {
            return;
        };
        root_dir.items.insert(new_item_index, placeholder);

        // Ensure the new item we just created is selected.
        let new_id = FileTreeIdentifier {
            root: id.root.clone(),
            index: new_item_index,
        };
        self.select_id(&new_id, ctx);

        // Ensure the editor is focused.
        ctx.focus(&self.editor_view);
        self.pending_edit = Some(PendingEdit {
            id: new_id,
            kind,
            new_item_parent: Some((*path).clone()),
        });
    }

    /// The row that stands for a new item in `parent` while it is being named.
    fn new_item_placeholder(
        kind: PendingEditKind,
        parent: &StandardizedPath,
        depth: usize,
    ) -> Option<FileTreeItem> {
        match kind {
            PendingEditKind::CreateNewFile => Some(FileTreeItem::File {
                metadata: FileMetadata::from_standardized(parent.join("new_file"), false).into(),
                depth,
                mouse_state_handle: MouseStateHandle::default(),
                draggable_state: warpui::elements::DraggableState::default(),
            }),
            PendingEditKind::CreateNewFolder => Some(FileTreeItem::DirectoryHeader {
                directory: FileTreeDirectoryEntryState {
                    path: Arc::new(parent.join("new_folder")),
                    ignored: false,
                    loaded: true,
                },
                depth,
                mouse_state_handle: MouseStateHandle::default(),
                draggable_state: warpui::elements::DraggableState::default(),
            }),
            PendingEditKind::RenameExisting => None,
        }
    }

    /// Puts the row of the item being named back under its folder in `root`, whose rows were
    /// just rebuilt from the tree's contents, which the new item is not part of yet.
    pub(super) fn restore_new_item_placeholder(&mut self, root: &StandardizedPath) {
        let Some(pending_edit) = self.pending_edit.as_ref() else {
            return;
        };
        let (kind, Some(parent)) = (pending_edit.kind, pending_edit.new_item_parent.clone()) else {
            return;
        };
        if pending_edit.id.root != *root {
            return;
        }
        let Some(root_dir) = self.root_directories.get_mut(root) else {
            return;
        };
        let parent_row = root_dir
            .items
            .iter()
            .enumerate()
            .find(|(_, item)| *item.path() == parent)
            .map(|(index, item)| (index, item.depth()));
        let Some((parent_index, parent_depth)) = parent_row else {
            return;
        };
        let Some(placeholder) = Self::new_item_placeholder(kind, &parent, parent_depth + 1) else {
            return;
        };
        root_dir.items.insert(parent_index + 1, placeholder);
        let id = FileTreeIdentifier {
            root: root.clone(),
            index: parent_index + 1,
        };
        self.selected_item = Some(id.clone());
        if let Some(pending_edit) = self.pending_edit.as_mut() {
            pending_edit.id = id;
        }
    }

    /// Starts naming a new file, or with `is_folder` a new folder, in the folder the selection
    /// stands for: the selected folder, the folder holding the selected file, or the first root
    /// of the tree when nothing is selected.
    pub fn start_creating_in_selection(&mut self, is_folder: bool, ctx: &mut ViewContext<Self>) {
        let id = self
            .selected_item
            .clone()
            .filter(|id| !self.is_remote_item(id))
            .or_else(|| self.first_local_root_id());
        let Some(id) = id else {
            return;
        };
        let kind = if is_folder {
            PendingEditKind::CreateNewFolder
        } else {
            PendingEditKind::CreateNewFile
        };
        self.start_creating(&id, kind, ctx);
    }

    /// The header row of the first root of the tree that is on this machine.
    pub(super) fn first_local_root_id(&self) -> Option<FileTreeIdentifier> {
        self.displayed_root_directories()
            .find(|(_, root_dir)| !root_dir.is_remote() && !root_dir.items.is_empty())
            .map(|(root, _)| FileTreeIdentifier {
                root: root.clone(),
                index: 0,
            })
    }

    /// Starts a rename edit on the item at the given identifier.
    pub(super) fn start_rename(&mut self, id: &FileTreeIdentifier, ctx: &mut ViewContext<Self>) {
        let Some(root_dir) = self.root_directories.get(&id.root) else {
            return;
        };
        let Some(item) = root_dir.items.get(id.index) else {
            return;
        };
        // Prefill the editor with the current file or directory name.
        let current_name = item
            .path()
            .file_name()
            .map(|s| s.to_owned())
            .unwrap_or_default();

        self.pending_edit = Some(PendingEdit {
            id: id.clone(),
            kind: PendingEditKind::RenameExisting,
            new_item_parent: None,
        });

        self.editor_view.update(ctx, |view, ctx| {
            view.set_buffer_text(&current_name, ctx);
        });
        ctx.focus(&self.editor_view);
    }

    /// Commits a pending edit to the file tree. When the name typed for a new item is refused,
    /// the name field stays open for another try if `stays_open_if_refused` is set, and the edit
    /// is dropped otherwise.
    pub(super) fn commit_pending_edit(
        &mut self,
        stays_open_if_refused: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        // The answer to the name already committed is still on its way.
        if self.is_creating {
            return;
        }
        let Some(pending_edit) = self.pending_edit.take() else {
            return;
        };

        let file_tree_id = pending_edit.id.clone();

        let buffer_content = self.editor_view.as_ref(ctx).buffer_text(ctx);

        match pending_edit.kind {
            PendingEditKind::CreateNewFile | PendingEditKind::CreateNewFolder => {
                let Some(parent) = pending_edit.new_item_parent.clone() else {
                    return;
                };
                let parent = parent.to_local_path_lossy();
                let is_folder = pending_edit.kind == PendingEditKind::CreateNewFolder;

                // The name field keeps showing what was typed until the disk has answered.
                self.pending_edit = Some(pending_edit);
                self.is_creating = true;
                ctx.spawn(
                    async move {
                        let result = file_ops::create_new(&parent, &buffer_content, is_folder);
                        (buffer_content, result)
                    },
                    move |me, (name, result), ctx| {
                        me.finish_creating(name, result, is_folder, stays_open_if_refused, ctx);
                    },
                );
            }
            PendingEditKind::RenameExisting => {
                let Some(root_dir) = self.root_directories.get(&file_tree_id.root) else {
                    return;
                };
                let Some(item) = root_dir.items.get(file_tree_id.index) else {
                    return;
                };
                let old_std_path = item.path().clone();
                self.editor_view.update(ctx, |view, ctx| {
                    view.clear_buffer(ctx);
                });
                if buffer_content.is_empty() {
                    return;
                }
                let mut new_std_path = old_std_path.clone();
                new_std_path.set_file_name(&buffer_content);
                if new_std_path == old_std_path {
                    return;
                }

                let old_path = old_std_path.to_local_path_lossy();
                let new_path = new_std_path.to_local_path_lossy();
                // `rename` replaces a file that is already there. A name that only differs in
                // case from the old one is the same file on a case-insensitive volume.
                let renames_to_itself = old_std_path
                    .as_str()
                    .eq_ignore_ascii_case(new_std_path.as_str());
                if !renames_to_itself && std::fs::symlink_metadata(&new_path).is_ok() {
                    self.show_error(
                        format!("Couldn’t rename: “{buffer_content}” already exists"),
                        ctx,
                    );
                    return;
                }
                if let Err(e) = std::fs::rename(&old_path, &new_path) {
                    self.show_error(format!("Couldn’t rename to “{buffer_content}”: {e}"), ctx);
                    return;
                }

                // Update the in-memory model immediately so the UI reflects the change without delay.
                if let Some(root_dir) = self.root_directories.get_mut(&file_tree_id.root) {
                    root_dir.entry.rename_path(&old_std_path, &new_std_path);
                }
                self.paths_awaiting_removal.remove(&new_std_path);
                if self.cut_paths.remove(&old_std_path) {
                    self.cut_paths.clear();
                }

                // Emit event to notify workspace that a file was renamed
                ctx.emit(FileTreeEvent::FileRenamed {
                    old_path: old_path.clone(),
                    new_path: new_path.clone(),
                });

                // Rebuild and select the renamed item using its FileTreeIdentifier
                self.rebuild_flatten_items_impl(Some(&file_tree_id), None);
                self.remember(
                    Step::Moved(vec![MovedItem {
                        from: old_path,
                        to: new_path,
                    }]),
                    ctx,
                );
                ctx.notify();
            }
        }
    }

    /// Shows the file or folder the name field asked for, opens a new file in the editor and
    /// remembers the creation for undoing; or says why the name was refused.
    fn finish_creating(
        &mut self,
        name: String,
        result: Result<NewItem, CreateError>,
        is_folder: bool,
        stays_open_if_refused: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        self.is_creating = false;
        let new_item = match result {
            Ok(new_item) => new_item,
            Err(error) => {
                let message = match &error {
                    // These already say which name they are about, or that there is none.
                    CreateError::EmptyName
                    | CreateError::InvalidName(_)
                    | CreateError::AlreadyExists(_) => format!("Couldn’t create it: {error}"),
                    CreateError::Io(_) => format!("Couldn’t create “{}”: {error}", name.trim()),
                };
                self.show_error(message, ctx);
                if stays_open_if_refused && self.pending_edit.is_some() {
                    ctx.focus(&self.editor_view);
                } else {
                    self.cancel_pending_edit(ctx);
                }
                return;
            }
        };

        self.pending_edit = None;
        self.editor_view.update(ctx, |view, ctx| {
            view.clear_buffer(ctx);
        });
        send_telemetry_from_ctx!(TelemetryEvent::FileTreeItemCreated, ctx);

        let (Ok(path), Ok(created_root)) = (
            StandardizedPath::try_from_local(&new_item.path),
            StandardizedPath::try_from_local(&new_item.created_root),
        ) else {
            return;
        };
        if let Some(root) = self.find_deepest_root_for_file(&path)
            && let Some(root_dir) = self.root_directories.get_mut(&root)
        {
            // Put what was created into the tree without waiting for it to notice. A folder made
            // on the way to a nested path is loaded from disk when the tree expands down to it.
            let created = if created_root != path {
                FileTreeEntryState::Directory(FileTreeDirectoryEntryState {
                    path: Arc::new(created_root),
                    ignored: false,
                    loaded: false,
                })
            } else if is_folder {
                FileTreeEntryState::Directory(FileTreeDirectoryEntryState {
                    path: Arc::new(path.clone()),
                    ignored: false,
                    loaded: true,
                })
            } else {
                FileTreeEntryState::File(
                    FileMetadata::from_standardized(path.clone(), false).into(),
                )
            };
            Self::insert_entry(&mut root_dir.entry, created);
            if is_folder {
                root_dir.expanded_folders.insert(path.clone());
            }
        }

        // The name field had the focus. A new file hands it to its editor below.
        ctx.focus_self();
        self.reveal_new_items(vec![path], ctx);
        if !is_folder {
            self.open_file(
                &new_item.path,
                Some(EditorLayout::SplitPane),
                TabOpenMode::Keep,
                ctx,
            );
        }
        self.remember(
            Step::Created(vec![CreatedItem {
                path: new_item.created_root,
                origin: Origin::New {
                    path: new_item.path,
                    is_folder,
                },
                fingerprint: new_item.fingerprint,
            }]),
            ctx,
        );
    }

    /// Cancels a pending edit and discards any changes.
    pub(super) fn cancel_pending_edit(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(pending_edit) = self.pending_edit.take() {
            let id = &pending_edit.id;
            if self.selected_item.as_ref() == Some(id) {
                self.selected_item = None;
            }
            self.editor_view.update(ctx, |view, ctx| {
                view.clear_buffer(ctx);
            });
            // Only the flows that create an item have a placeholder to remove.
            let has_placeholder = match pending_edit.kind {
                PendingEditKind::CreateNewFile | PendingEditKind::CreateNewFolder => true,
                PendingEditKind::RenameExisting => false,
            };
            if has_placeholder
                && let Some(root_dir) = self.root_directories.get_mut(&id.root)
                && id.index < root_dir.items.len()
            {
                root_dir.items.remove(id.index);
            }
        }
        ctx.notify();
    }

    /// Inserts a new entry into the tree.
    fn insert_entry(root_entry: &mut FileTreeEntry, child_entry: FileTreeEntryState) {
        let Some(parent) = child_entry.path().parent() else {
            return;
        };

        root_entry.insert_child_state(&parent, child_entry);
    }

    pub(super) fn handle_pending_edit(&mut self, ctx: &mut ViewContext<Self>) {
        if self.pending_edit.is_none() {
            return;
        };

        let editor_contents = self.editor_view.as_ref(ctx).buffer_text(ctx);
        // If the editor is empty and the editor was dismissed, cancel the editor.
        // Otherwise commit the editor. This matches VSCode's behavior.
        if editor_contents.is_empty() {
            self.cancel_pending_edit(ctx);
        } else {
            self.commit_pending_edit(false, ctx);
        }
    }
}
