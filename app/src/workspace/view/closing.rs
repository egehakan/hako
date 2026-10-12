use itertools::Itertools;
use warpui::{EntityId, SingletonEntity, ViewContext, ViewHandle};

use super::{OpenDialogSource, Workspace, group_member_indices};
use crate::pane_group::{PaneGroup, PaneId};
use crate::quit_warning::UnsavedStateSummary;
use crate::workspace::close_confirmation::{
    CloseConfirmationEvent, CloseDecision, CloseStakes, CloseSubject, close_decision,
    confirms_every_close,
};
use crate::workspace::tab_folders::TabFolders;
use crate::workspace::tab_group::{TabGroup, TabGroupId};

/// A close that waits for the answer to the confirmation dialog. Tabs are held by the id of their
/// pane group, since tab indices can shift while the dialog is open.
pub(super) enum PendingClose {
    Pane {
        pane_group_id: EntityId,
        pane_id: PaneId,
    },
    Tabs {
        pane_group_ids: Vec<EntityId>,
        add_to_undo_stack: bool,
        group: Option<PendingGroupClose>,
    },
    Window,
}

/// Says that the tabs being closed are those of a group.
#[derive(Clone, Copy)]
pub(super) struct PendingGroupClose {
    pub(super) group_id: TabGroupId,
    /// Whether the group goes too, even when it is a folder.
    pub(super) removes_group: bool,
}

/// How a window's request to close stands with the confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowCloseConfirmation {
    /// The dialog is now showing; the window must stay open until it is answered.
    Asked,
    /// The close was already confirmed, so nothing more is to be asked.
    Confirmed,
    /// Closing is not confirmed up front; the warnings about running work apply.
    NotNeeded,
}

impl Workspace {
    fn tab_title(&self, tab_index: usize, ctx: &ViewContext<Self>) -> String {
        self.tabs
            .get(tab_index)
            .map(|tab| tab.pane_group.as_ref(ctx).display_title(ctx))
            .unwrap_or_default()
    }

    /// What closing the tabs at `tab_indices` would lose. Saves what auto-save can first.
    fn close_stakes_for_tabs(
        &self,
        tab_indices: &[usize],
        ctx: &mut ViewContext<Self>,
    ) -> CloseStakes {
        let tabs = tab_indices
            .iter()
            .filter_map(|index| self.get_pane_group_view(*index))
            .map(|tab| tab.downgrade())
            .collect_vec();
        let summary = UnsavedStateSummary::for_tabs(tabs, ctx);
        CloseStakes {
            running_commands: summary.total_long_running_commands,
            shared_sessions: summary.shared_sessions,
            unsaved_changes: summary.save_unsaved_code(ctx),
        }
    }

    #[cfg(test)]
    pub(crate) fn is_close_confirmation_open(&self) -> bool {
        self.pending_close.is_some()
    }

    /// Shows the confirmation for `pending`. A request made while one is showing is dropped:
    /// the dialog on screen is about the first request and has to be answered.
    fn ask_before_closing(
        &mut self,
        pending: PendingClose,
        subject: CloseSubject,
        stakes: CloseStakes,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.pending_close.is_some() {
            return;
        }
        let CloseDecision::Ask(text) = close_decision(&subject, stakes, true) else {
            return;
        };
        self.pending_close = Some(pending);
        self.customizing_tab_group = None;
        self.close_confirmation_dialog
            .update(ctx, |dialog, ctx| dialog.show(text, ctx));
        ctx.focus(&self.close_confirmation_dialog);
        ctx.notify();
    }

    /// Asks before closing the tabs at `tab_indices`, as the tabs of `group` when given.
    pub(super) fn ask_before_closing_tabs(
        &mut self,
        tab_indices: Vec<usize>,
        add_to_undo_stack: bool,
        group: Option<PendingGroupClose>,
        ctx: &mut ViewContext<Self>,
    ) {
        let stakes = self.close_stakes_for_tabs(&tab_indices, ctx);
        let subject = match (group, tab_indices.as_slice()) {
            (Some(group), _) => CloseSubject::Group {
                name: self
                    .tab_groups
                    .get(&group.group_id)
                    .map(TabGroup::display_name)
                    .unwrap_or_default(),
                tab_count: tab_indices.len(),
                removes_group: group.removes_group,
            },
            (None, [tab_index]) => CloseSubject::Tab {
                title: self.tab_title(*tab_index, ctx),
                closes_window: self.tabs.len() == 1,
            },
            (None, _) => CloseSubject::Tabs {
                count: tab_indices.len(),
            },
        };
        let pane_group_ids = tab_indices
            .iter()
            .filter_map(|index| self.tabs.get(*index))
            .map(|tab| tab.pane_group.id())
            .collect();
        self.ask_before_closing(
            PendingClose::Tabs {
                pane_group_ids,
                add_to_undo_stack,
                group,
            },
            subject,
            stakes,
            ctx,
        );
    }

