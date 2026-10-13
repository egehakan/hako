//! Answers to what a remote client asks.

use std::path::PathBuf;

use ::hako_remote::protocol::{Method, RpcError, TabCreateParams, TabCreateResult, TabParams};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use warpui::{AppContext, EntityId, SingletonEntity, ViewHandle, WindowId};

use super::app_info::app_info;
use super::snapshot;
use crate::pane_group::PaneGroup;
use crate::root_view::{OpenPath, open_new_from_path, open_new_window_get_handles};
use crate::terminal::general_settings::GeneralSettings;
use crate::workspace::view::{RemoteCloseRefusal, RemoteTabPlace};
use crate::workspace::{Workspace, WorkspaceRegistry};

pub(super) fn handle_request(
    method: &str,
    params: Value,
    ctx: &mut AppContext,
) -> Result<Value, RpcError> {
    let Some(known_method) = Method::from_name(method) else {
        return Err(RpcError::not_supported(method));
    };
    match known_method {
        Method::AppInfo => to_json(&app_info()),
        Method::WorkspaceList => to_json(&snapshot::workspace(ctx)),
        Method::TabCreate => to_json(&create_tab(parse(params)?, ctx)?),
        Method::TabActivate => activate_tab(parse(params)?, ctx).map(|()| json!({})),
        Method::TabClose => close_tab(parse(params)?, ctx).map(|()| json!({})),
    }
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params)
        .map_err(|error| RpcError::bad_request(format!("Invalid params: {error}")))
}

fn to_json<T: Serialize>(result: &T) -> Result<Value, RpcError> {
    serde_json::to_value(result).map_err(|error| RpcError::failed(error.to_string()))
}

fn find_tab(
    tab_id: &str,
    ctx: &AppContext,
) -> Result<(WindowId, ViewHandle<Workspace>, EntityId), RpcError> {
    snapshot::workspace_windows(ctx)
        .into_iter()
        .find_map(|(window_id, workspace)| {
            let pane_group_id = workspace
                .as_ref(ctx)
                .tab_views()
                .map(|pane_group| pane_group.id())
                .find(|pane_group_id| snapshot::tab_id(*pane_group_id) == tab_id)?;
            Some((window_id, workspace, pane_group_id))
        })
        .ok_or_else(|| RpcError::not_found("No such tab"))
}

/// Where a new tab goes.
enum TabDestination {
    Window(ViewHandle<Workspace>, RemoteTabPlace),
    /// A window of its own, starting in the directory when one is given. This is where a tab
    /// goes while no window is open.
    NewWindow(Option<PathBuf>),
}

fn tab_destination(params: TabCreateParams, ctx: &AppContext) -> Result<TabDestination, RpcError> {
    let windows = snapshot::workspace_windows(ctx);
    if let Some(folder_id) = params.folder_id {
        return windows
            .into_iter()
            .find_map(|(_, workspace)| {
                let group_id = workspace
                    .as_ref(ctx)
                    .tab_groups
                    .values()
                    .find(|group| group.is_folder() && snapshot::folder_id(group.id) == folder_id)?
                    .id;
                Some(TabDestination::Window(
                    workspace,
                    RemoteTabPlace::Folder(group_id),
                ))
            })
            .ok_or_else(|| RpcError::not_found("No such folder"));
    }

    let directory = match params.directory {
        Some(directory) => {
            let directory = PathBuf::from(directory);
            if !directory.is_absolute() || !directory.is_dir() {
                return Err(RpcError::not_found("No such directory"));
            }
            Some(directory)
        }
        None => None,
    };
    let window_id = match params.window_id {
        Some(requested) => windows
            .iter()
            .map(|(window_id, _)| *window_id)
            .find(|window_id| snapshot::window_id(*window_id) == requested)
            .ok_or_else(|| RpcError::not_found("No such window"))?,
        None => match snapshot::active_window(&windows, ctx) {
            Some(window_id) => window_id,
            None => return Ok(TabDestination::NewWindow(directory)),
        },
    };
    let workspace = windows
        .into_iter()
        .find_map(|(id, workspace)| (id == window_id).then_some(workspace))
        .ok_or_else(|| RpcError::not_found("No such window"))?;
    let place = match directory {
        Some(directory) => RemoteTabPlace::Directory(directory),
        None => RemoteTabPlace::Default,
    };
    Ok(TabDestination::Window(workspace, place))
}

