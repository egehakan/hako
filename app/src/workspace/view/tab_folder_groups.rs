use std::collections::HashSet;
use std::path::{Path, PathBuf};

use pathfinder_color::ColorU;
use warp_core::features::FeatureFlag;
use warpui::platform::FilePickerConfiguration;
use warpui::{SingletonEntity, ViewContext};

use super::{NewTerminalOptions, PanesLayout, Workspace, group_member_indices};
use crate::app_state::{LeafContents, PaneNodeSnapshot, TabSnapshot};
use crate::tab::{SelectedTabColor, uses_vertical_tabs};
use crate::view_components::DismissibleToast;
use crate::workspace::tab_folders::{
    TabFolder, TabFolders, TabGroupIcon, TabListItem, folder_directory_for,
    insertion_index_for_empty_group, normalized_directory, normalized_group_order, tab_list_items,
};
use crate::workspace::tab_group::TabGroupId;
use crate::workspace::tab_group_customize_dialog::TabGroupCustomizeEvent;
use crate::workspace::util::TabMovement;

/// The directory of the first terminal in a saved tab, which is where the tab is considered to
/// live when it has to be put in a folder after the fact.
fn saved_tab_directory(tab: &TabSnapshot) -> Option<PathBuf> {
    fn first_terminal_cwd(node: &PaneNodeSnapshot) -> Option<&str> {
        match node {
            PaneNodeSnapshot::Leaf(leaf) => match &leaf.contents {
                LeafContents::Terminal(terminal) => terminal.cwd.as_deref(),
                _ => None,
            },
            PaneNodeSnapshot::Branch(branch) => branch
                .children
                .iter()
                .find_map(|(_, child)| first_terminal_cwd(child)),
        }
    }
    first_terminal_cwd(&tab.root)
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
}

/// Picks the folder a terminal starting in `start` belongs to among `folders`: the deepest one
/// containing `start` that is not above the repository (or directory) `start` resolves to.
/// Returns the directory to make a folder for when none qualifies.
pub(super) fn resolve_folder_for_start_directory<'a>(
    start: &Path,
    folders: impl Iterator<Item = (TabGroupId, &'a Path)>,
) -> Result<TabGroupId, PathBuf> {
    let start = normalized_directory(start);
    let target = folder_directory_for(&start);
    folders
        .filter(|(_, directory)| start.starts_with(directory) && directory.starts_with(&target))
        .max_by_key(|(_, directory)| directory.components().count())
        .map(|(id, _)| id)
        .ok_or(target)
}

impl Workspace {
    /// Whether tabs are filed under folders automatically. Folders are a concept of the tab
    /// sidebar, so tabs in the horizontal tab bar are left alone.
    fn files_tabs_under_folders(&self, ctx: &ViewContext<Self>) -> bool {
        FeatureFlag::GroupedTabs.is_enabled()
            && uses_vertical_tabs(ctx)
            && !self.is_tab_drag_preview
            && !self.suppress_folder_assignment
    }

    /// Every group of this window in the order the tab list shows them.
    pub(crate) fn ordered_tab_group_ids(&self) -> Vec<TabGroupId> {
        let groups_in_tab_order: Vec<TabGroupId> =
            self.tabs.iter().filter_map(|tab| tab.group_id).collect();
        let existing: HashSet<TabGroupId> = self.tab_groups.keys().copied().collect();
        normalized_group_order(&self.tab_group_order, &groups_in_tab_order, &existing)
    }

    /// The tab list as the sidebar lays it out, including groups without tabs.
    pub(crate) fn tab_list_items(&self) -> Vec<TabListItem> {
        let tab_groups: Vec<Option<TabGroupId>> = self
            .tabs
            .iter()
            .map(|tab| tab.group_id.filter(|id| self.tab_groups.contains_key(id)))
            .collect();
        tab_list_items(&tab_groups, &self.ordered_tab_group_ids())
    }

    /// Records the current display order. A group that loses its last tab keeps the slot it has
    /// in the recorded order, so this has to run before tabs are moved or removed.
    pub(super) fn store_tab_group_order(&mut self) {
        self.tab_group_order = self.ordered_tab_group_ids();
    }

    fn folder_group_id(&self, directory: &Path) -> Option<TabGroupId> {
        self.tab_groups
            .values()
            .find(|group| group.directory.as_deref() == Some(directory))
            .map(|group| group.id)
    }