    /// Asks before closing a pane. Closing the only pane of a tab closes the tab, and the dialog
    /// says so.
    pub(super) fn ask_before_closing_pane(
        &mut self,
        pane_group: &ViewHandle<PaneGroup>,
        pane_id: PaneId,
        stakes: CloseStakes,
        ctx: &mut ViewContext<Self>,
    ) {
        let tab_index = self
            .tabs
            .iter()
            .position(|tab| tab.pane_group.id() == pane_group.id());
        let closes_tab = pane_group.as_ref(ctx).visible_pane_ids().len() <= 1;
        let subject = match tab_index {
            Some(tab_index) if closes_tab => CloseSubject::Tab {
                title: self.tab_title(tab_index, ctx),
                closes_window: self.tabs.len() == 1,
            },
            Some(_) | None => CloseSubject::Pane {
                title: pane_group
                    .as_ref(ctx)
                    .pane_by_id(pane_id)
                    .map(|pane| pane.pane_configuration().as_ref(ctx).title().to_string())
                    .unwrap_or_default(),
            },
        };
        self.ask_before_closing(
            PendingClose::Pane {
                pane_group_id: pane_group.id(),
                pane_id,
            },
            subject,
            stakes,
            ctx,
        );
    }

    /// Decides whether this window may close now. `stakes` is what closing it would lose.
    pub fn confirm_window_close(
        &mut self,
        stakes: CloseStakes,
        ctx: &mut ViewContext<Self>,
    ) -> WindowCloseConfirmation {
        if std::mem::take(&mut self.window_close_confirmed) {
            return WindowCloseConfirmation::Confirmed;
        }
        if !confirms_every_close(ctx) {
            return WindowCloseConfirmation::NotNeeded;
        }
        let subject = CloseSubject::Window {
            tab_count: self.tabs.len(),
        };
        self.ask_before_closing(PendingClose::Window, subject, stakes, ctx);
        WindowCloseConfirmation::Asked
    }

    pub(super) fn handle_close_confirmation_event(
        &mut self,
        event: &CloseConfirmationEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(pending) = self.pending_close.take() else {
            return;
        };
        match event {
            CloseConfirmationEvent::Cancelled => {}
            CloseConfirmationEvent::Confirmed => self.carry_out_close(pending, ctx),
        }
        self.focus_active_tab(ctx);
        ctx.notify();
    }

    fn carry_out_close(&mut self, pending: PendingClose, ctx: &mut ViewContext<Self>) {
        match pending {
            PendingClose::Pane {
                pane_group_id,
                pane_id,
            } => self.close_pane(pane_group_id, pane_id, ctx),
            PendingClose::Tabs {
                pane_group_ids,
                add_to_undo_stack,
                group,
            } => {
                let tab_indices = self
                    .tabs
                    .iter()
                    .positions(|tab| pane_group_ids.contains(&tab.pane_group.id()))
                    .collect_vec();
                match (tab_indices.as_slice(), group) {
                    ([], _) => {}
                    ([tab_index], None) => self.close_tab(*tab_index, true, add_to_undo_stack, ctx),
                    ([first, ..], _) => {
                        let dialog_source = OpenDialogSource::CloseOtherTabs { tab_index: *first };
                        self.close_tabs(
                            tab_indices.into_iter(),
                            dialog_source,
                            true,
                            add_to_undo_stack,
                            ctx,
                        );
                    }
                }
                if let Some(group) = group {
                    self.finish_group_close(group, ctx);
                }
            }
            PendingClose::Window => {
                // Closing goes back through the window's close request, so that what follows a
                // confirmed close (such as quitting with the last window) still happens.
                self.window_close_confirmed = true;
                ctx.close_window();
            }
        }
    }

    /// Removes a group whose tabs were just closed, when it is to go: a folder only when it was
    /// removed, any other group always.
    pub(super) fn finish_group_close(
        &mut self,
        group: PendingGroupClose,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(directory) = self
            .tab_groups
            .get(&group.group_id)
            .map(|group| group.directory.clone())
        else {
            return;
        };
        if directory.is_some() && !group.removes_group {
            return;
        }
        if group_member_indices(&self.tabs, group.group_id)
            .next()
            .is_none()
        {
            self.delete_empty_tab_group(group.group_id, ctx);
        } else if let Some(directory) = directory {
            // The last tab of a window is not removed; the window closes instead. The folder
            // still has to go from the other windows.
            TabFolders::handle(ctx).update(ctx, |folders, ctx| folders.remove(&directory, ctx));
        }
    }
}

#[cfg(test)]
#[path = "closing_tests.rs"]
mod tests;
