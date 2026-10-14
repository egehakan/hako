//! What a remote client can ask of a window: opening, closing and renaming tabs, and renaming
//! folders.

use std::path::PathBuf;

use warp_core::context_flag::ContextFlag;
use warpui::{EntityId, TypedActionView, ViewContext, ViewHandle};

use super::Workspace;
use crate::pane_group::PaneGroup;
use crate::workspace::action::WorkspaceAction;
use crate::workspace::tab_group::TabGroupId;

/// Where a tab opened for a remote client goes.
pub(crate) enum RemoteTabPlace {
    /// In a folder, as its "+" button does.
    Folder(TabGroupId),
    Directory(PathBuf),
    /// Where a new tab opens by default.
    Default,
}

/// Why a tab was not closed for a remote client.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoteCloseRefusal {
    NoSuchTab,
    /// The tab holds edits that auto-save cannot save.
    UnsavedChanges,
    /// The tab is the last one of a window that may not be closed.
    LastTab,
}

impl Workspace {
    /// Opens a tab with one terminal and makes it the active tab. Returns `None` when no tab
    /// was opened, which is the case for a folder whose directory no longer exists.
    pub(crate) fn open_tab_for_remote(
        &mut self,
        place: RemoteTabPlace,
        ctx: &mut ViewContext<Self>,
    ) -> Option<ViewHandle<PaneGroup>> {
        let tab_count = self.tabs.len();
        match place {
            RemoteTabPlace::Folder(group_id) => {
                self.handle_action(&WorkspaceAction::NewTabInGroup(group_id), ctx);
            }
            RemoteTabPlace::Directory(directory) => {
                self.open_directory_in_new_tab(directory, ctx);
                ctx.dispatch_global_action("workspace:save_app", ());
                ctx.notify();
            }
            RemoteTabPlace::Default => {
                self.handle_action(
                    &WorkspaceAction::AddTerminalTab {
                        hide_homepage: false,
                    },
                    ctx,
                );
            }
        }
        (self.tabs.len() > tab_count).then(|| self.active_tab_pane_group().clone())
    }

    /// Closes the tab of `pane_group_id` without asking first, since the remote client already
    /// did. The last tab takes its window with it. What the client could not have asked about,
    /// edits that cannot be saved, keeps the tab open instead.
    pub(crate) fn close_tab_for_remote(
        &mut self,
        pane_group_id: EntityId,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), RemoteCloseRefusal> {
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.pane_group.id() == pane_group_id)
            .ok_or(RemoteCloseRefusal::NoSuchTab)?;
        let closes_window = self.tabs.len() == 1;
        if closes_window && !ContextFlag::CloseWindow.is_enabled() {
            return Err(RemoteCloseRefusal::LastTab);
        }
        if self.close_stakes_for_tabs(&[index], ctx).unsaved_changes {
            return Err(RemoteCloseRefusal::UnsavedChanges);
        }
        self.close_tab(index, true, true, ctx);
        if closes_window {
            // The window's own request to close arrives later, and must not ask either.
            self.window_close_confirmed = true;
        }
        Ok(())
    }

    /// Gives the tab of `pane_group_id` its own title, or for `None` goes back to the automatic
    /// one, as renaming it on the Mac does. Returns whether there is such a tab.
    pub(crate) fn rename_tab_for_remote(
        &mut self,
        pane_group_id: EntityId,
        title: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.pane_group.id() == pane_group_id)
        else {
            return false;
        };
        tab.pane_group.update(ctx, |pane_group, ctx| {
            pane_group.set_custom_title(title, ctx);
        });
        self.update_window_title(ctx);
        ctx.dispatch_global_action("workspace:save_app", ());
        ctx.notify();
        true
    }

    /// Names the folder of `group_id`, or for `None` goes back to its default name, as renaming
    /// it on the Mac does. Returns whether there is such a folder.
    pub(crate) fn rename_folder_for_remote(
        &mut self,
        group_id: TabGroupId,
        name: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(group) = self
            .tab_groups
            .get_mut(&group_id)
            .filter(|group| group.is_folder())
        else {
            return false;
        };
        group.name = name;
        self.publish_tab_folder(group_id, ctx);
        ctx.dispatch_global_action("workspace:save_app", ());
        ctx.notify();
        true
    }
}

#[cfg(test)]
#[path = "remote_access_tests.rs"]
mod tests;