    /// The tab index at which a tab joins the end of `group_id` without moving the group.
    pub(super) fn group_append_index(&self, group_id: TabGroupId) -> usize {
        if let Some(index) = self.index_after_group(group_id) {
            return index;
        }
        let tab_groups: Vec<Option<TabGroupId>> =
            self.tabs.iter().map(|tab| tab.group_id).collect();
        let index =
            insertion_index_for_empty_group(group_id, &tab_groups, &self.ordered_tab_group_ids());
        if self.tab_groups.get(&group_id).is_some_and(|g| g.pinned) {
            index.min(self.pinned_boundary_index(&self.tabs))
        } else {
            self.clamp_to_unpinned_region(&self.tabs, index)
        }
    }

    /// Returns this window's group for the folder at `directory`, adding the folder if needed.
    pub(super) fn ensure_folder_group(
        &mut self,
        directory: PathBuf,
        ctx: &mut ViewContext<Self>,
    ) -> TabGroupId {
        if let Some(group_id) = self.folder_group_id(&directory) {
            return group_id;
        }
        self.store_tab_group_order();
        let folder = TabFolders::as_ref(ctx)
            .folder(&directory)
            .cloned()
            .unwrap_or_else(|| TabFolder::new(directory));
        let group = folder.to_group();
        let group_id = group.id;
        self.tab_groups.insert(group_id, group);
        self.tab_group_order.push(group_id);
        self.publish_tab_folder(group_id, ctx);
        ctx.notify();
        group_id
    }

    /// Tells the other windows about this window's version of a folder.
    pub(super) fn publish_tab_folder(&self, group_id: TabGroupId, ctx: &mut ViewContext<Self>) {
        let Some(folder) = self
            .tab_groups
            .get(&group_id)
            .and_then(TabFolder::from_group)
        else {
            return;
        };
        TabFolders::handle(ctx).update(ctx, |folders, ctx| folders.upsert(folder, ctx));
    }

    /// Makes this window's folder order the one a new window starts with.
    pub(super) fn publish_tab_folder_order(&self, ctx: &mut ViewContext<Self>) {
        let directories: Vec<PathBuf> = self
            .ordered_tab_group_ids()
            .iter()
            .filter_map(|id| self.tab_groups.get(id)?.directory.clone())
            .collect();
        TabFolders::handle(ctx).update(ctx, |folders, _| folders.set_order(&directories));
    }

    /// Registers the folders this window was created with, then takes on every other folder.
    /// A window that has no order of its own yet lists the folders in the app-wide order.
    pub(super) fn join_tab_folders(&mut self, follow_app_order: bool, ctx: &mut ViewContext<Self>) {
        if !FeatureFlag::GroupedTabs.is_enabled() || self.is_tab_drag_preview {
            return;
        }
        for group_id in self.ordered_tab_group_ids() {
            self.publish_tab_folder(group_id, ctx);
        }
        self.adopt_tab_folders(false, ctx);
        if follow_app_order {
            let folders = TabFolders::as_ref(ctx).folders();
            let app_position = |group_id: &TabGroupId| {
                self.tab_groups
                    .get(group_id)
                    .and_then(|group| group.directory.as_ref())
                    .and_then(|directory| {
                        folders
                            .iter()
                            .position(|folder| folder.directory == *directory)
                    })
                    .unwrap_or(usize::MAX)
            };
            let mut order = self.ordered_tab_group_ids();
            order.sort_by_key(app_position);
            self.tab_group_order = order;
        }
    }