fn create_tab(params: TabCreateParams, ctx: &mut AppContext) -> Result<TabCreateResult, RpcError> {
    let pane_group = match tab_destination(params, ctx)? {
        TabDestination::Window(workspace, place) => workspace
            .update(ctx, |workspace, ctx| {
                workspace.open_tab_for_remote(place, ctx)
            })
            .ok_or_else(|| RpcError::failed("The tab could not be opened"))?,
        TabDestination::NewWindow(directory) => open_window_with_tab(directory, ctx)?,
    };
    let session_id = pane_group
        .read(ctx, |pane_group, ctx| {
            let pane_id = pane_group.active_session_id(ctx)?;
            snapshot::session_id(pane_group, pane_id.into())
        })
        .ok_or_else(|| RpcError::failed("The new tab has no terminal"))?;
    Ok(TabCreateResult {
        tab_id: snapshot::tab_id(pane_group.id()),
        session_id,
    })
}

/// Opens a window, which comes with one terminal tab, and returns that tab.
fn open_window_with_tab(
    directory: Option<PathBuf>,
    ctx: &mut AppContext,
) -> Result<ViewHandle<PaneGroup>, RpcError> {
    let (window_id, _) = match directory {
        Some(path) => open_new_from_path(&OpenPath { path }, ctx),
        None => open_new_window_get_handles(None, ctx),
    };
    // A window that opens on the sign-in screen has no tabs yet.
    WorkspaceRegistry::as_ref(ctx)
        .get(window_id, ctx)
        .and_then(|workspace| {
            let workspace = workspace.as_ref(ctx);
            workspace
                .get_pane_group_view(workspace.active_tab_index())
                .cloned()
        })
        .ok_or_else(|| RpcError::failed("The window opened without a tab"))
}

fn activate_tab(params: TabParams, ctx: &mut AppContext) -> Result<(), RpcError> {
    let (window_id, workspace, pane_group_id) = find_tab(&params.tab_id, ctx)?;
    workspace.update(ctx, |workspace, ctx| {
        workspace.activate_tab_by_pane_group_id(pane_group_id, ctx);
    });
    ctx.windows().show_window_and_focus_app(window_id);
    Ok(())
}

fn close_tab(params: TabParams, ctx: &mut AppContext) -> Result<(), RpcError> {
    let (_, workspace, pane_group_id) = find_tab(&params.tab_id, ctx)?;
    // A window goes with its last tab. Where the app quits with its last window, that would end
    // remote access too, with nobody at the Mac to start it again.
    if workspace.as_ref(ctx).tab_count() == 1 && closing_a_window_quits_the_app(ctx) {
        return Err(RpcError::failed(
            "Closing the last tab would quit Hako on the Mac",
        ));
    }
    workspace
        .update(ctx, |workspace, ctx| {
            workspace.close_tab_for_remote(pane_group_id, ctx)
        })
        .map_err(|refusal| match refusal {
            RemoteCloseRefusal::NoSuchTab => RpcError::not_found("No such tab"),
            RemoteCloseRefusal::UnsavedChanges => {
                RpcError::failed("The tab has unsaved changes; close it on the Mac")
            }
            RemoteCloseRefusal::LastTab => RpcError::failed("The last tab cannot be closed"),
        })
}

fn closing_a_window_quits_the_app(ctx: &AppContext) -> bool {
    let quits_with_last_window = cfg!(any(target_os = "linux", target_os = "freebsd", windows))
        || *GeneralSettings::as_ref(ctx).quit_on_last_window_closed;
    quits_with_last_window && ctx.window_ids().count() == 1
}
