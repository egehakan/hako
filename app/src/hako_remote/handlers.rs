//! Answers to what a remote client asks.

use std::path::{Path, PathBuf};

use ::hako_remote::files;
use ::hako_remote::protocol::{
    FolderCreateParams, FolderCreateResult, FolderRenameParams, FsRoots, RpcError, TabCreateParams,
    TabCreateResult, TabParams, TabRenameParams, TerminalColors, Theme,
};
use ::hako_remote::rename::{
    MAX_FOLDER_NAME_CHARS, MAX_TAB_TITLE_CHARS, chosen_name, well_formed_tab_id,
};
use pathfinder_color::ColorU;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use warp_core::ui::theme::AnsiColors;
use warpui::{AppContext, EntityId, SingletonEntity, ViewHandle, WindowId};

use super::snapshot;
use crate::appearance::Appearance;
#[cfg(feature = "local_fs")]
use crate::code_review::nested_repos::{contains_home_directory, find_nested_repos};
use crate::pane_group::PaneGroup;
use crate::root_view::{OpenPath, open_new_from_path, open_new_window_get_handles};
use crate::terminal::general_settings::GeneralSettings;
use crate::workspace::tab_folders::format_hex_color;
use crate::workspace::tab_group::TabGroupId;
use crate::workspace::view::{RemoteCloseRefusal, RemoteTabPlace};
use crate::workspace::{Workspace, WorkspaceRegistry};

pub(super) fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params)
        .map_err(|error| RpcError::bad_request(format!("Invalid params: {error}")))
}