    /// Brings this window's folders in line with the app-wide list: missing folders are added
    /// without tabs and shared looks are copied. With `drop_removed`, a folder that is no longer
    /// in the list is dropped here too and its tabs are left ungrouped.
    pub(super) fn adopt_tab_folders(&mut self, drop_removed: bool, ctx: &mut ViewContext<Self>) {
        if !FeatureFlag::GroupedTabs.is_enabled() || self.is_tab_drag_preview {
            return;
        }
        self.store_tab_group_order();
        let folders = TabFolders::as_ref(ctx).folders().to_vec();
        let mut changed = false;
        for folder in &folders {
            match self.folder_group_id(&folder.directory) {
                Some(group_id) => {
                    let Some(group) = self.tab_groups.get_mut(&group_id) else {
                        continue;
                    };
                    let mut updated = group.clone();
                    folder.apply_appearance(&mut updated);
                    if TabFolder::from_group(&updated) != TabFolder::from_group(group) {
                        folder.apply_appearance(group);
                        changed = true;
                    }
                }
                None => {
                    let group = folder.to_group();
                    self.tab_group_order.push(group.id);
                    self.tab_groups.insert(group.id, group);
                    changed = true;
                }
            }
        }

        if drop_removed {
            let removed: Vec<TabGroupId> = self
                .tab_groups
                .values()
                .filter(|group| {
                    group.directory.as_ref().is_some_and(|directory| {
                        !folders.iter().any(|folder| folder.directory == *directory)
                    })
                })
                .map(|group| group.id)
                .collect();
            for group_id in removed {
                self.forget_tab_group(group_id);
                changed = true;
            }
        }

        if changed {
            ctx.notify();
        }
    }

    /// Drops a group from this window, leaving any tabs it still holds ungrouped where they are.
    fn forget_tab_group(&mut self, group_id: TabGroupId) {
        for tab in &mut self.tabs {
            if tab.group_id == Some(group_id) {
                tab.group_id = None;
                // The tab did not leave by choice, so its folder may claim it again.
                tab.stays_ungrouped = false;
            }
        }
        self.tab_groups.remove(&group_id);
        self.tab_group_order.retain(|id| *id != group_id);
    }

    /// Moves the tab into `group_id` as its last tab, wherever the tab and the group are.
    pub(super) fn place_tab_in_group(
        &mut self,
        tab_index: usize,
        group_id: TabGroupId,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(tab_index) else {
            return;
        };
        if tab.group_id == Some(group_id) {
            return;
        }
        // Cleared first so that the tab does not count as a member of its old group, or of the
        // new one, while the destination is worked out.
        tab.group_id = None;
        let target = self.group_append_index(group_id);
        let tab = &mut self.tabs[tab_index];
        tab.group_id = Some(group_id);
        tab.pinned = false;
        tab.stays_ungrouped = false;
        self.move_tab_to_index(tab_index, target, ctx);
    }

    /// Files a newly opened terminal tab (the active one) under the folder of the directory it
    /// starts in. `initial_directory` is `None` when the shell starts in the home directory.
    pub(super) fn file_new_terminal_tab_under_folder(
        &mut self,
        initial_directory: Option<&Path>,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.files_tabs_under_folders(ctx) {
            return;
        }
        let Some(start) = initial_directory
            .map(Path::to_path_buf)
            .or_else(dirs::home_dir)
        else {
            return;
        };
        let group_id = self.folder_group_for_start_directory(&start, ctx);
        let tab_index = self.active_tab_index;
        self.place_tab_in_group(tab_index, group_id, ctx);
        self.expand_tab_group(group_id, ctx);
    }

    fn folder_group_for_start_directory(
        &mut self,
        start: &Path,
        ctx: &mut ViewContext<Self>,
    ) -> TabGroupId {
        let folders = self.tab_groups.values().filter_map(|group| {
            group
                .directory
                .as_deref()
                .map(|directory| (group.id, directory))
        });
        match resolve_folder_for_start_directory(start, folders) {
            Ok(group_id) => group_id,
            Err(directory) => self.ensure_folder_group(directory, ctx),
        }
    }

    /// Files restored tabs that are in no group under the folder of their saved directory. Tabs
    /// the user took out of a group are left alone. `saved_tabs` lines up with the tab list.
    pub(super) fn file_restored_tabs_under_folders(
        &mut self,
        saved_tabs: &[TabSnapshot],
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.files_tabs_under_folders(ctx) {
            return;
        }
        let pending: Vec<_> = self
            .tabs
            .iter()
            .zip(saved_tabs)
            .filter(|(tab, saved)| {
                tab.group_id.is_none() && !tab.pinned && saved.stays_ungrouped != Some(true)
            })
            .filter_map(|(tab, saved)| Some((tab.pane_group.id(), saved_tab_directory(saved)?)))
            .collect();
        for (pane_group_id, directory) in pending {
            let group_id = self.folder_group_for_start_directory(&directory, ctx);
            if let Some(tab_index) = self
                .tabs
                .iter()
                .position(|tab| tab.pane_group.id() == pane_group_id)
            {
                self.place_tab_in_group(tab_index, group_id, ctx);
            }
        }
    }

