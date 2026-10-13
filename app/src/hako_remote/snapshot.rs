//! The workspace as a remote client sees it. Reading it takes no terminal model lock.

use ::hako_remote::protocol::{
    AgentState, AgentStatus, Folder, Pane, PaneKind, Tab, Window, Workspace as WorkspaceSnapshot,
};
use warp_core::ui::theme::WarpTheme;
use warpui::{AppContext, EntityId, SingletonEntity, ViewHandle, WindowId};

use crate::appearance::Appearance;
use crate::pane_group::pane::IPaneType;
use crate::pane_group::{PaneGroup, PaneId};
use crate::terminal::cli_agent_sessions::{CLIAgentSessionStatus, CLIAgentSessionsModel};
use crate::workspace::tab_folders::{TabListItem, format_hex_color};
use crate::workspace::tab_group::{TabGroup, TabGroupId};
use crate::workspace::view::pane_close_label;
use crate::workspace::{Workspace, WorkspaceRegistry};

/// Every window that holds tabs, oldest first.
pub(super) fn workspace_windows(ctx: &AppContext) -> Vec<(WindowId, ViewHandle<Workspace>)> {
    let mut windows = WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx);
    windows.retain(|(_, workspace)| !workspace.as_ref(ctx).is_tab_drag_preview());
    windows.sort_by_key(|(window_id, _)| *window_id);
    windows
}

/// The window that is in front, or was when the app was last in front.
pub(super) fn active_window(
    windows: &[(WindowId, ViewHandle<Workspace>)],
    ctx: &AppContext,
) -> Option<WindowId> {
    ctx.windows()
        .frontmost_window_id()
        .filter(|frontmost| windows.iter().any(|(window_id, _)| window_id == frontmost))
        .or_else(|| windows.last().map(|(window_id, _)| *window_id))
}

pub(super) fn window_id(window_id: WindowId) -> String {
    window_id.to_string()
}

pub(super) fn tab_id(pane_group_id: EntityId) -> String {
    pane_group_id.to_string()
}

pub(super) fn folder_id(group_id: TabGroupId) -> String {
    group_id.0.to_string()
}

pub(super) fn session_id(pane_group: &PaneGroup, pane_id: PaneId) -> Option<String> {
    pane_group
        .terminal_session_uuid(pane_id)
        .map(|uuid| uuid.to_string())
}

pub(super) fn workspace(ctx: &AppContext) -> WorkspaceSnapshot {
    let windows = workspace_windows(ctx);
    let active = active_window(&windows, ctx);
    let theme = Appearance::as_ref(ctx).theme();
    WorkspaceSnapshot {
        windows: windows
            .iter()
            .map(|(id, workspace)| Window {
                id: window_id(*id),
                active: Some(*id) == active,
                folders: folders(workspace.as_ref(ctx), theme),
                tabs: tabs(workspace.as_ref(ctx), ctx),
            })
            .collect(),
    }
}

/// The window's folders in the order the tab list shows them. A group that stands for no
/// directory is not a folder and is left out.
fn folders(workspace: &Workspace, theme: &WarpTheme) -> Vec<Folder> {
    workspace
        .tab_list_items()
        .into_iter()
        .filter_map(|item| match item {
            TabListItem::Group { id, .. } => folder(workspace.tab_groups.get(&id)?, theme),
            TabListItem::Tab(_) => None,
        })
        .collect()
}

fn folder(group: &TabGroup, theme: &WarpTheme) -> Option<Folder> {
    let directory = group.directory.as_ref()?;
    Some(Folder {
        id: folder_id(group.id),
        name: group.display_name(),
        directory: directory.to_string_lossy().into_owned(),
        color: group.resolved_color(theme).map(format_hex_color),
        icon: group.icon.as_ref().map(|icon| icon.serialize()),
    })
}

fn tabs(workspace: &Workspace, ctx: &AppContext) -> Vec<Tab> {
    workspace
        .tabs
        .iter()
        .enumerate()
        .map(|(index, tab)| Tab {
            id: tab_id(tab.pane_group.id()),
            title: workspace.tab_title(index, ctx),
            active: index == workspace.active_tab_index(),
            folder_id: tab
                .group_id
                .filter(|group_id| {
                    workspace
                        .tab_groups
                        .get(group_id)
                        .is_some_and(TabGroup::is_folder)
                })
                .map(folder_id),
            panes: panes(tab.pane_group.as_ref(ctx), ctx),
        })
        .collect()
}

fn panes(pane_group: &PaneGroup, ctx: &AppContext) -> Vec<Pane> {
    let focused = pane_group.focused_pane_id(ctx);
    pane_group
        .visible_pane_ids()
        .into_iter()
        .map(|pane_id| {
            let terminal = pane_group.terminal_view_from_pane_id(pane_id, ctx);
            Pane {
                id: pane_id.creation_order_id().to_string(),
                kind: pane_kind(pane_id),
                title: pane_close_label(pane_group, pane_id, ctx),
                focused: pane_id == focused,
                session_id: session_id(pane_group, pane_id),
                cwd: terminal
                    .as_ref()
                    .and_then(|terminal| terminal.as_ref(ctx).pwd()),
                agent: terminal
                    .as_ref()
                    .and_then(|terminal| agent_state(terminal.id(), ctx)),
            }
        })
        .collect()
}

fn pane_kind(pane_id: PaneId) -> PaneKind {
    match pane_id.pane_type() {
        IPaneType::Terminal => PaneKind::Terminal,
        IPaneType::File | IPaneType::Code => PaneKind::File,
        IPaneType::Notebook
        | IPaneType::CodeDiff
        | IPaneType::EnvVarCollection
        | IPaneType::EnvironmentManagement
        | IPaneType::Workflow
        | IPaneType::Settings
        | IPaneType::AIFact
        | IPaneType::AIDocument
        | IPaneType::CustomRouterEditor
        | IPaneType::ExecutionProfileEditor
        | IPaneType::GetStarted
        | IPaneType::NetworkLog
        | IPaneType::DeferredPlaceholder => PaneKind::Other,
        #[cfg(test)]
        IPaneType::Dummy => PaneKind::Other,
    }
}

/// The coding agent running in a terminal, once it reports a status that can be relied on.
fn agent_state(terminal_view_id: EntityId, ctx: &AppContext) -> Option<AgentState> {
    let session = CLIAgentSessionsModel::as_ref(ctx)
        .session(terminal_view_id)
        .filter(|session| session.supports_rich_status())?;
    let (status, message) = match &session.status {
        CLIAgentSessionStatus::InProgress => (AgentStatus::Working, None),
        CLIAgentSessionStatus::Blocked { message, source: _ } => {
            (AgentStatus::Waiting, message.clone())
        }
        CLIAgentSessionStatus::Success | CLIAgentSessionStatus::Cancelled => {
            (AgentStatus::Done, None)
        }
        CLIAgentSessionStatus::Failed {
            message,
            error_type: _,
        } => (AgentStatus::Failed, message.clone()),
    };
    Some(AgentState {
        name: session.agent.display_name().to_owned(),
        status,
        message,
    })
}