pub(super) fn to_json<T: Serialize>(result: &T) -> Result<Value, RpcError> {
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

fn find_folder(
    folder_id: &str,
    ctx: &AppContext,
) -> Result<(ViewHandle<Workspace>, TabGroupId), RpcError> {
    snapshot::workspace_windows(ctx)
        .into_iter()
        .find_map(|(_, workspace)| {
            let group_id = workspace
                .as_ref(ctx)
                .tab_groups
                .values()
                .find(|group| group.is_folder() && snapshot::folder_id(group.id) == folder_id)?
                .id;
            Some((workspace, group_id))
        })
        .ok_or_else(|| RpcError::not_found("No such folder"))
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
        let (workspace, group_id) = find_folder(&folder_id, ctx)?;
        return Ok(TabDestination::Window(
            workspace,
            RemoteTabPlace::Folder(group_id),
        ));
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
    let Some(workspace) = requested_window(params.window_id, windows, ctx)? else {
        return Ok(TabDestination::NewWindow(directory));
    };
    let place = match directory {
        Some(directory) => RemoteTabPlace::Directory(directory),
        None => RemoteTabPlace::Default,
    };
    Ok(TabDestination::Window(workspace, place))
}

/// The window a request names, or the active one when it names none, among `windows`. `None`
/// when there is no window to take.
fn requested_window(
    requested: Option<String>,
    windows: Vec<(WindowId, ViewHandle<Workspace>)>,
    ctx: &AppContext,
) -> Result<Option<ViewHandle<Workspace>>, RpcError> {
    let window_id = match requested {
        Some(requested) => windows
            .iter()
            .map(|(window_id, _)| *window_id)
            .find(|window_id| snapshot::window_id(*window_id) == requested)
            .ok_or_else(|| RpcError::not_found("No such window"))?,
        None => match snapshot::active_window(&windows, ctx) {
            Some(window_id) => window_id,
            None => return Ok(None),
        },
    };
    windows
        .into_iter()
        .find_map(|(id, workspace)| (id == window_id).then_some(workspace))
        .ok_or_else(|| RpcError::not_found("No such window"))
        .map(Some)
}

/// A tab that was asked for.
pub(super) enum NewTab {
    Open(TabCreateResult),
    /// The tab is the first one of a window that has just been opened, which gets it a moment
    /// after it appears.
    InNewWindow(WindowId),
}

pub(super) fn create_tab(
    params: TabCreateParams,
    ctx: &mut AppContext,
) -> Result<NewTab, RpcError> {
    match tab_destination(params, ctx)? {
        TabDestination::Window(workspace, place) => {
            let pane_group = workspace
                .update(ctx, |workspace, ctx| {
                    workspace.open_tab_for_remote(place, ctx)
                })
                .ok_or_else(|| RpcError::failed("The tab could not be opened"))?;
            tab_with_terminal(&pane_group, ctx)
                .map(NewTab::Open)
                .ok_or_else(|| RpcError::failed("The new tab has no terminal"))
        }
        TabDestination::NewWindow(directory) => {
            let (window_id, _) = match directory {
                Some(path) => open_new_from_path(&OpenPath { path }, ctx),
                None => open_new_window_get_handles(None, ctx),
            };
            Ok(first_tab(window_id, ctx).map_or(NewTab::InNewWindow(window_id), NewTab::Open))
        }
    }
}

/// The tab a window opened with, once it is there and holds a terminal.
pub(super) fn first_tab(window_id: WindowId, ctx: &AppContext) -> Option<TabCreateResult> {
    let workspace = WorkspaceRegistry::as_ref(ctx).get(window_id, ctx)?;
    let workspace = workspace.as_ref(ctx);
    let pane_group = workspace.get_pane_group_view(workspace.active_tab_index())?;
    tab_with_terminal(pane_group, ctx)
}

fn tab_with_terminal(
    pane_group: &ViewHandle<PaneGroup>,
    ctx: &AppContext,
) -> Option<TabCreateResult> {
    let session_id = pane_group.read(ctx, |pane_group, ctx| {
        let pane_id = pane_group.active_session_id(ctx)?;
        snapshot::session_id(pane_group, pane_id.into())
    })?;
    Some(TabCreateResult {
        tab_id: snapshot::tab_id(pane_group.id()),
        session_id,
    })
}

/// A folder that was asked for.
pub(super) enum NewFolder {
    Added(FolderCreateResult),
    /// The folder goes into a window that has just been opened, which can take it a moment
    /// after it appears.
    InNewWindow(WindowId, PathBuf),
}

/// Adds the folder of an existing directory to a window, as adding it on the Mac does. A window
/// that already has the folder keeps the one it has.
pub(super) fn create_folder(
    params: FolderCreateParams,
    ctx: &mut AppContext,
) -> Result<NewFolder, RpcError> {
    let directory = PathBuf::from(params.directory);
    let windows = snapshot::workspace_windows(ctx);
    Ok(match requested_window(params.window_id, windows, ctx)? {
        Some(workspace) => NewFolder::Added(add_folder(&workspace, directory, ctx)),
        None => {
            let (window_id, _) = open_new_window_get_handles(None, ctx);
            NewFolder::InNewWindow(window_id, directory)
        }
    })
}

/// Adds the folder to a window, once the window is there to take it.
pub(super) fn folder_in_window(
    window_id: WindowId,
    directory: &Path,
    ctx: &mut AppContext,
) -> Option<FolderCreateResult> {
    let workspace = WorkspaceRegistry::as_ref(ctx).get(window_id, ctx)?;
    Some(add_folder(&workspace, directory.to_path_buf(), ctx))
}

fn add_folder(
    workspace: &ViewHandle<Workspace>,
    directory: PathBuf,
    ctx: &mut AppContext,
) -> FolderCreateResult {
    let group_id = workspace.update(ctx, |workspace, ctx| {
        workspace.add_tab_folder_for_directory(directory, ctx)
    });
    FolderCreateResult {
        folder_id: snapshot::folder_id(group_id),
    }
}

pub(super) fn activate_tab(params: TabParams, ctx: &mut AppContext) -> Result<(), RpcError> {
    let (window_id, workspace, pane_group_id) = find_tab(&params.tab_id, ctx)?;
    workspace.update(ctx, |workspace, ctx| {
        workspace.activate_tab_by_pane_group_id(pane_group_id, ctx);
    });
    ctx.windows().show_window_and_focus_app(window_id);
    Ok(())
}

pub(super) fn close_tab(params: TabParams, ctx: &mut AppContext) -> Result<(), RpcError> {
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

pub(super) fn rename_tab(params: TabRenameParams, ctx: &mut AppContext) -> Result<(), RpcError> {
    well_formed_tab_id(&params.tab_id)?;
    let title = chosen_name(params.title.as_deref(), MAX_TAB_TITLE_CHARS)?;
    let (_, workspace, pane_group_id) = find_tab(&params.tab_id, ctx)?;
    let renamed = workspace.update(ctx, |workspace, ctx| {
        workspace.rename_tab_for_remote(pane_group_id, title, ctx)
    });
    renamed
        .then_some(())
        .ok_or_else(|| RpcError::not_found("No such tab"))
}

pub(super) fn rename_folder(
    params: FolderRenameParams,
    ctx: &mut AppContext,
) -> Result<(), RpcError> {
    let name = chosen_name(params.name.as_deref(), MAX_FOLDER_NAME_CHARS)?;
    let (workspace, group_id) = find_folder(&params.folder_id, ctx)?;
    let renamed = workspace.update(ctx, |workspace, ctx| {
        workspace.rename_folder_for_remote(group_id, name, ctx)
    });
    renamed
        .then_some(())
        .ok_or_else(|| RpcError::not_found("No such folder"))
}

fn closing_a_window_quits_the_app(ctx: &AppContext) -> bool {
    let quits_with_last_window = cfg!(any(target_os = "linux", target_os = "freebsd", windows))
        || *GeneralSettings::as_ref(ctx).quit_on_last_window_closed;
    quits_with_last_window && ctx.window_ids().count() == 1
}

/// The directories a remote client starts browsing files from.
pub(super) fn file_roots(ctx: &AppContext) -> FsRoots {
    let home = dirs::home_dir().unwrap_or_default();
    files::roots(&home.to_string_lossy(), &snapshot::workspace(ctx))
}

/// The directories of the folders, in the order of the workspace and each once.
pub(super) fn folder_directories(ctx: &AppContext) -> Vec<String> {
    let mut directories = Vec::new();
    let folders = snapshot::workspace(ctx)
        .windows
        .into_iter()
        .flat_map(|window| window.folders);
    for folder in folders {
        if !directories.contains(&folder.directory) {
            directories.push(folder.directory);
        }
    }
    directories
}

/// The working trees under a folder, found the way the source control panel finds them, which
/// does not look through a folder that holds the whole home directory.
pub(super) fn working_trees(folder: &Path) -> Vec<PathBuf> {
    #[cfg(feature = "local_fs")]
    {
        if contains_home_directory(folder) {
            return Vec::new();
        }
        find_nested_repos(folder)
            .into_iter()
            .map(|repo| repo.root)
            .collect()
    }
    #[cfg(not(feature = "local_fs"))]
    {
        let _ = folder;
        Vec::new()
    }
}

/// The colours and the font the app draws its terminals with.
pub(super) fn theme(ctx: &AppContext) -> Theme {
    let appearance = Appearance::as_ref(ctx);
    let theme = appearance.theme();
    let colors = theme.terminal_colors();
    let eight = |colors: &AnsiColors| {
        [
            colors.black,
            colors.red,
            colors.green,
            colors.yellow,
            colors.blue,
            colors.magenta,
            colors.cyan,
            colors.white,
        ]
        .map(|color| format_hex_color(color.into()))
    };
    let [normal, bright] = [eight(&colors.normal), eight(&colors.bright)];
    let mut ansi = normal.into_iter().chain(bright);
    let selection = theme.text_selection_color().into_solid();
    Theme {
        terminal: TerminalColors {
            background: format_hex_color(theme.background().into_solid()),
            foreground: format_hex_color(theme.foreground().into_solid()),
            cursor: format_hex_color(theme.cursor().into_solid()),
            selection: format_hex_color_with_alpha(selection),
            ansi: std::array::from_fn(|_| ansi.next().unwrap_or_default()),
        },
        font_family: ctx
            .font_cache()
            .load_family_name_from_id(appearance.monospace_font_family())
            .unwrap_or_default(),
        font_size: appearance.monospace_font_size(),
    }
}

fn format_hex_color_with_alpha(color: ColorU) -> String {
    format!("{}{:02X}", format_hex_color(color), color.a)
}