    /// Asks for a directory and adds it as a folder.
    pub(super) fn add_tab_folder(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.open_file_picker(
            |result, ctx| {
                let Ok(paths) = result else { return };
                let Some(path) = paths.into_iter().next() else {
                    return;
                };
                if let Some(workspace) = ctx.handle().upgrade(ctx) {
                    workspace.update(ctx, |workspace, ctx| {
                        workspace.add_tab_folder_for_directory(PathBuf::from(path), ctx);
                    });
                }
            },
            FilePickerConfiguration::new().folders_only(),
        );
    }

    /// Adds `directory` itself as a folder, even inside a repository, and returns its group.
    pub(crate) fn add_tab_folder_for_directory(
        &mut self,
        directory: PathBuf,
        ctx: &mut ViewContext<Self>,
    ) -> TabGroupId {
        let group_id = self.ensure_folder_group(normalized_directory(&directory), ctx);
        self.expand_tab_group(group_id, ctx);
        self.publish_tab_folder_order(ctx);
        ctx.dispatch_global_action("workspace:save_app", ());
        group_id
    }

    /// Opens a terminal tab in a folder's directory, inside the folder. Returns false when the
    /// directory is gone, in which case the folder is marked as missing instead.
    pub(super) fn new_tab_in_folder(
        &mut self,
        group_id: TabGroupId,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(group) = self.tab_groups.get_mut(&group_id) else {
            return false;
        };
        let Some(directory) = group.directory.clone() else {
            return false;
        };
        group.refresh_directory_missing();
        if group.directory_missing {
            let message = format!("The folder {} no longer exists.", directory.display());
            self.toast_stack.update(ctx, |toast_stack, ctx| {
                toast_stack.add_ephemeral_toast(DismissibleToast::error(message), ctx);
            });
            ctx.notify();
            return false;
        }

        self.suppress_folder_assignment = true;
        self.add_tab_with_pane_layout(
            PanesLayout::SingleTerminal(Box::new(
                NewTerminalOptions::default().with_initial_directory_opt(Some(directory)),
            )),
            Default::default(),
            None,
            ctx,
        );
        self.suppress_folder_assignment = false;

        let tab_index = self.active_tab_index;
        self.place_tab_in_group(tab_index, group_id, ctx);
        self.expand_tab_group(group_id, ctx);
        ctx.dispatch_global_action("workspace:save_app", ());
        true
    }

    /// Removes a group that has no tabs, in every window when it is a folder.
    pub(super) fn delete_empty_tab_group(
        &mut self,
        group_id: TabGroupId,
        ctx: &mut ViewContext<Self>,
    ) {
        if group_member_indices(&self.tabs, group_id).next().is_some() {
            return;
        }
        self.store_tab_group_order();
        let directory = self
            .tab_groups
            .get(&group_id)
            .and_then(|group| group.directory.clone());
        self.forget_tab_group(group_id);
        if let Some(directory) = directory {
            TabFolders::handle(ctx).update(ctx, |folders, ctx| folders.remove(&directory, ctx));
        }
        ctx.dispatch_global_action("workspace:save_app", ());
        ctx.notify();
    }

    /// Whether a group can move one place in `direction` in the tab list.
    pub(super) fn can_move_tab_group_in_list(
        &self,
        group_id: TabGroupId,
        direction: TabMovement,
    ) -> bool {
        self.tab_group_move_in_list(group_id, direction).is_some()
    }

    /// Works out how a group moves one place in the tab list: by swapping places with a group
    /// without tabs, or by moving its tabs as before.
    fn tab_group_move_in_list(
        &self,
        group_id: TabGroupId,
        direction: TabMovement,
    ) -> Option<TabGroupListMove> {
        let items = self.tab_list_items();
        let position = items
            .iter()
            .position(|item| matches!(item, TabListItem::Group { id, .. } if *id == group_id))?;
        let has_tabs = group_member_indices(&self.tabs, group_id).next().is_some();
        let neighbor = match direction {
            TabMovement::Left => items.get(position.checked_sub(1)?)?,
            TabMovement::Right => items.get(position + 1)?,
        };
        match neighbor {
            TabListItem::Group { id, members } if members.is_empty() || !has_tabs => {
                Some(TabGroupListMove::SwapWith(*id))
            }
            TabListItem::Group { .. } | TabListItem::Tab(_) if has_tabs => self
                .can_move_tab_group(group_id, direction)
                .then_some(TabGroupListMove::MoveTabs),
            // A group without tabs has no place among loose tabs; it sits between groups.
            TabListItem::Group { .. } | TabListItem::Tab(_) => {
                let order = self.ordered_tab_group_ids();
                let slot = order.iter().position(|id| *id == group_id)?;
                let other = match direction {
                    TabMovement::Left => order.get(slot.checked_sub(1)?)?,
                    TabMovement::Right => order.get(slot + 1)?,
                };
                Some(TabGroupListMove::SwapWith(*other))
            }
        }
    }

    /// Moves a group one place in the tab list. Returns false when the move is one of tabs that
    /// the caller has to carry out.
    pub(super) fn move_tab_group_in_list(
        &mut self,
        group_id: TabGroupId,
        direction: TabMovement,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        match self.tab_group_move_in_list(group_id, direction) {
            Some(TabGroupListMove::SwapWith(other)) => {
                let mut order = self.ordered_tab_group_ids();
                if let (Some(a), Some(b)) = (
                    order.iter().position(|id| *id == group_id),
                    order.iter().position(|id| *id == other),
                ) {
                    order.swap(a, b);
                }
                self.tab_group_order = order;
                self.publish_tab_folder_order(ctx);
                ctx.dispatch_global_action("workspace:save_app", ());
                ctx.notify();
                true
            }
            Some(TabGroupListMove::MoveTabs) => false,
            None => true,
        }
    }

    /// Re-checks which folders point at a directory that no longer exists.
    pub(super) fn refresh_missing_tab_folders(&mut self, ctx: &mut ViewContext<Self>) {
        let mut changed = false;
        for group in self.tab_groups.values_mut() {
            let was_missing = group.directory_missing;
            group.refresh_directory_missing();
            changed |= was_missing != group.directory_missing;
        }
        if changed {
            ctx.notify();
        }
    }

    /// Gives a group a freely chosen color, or with `None` takes its color away.
    pub(crate) fn set_tab_group_custom_color(
        &mut self,
        group_id: TabGroupId,
        color: Option<ColorU>,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(group) = self.tab_groups.get_mut(&group_id) else {
            return;
        };
        group.custom_color = color;
        group.color = SelectedTabColor::Unset;
        self.publish_tab_folder(group_id, ctx);
        ctx.dispatch_global_action("workspace:save_app", ());
        ctx.notify();
    }

    /// Opens the dialog in which a group's color and icon are chosen.
    pub(super) fn customize_tab_group(
        &mut self,
        group_id: TabGroupId,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(group) = self.tab_groups.get(&group_id) else {
            return;
        };
        self.customizing_tab_group = Some(group_id);
        self.tab_group_customize_dialog
            .update(ctx, |dialog, ctx| dialog.open_for_group(group, ctx));
        ctx.notify();
    }

    pub(super) fn handle_tab_group_customize_event(
        &mut self,
        event: &TabGroupCustomizeEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            TabGroupCustomizeEvent::SetColor { group_id, color } => {
                self.set_tab_group_custom_color(*group_id, *color, ctx);
            }
            TabGroupCustomizeEvent::SetIcon { group_id, icon } => {
                self.set_tab_group_icon(*group_id, icon.clone(), ctx);
            }
            TabGroupCustomizeEvent::Close => {
                self.customizing_tab_group = None;
                self.focus_active_tab(ctx);
                ctx.notify();
            }
        }
    }

    pub(crate) fn set_tab_group_icon(
        &mut self,
        group_id: TabGroupId,
        icon: Option<TabGroupIcon>,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(group) = self.tab_groups.get_mut(&group_id) else {
            return;
        };
        group.icon = icon;
        self.publish_tab_folder(group_id, ctx);
        ctx.dispatch_global_action("workspace:save_app", ());
        ctx.notify();
    }
}

enum TabGroupListMove {
    /// Trade places with another group in the stored order; no tab moves.
    SwapWith(TabGroupId),
    /// Move the group's tabs past its neighbor.
    MoveTabs,
}

#[cfg(test)]
#[path = "tab_folder_groups_tests.rs"]
mod tests;
