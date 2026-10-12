use std::fs;

use settings::Setting as _;
use warp_core::features::FeatureFlag;
use warp_errors::report_if_error;
use warpui::{App, SingletonEntity, TypedActionView, ViewContext, ViewHandle};

use super::WindowCloseConfirmation;
use crate::pane_group::{Direction, PaneGroupAction};
use crate::terminal::general_settings::GeneralSettings;
use crate::workspace::close_confirmation::{CloseConfirmationEvent, CloseStakes};
use crate::workspace::tab_folders::normalized_directory;
use crate::workspace::tab_settings::TabSettings;
use crate::workspace::view::tests::{initialize_app, mock_workspace};
use crate::workspace::{Workspace, WorkspaceAction};

fn set_confirm_before_closing(app: &mut App, enabled: bool) {
    app.update(|ctx| {
        GeneralSettings::handle(ctx).update(ctx, |settings, ctx| {
            report_if_error!(settings.confirm_before_closing.set_value(enabled, ctx));
        });
    });
}

/// A workspace with three tabs in an app where every close is confirmed.
fn confirming_workspace(app: &mut App) -> ViewHandle<Workspace> {
    initialize_app(app);
    set_confirm_before_closing(app, true);
    let workspace = mock_workspace(app);
    workspace.update(app, |workspace, ctx| {
        workspace.add_terminal_tab(false, ctx);
        workspace.add_terminal_tab(false, ctx);
        assert_eq!(workspace.tab_count(), 3);
    });
    workspace
}

fn dialog_title(workspace: &Workspace, ctx: &ViewContext<Workspace>) -> String {
    assert!(workspace.is_close_confirmation_open());
    workspace
        .close_confirmation_dialog
        .as_ref(ctx)
        .text()
        .expect("the dialog has text while it is open")
        .title
        .clone()
}

fn answer(
    workspace: &mut Workspace,
    answer: CloseConfirmationEvent,
    ctx: &mut ViewContext<Workspace>,
) {
    workspace.handle_close_confirmation_event(&answer, ctx);
    assert!(!workspace.is_close_confirmation_open());
}

/// Checks that `request` closes nothing until it is confirmed: cancelling leaves `tabs_before`
/// tabs, and confirming a second request leaves `tabs_after`.
fn assert_asks_before_closing_tabs(
    request: WorkspaceAction,
    expected_title: &'static str,
    tabs_after: usize,
) {
    App::test((), |mut app| async move {
        let workspace = confirming_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace.activate_tab(0, ctx);
            let tabs_before = workspace.tab_count();

            workspace.handle_action(&request, ctx);
            assert_eq!(dialog_title(workspace, ctx), expected_title);
            assert_eq!(workspace.tab_count(), tabs_before);

            answer(workspace, CloseConfirmationEvent::Cancelled, ctx);
            assert_eq!(workspace.tab_count(), tabs_before);

            workspace.handle_action(&request, ctx);
            answer(workspace, CloseConfirmationEvent::Confirmed, ctx);
            assert_eq!(workspace.tab_count(), tabs_after);
        });
    });
}

#[test]
fn closing_a_tab_asks_first() {
    assert_asks_before_closing_tabs(WorkspaceAction::CloseTab(1), "Close this tab?", 2);
}

#[test]
fn closing_the_active_tab_asks_first() {
    assert_asks_before_closing_tabs(WorkspaceAction::CloseActiveTab, "Close this tab?", 2);
}

#[test]
fn closing_the_other_tabs_asks_first() {
    assert_asks_before_closing_tabs(WorkspaceAction::CloseOtherTabs(0), "Close 2 tabs?", 1);
}

#[test]
fn closing_the_non_active_tabs_asks_first() {
    assert_asks_before_closing_tabs(WorkspaceAction::CloseNonActiveTabs, "Close 2 tabs?", 1);
}

#[test]
fn closing_the_tabs_to_the_right_asks_first() {
    assert_asks_before_closing_tabs(WorkspaceAction::CloseTabsRight(0), "Close 2 tabs?", 1);
    assert_asks_before_closing_tabs(WorkspaceAction::CloseTabsRight(1), "Close this tab?", 2);
    assert_asks_before_closing_tabs(
        WorkspaceAction::CloseTabsRightActiveTab,
        "Close 2 tabs?",
        1,
    );
}

#[test]
fn a_cancelled_close_leaves_the_same_tabs_in_the_same_order() {
    App::test((), |mut app| async move {
        let workspace = confirming_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace.activate_tab(1, ctx);
            let before: Vec<_> = workspace
                .tabs
                .iter()
                .map(|tab| tab.pane_group.id())
                .collect();
            let snapshot_before = workspace.snapshot(ctx.window_id(), false, ctx);

            workspace.handle_action(&WorkspaceAction::CloseOtherTabs(1), ctx);
            answer(workspace, CloseConfirmationEvent::Cancelled, ctx);

            let after: Vec<_> = workspace
                .tabs
                .iter()
                .map(|tab| tab.pane_group.id())
                .collect();
            assert_eq!(after, before);
            assert_eq!(workspace.active_tab_index(), 1);
            // What would be saved for the next launch is what it was before the request.
            let snapshot_after = workspace.snapshot(ctx.window_id(), false, ctx);
            assert_eq!(snapshot_after.tabs.len(), snapshot_before.tabs.len());
            assert_eq!(snapshot_after.active_tab_index, 1);
        });
    });
}

#[test]
fn a_request_made_while_the_dialog_is_open_does_not_replace_it() {
    App::test((), |mut app| async move {
        let workspace = confirming_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace.handle_action(&WorkspaceAction::CloseTab(2), ctx);
            workspace.handle_action(&WorkspaceAction::CloseOtherTabs(0), ctx);
            assert_eq!(dialog_title(workspace, ctx), "Close this tab?");

            answer(workspace, CloseConfirmationEvent::Confirmed, ctx);
            assert_eq!(workspace.tab_count(), 2);
        });
    });
}

#[test]
fn the_tab_confirmed_is_the_one_closed_even_if_tabs_moved_meanwhile() {
    App::test((), |mut app| async move {
        let workspace = confirming_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let doomed = workspace.tabs[2].pane_group.id();
            workspace.handle_action(&WorkspaceAction::CloseTab(2), ctx);
            workspace.move_tab_to_index(2, 0, ctx);

            answer(workspace, CloseConfirmationEvent::Confirmed, ctx);
            assert_eq!(workspace.tab_count(), 2);
            assert!(
                workspace
                    .tabs
                    .iter()
                    .all(|tab| tab.pane_group.id() != doomed)
            );
        });
    });
}

#[test]
fn closing_a_pane_asks_first() {
    App::test((), |mut app| async move {
        let workspace = confirming_workspace(&mut app);
        let (pane_group, pane_id) = workspace.update(&mut app, |workspace, ctx| {
            let pane_group = workspace.get_pane_group_view(1).unwrap().clone();
            let pane_id = pane_group.update(ctx, |view, ctx| {
                view.handle_action(&PaneGroupAction::Add(Direction::Right), ctx);
                assert_eq!(view.pane_count(), 2);
                view.pane_id_by_index(0).unwrap()
            });
            (pane_group, pane_id)
        });

        pane_group.update(&mut app, |view, ctx| {
            view.handle_action(&PaneGroupAction::Remove(pane_id), ctx);
        });
        workspace.update(&mut app, |workspace, ctx| {
            assert_eq!(dialog_title(workspace, ctx), "Close this pane?");
            answer(workspace, CloseConfirmationEvent::Cancelled, ctx);
        });
        pane_group.read(&app, |view, _| assert_eq!(view.visible_pane_count(), 2));

        pane_group.update(&mut app, |view, ctx| {
            view.handle_action(&PaneGroupAction::Remove(pane_id), ctx);
        });
        workspace.update(&mut app, |workspace, ctx| {
            answer(workspace, CloseConfirmationEvent::Confirmed, ctx);
            assert_eq!(workspace.tab_count(), 3);
        });
        pane_group.read(&app, |view, _| assert_eq!(view.visible_pane_count(), 1));
    });
}

#[test]
fn closing_the_only_pane_of_a_tab_asks_about_the_tab() {
    App::test((), |mut app| async move {
        let workspace = confirming_workspace(&mut app);
        let pane_group =
            workspace.read(&app, |workspace, _| workspace.get_pane_group_view(1).unwrap().clone());

        pane_group.update(&mut app, |view, ctx| {
            view.handle_action(&PaneGroupAction::RemoveActive, ctx);
        });
        workspace.update(&mut app, |workspace, ctx| {
            assert_eq!(dialog_title(workspace, ctx), "Close this tab?");
            assert_eq!(workspace.tab_count(), 3);
            answer(workspace, CloseConfirmationEvent::Confirmed, ctx);
        });
        workspace.read(&app, |workspace, _| assert_eq!(workspace.tab_count(), 2));
    });
}

#[test]
fn closing_a_group_asks_first_and_keeps_a_folder() {
    let _vertical_tabs_guard = FeatureFlag::VerticalTabs.override_enabled(true);
    let _grouped_tabs_guard = FeatureFlag::GroupedTabs.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        set_confirm_before_closing(&mut app, true);
        app.update(|ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(true, ctx));
            });
        });
        let temp = tempfile::tempdir().expect("temp dir");
        let directory = normalized_directory(temp.path()).join("project");
        fs::create_dir_all(&directory).expect("project dir");

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let group_id = workspace.add_tab_folder_for_directory(directory.clone(), ctx);
            workspace.handle_action(&WorkspaceAction::NewTabInGroup(group_id), ctx);
            workspace.handle_action(&WorkspaceAction::NewTabInGroup(group_id), ctx);
            assert_eq!(workspace.tab_count(), 3);

            workspace.handle_action(&WorkspaceAction::CloseTabGroup(group_id), ctx);
            assert_eq!(
                dialog_title(workspace, ctx),
                "Close all tabs in \"project\"?"
            );
            answer(workspace, CloseConfirmationEvent::Cancelled, ctx);
            assert_eq!(workspace.tab_count(), 3);

            workspace.handle_action(&WorkspaceAction::CloseTabGroup(group_id), ctx);
            answer(workspace, CloseConfirmationEvent::Confirmed, ctx);
            assert_eq!(workspace.tab_count(), 1);
            assert!(workspace.tab_groups.contains_key(&group_id));

            // Removing the group is asked about as well, although nothing is open in it.
            workspace.handle_action(&WorkspaceAction::RemoveTabGroup(group_id), ctx);
            assert_eq!(
                dialog_title(workspace, ctx),
                "Remove the group \"project\"?"
            );
            answer(workspace, CloseConfirmationEvent::Cancelled, ctx);
            assert!(workspace.tab_groups.contains_key(&group_id));

            workspace.handle_action(&WorkspaceAction::RemoveTabGroup(group_id), ctx);
            answer(workspace, CloseConfirmationEvent::Confirmed, ctx);
            assert!(!workspace.tab_groups.contains_key(&group_id));
            assert_eq!(workspace.tab_count(), 1);
        });
    });
}

#[test]
fn removing_a_group_closes_its_tabs_after_one_confirmation() {
    let _vertical_tabs_guard = FeatureFlag::VerticalTabs.override_enabled(true);
    let _grouped_tabs_guard = FeatureFlag::GroupedTabs.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        set_confirm_before_closing(&mut app, true);
        app.update(|ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(true, ctx));
            });
        });
        let temp = tempfile::tempdir().expect("temp dir");
        let directory = normalized_directory(temp.path());

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let group_id = workspace.add_tab_folder_for_directory(directory.clone(), ctx);
            workspace.handle_action(&WorkspaceAction::NewTabInGroup(group_id), ctx);
            workspace.handle_action(&WorkspaceAction::NewTabInGroup(group_id), ctx);

            workspace.handle_action(&WorkspaceAction::RemoveTabGroup(group_id), ctx);
            assert!(dialog_title(workspace, ctx).starts_with("Remove the group"));
            assert_eq!(workspace.tab_count(), 3);

            answer(workspace, CloseConfirmationEvent::Confirmed, ctx);
            assert_eq!(workspace.tab_count(), 1);
            assert!(!workspace.tab_groups.contains_key(&group_id));
        });
    });
}

#[test]
fn closing_the_window_asks_first() {
    App::test((), |mut app| async move {
        let workspace = confirming_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            assert_eq!(
                workspace.confirm_window_close(CloseStakes::default(), ctx),
                WindowCloseConfirmation::Asked
            );
            assert_eq!(dialog_title(workspace, ctx), "Close this window?");

            answer(workspace, CloseConfirmationEvent::Cancelled, ctx);
            assert!(!workspace.window_close_confirmed);
            assert_eq!(workspace.tab_count(), 3);

            // Asked again the next time, and let through only once it has been confirmed.
            assert_eq!(
                workspace.confirm_window_close(CloseStakes::default(), ctx),
                WindowCloseConfirmation::Asked
            );
            answer(workspace, CloseConfirmationEvent::Confirmed, ctx);
            assert_eq!(
                workspace.confirm_window_close(CloseStakes::default(), ctx),
                WindowCloseConfirmation::Confirmed
            );
            assert!(!workspace.window_close_confirmed);
        });
    });
}

#[test]
fn closing_the_last_tab_asks_once_for_the_tab_and_the_window() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        set_confirm_before_closing(&mut app, true);
        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            assert_eq!(workspace.tab_count(), 1);
            workspace.handle_action(&WorkspaceAction::CloseTab(0), ctx);
            assert_eq!(dialog_title(workspace, ctx), "Close this tab?");
            let body = workspace
                .close_confirmation_dialog
                .as_ref(ctx)
                .text()
                .expect("dialog text")
                .body
                .clone();
            assert!(body.contains("the window closes with it"), "{body}");

            answer(workspace, CloseConfirmationEvent::Confirmed, ctx);
            // The window's own close request passes without a second dialog.
            assert_eq!(
                workspace.confirm_window_close(CloseStakes::default(), ctx),
                WindowCloseConfirmation::Confirmed
            );
        });
    });
}

#[test]
fn nothing_is_asked_when_the_setting_is_off() {
    App::test((), |mut app| async move {
        let workspace = confirming_workspace(&mut app);
        set_confirm_before_closing(&mut app, false);
        let pane_group = workspace.update(&mut app, |workspace, ctx| {
            workspace.handle_action(&WorkspaceAction::CloseTab(2), ctx);
            assert!(!workspace.is_close_confirmation_open());
            assert_eq!(workspace.tab_count(), 2);

            assert_eq!(
                workspace.confirm_window_close(CloseStakes::default(), ctx),
                WindowCloseConfirmation::NotNeeded
            );
            assert!(!workspace.is_close_confirmation_open());
            workspace.get_pane_group_view(1).unwrap().clone()
        });

        pane_group.update(&mut app, |view, ctx| {
            view.handle_action(&PaneGroupAction::RemoveActive, ctx);
        });
        workspace.read(&app, |workspace, _| {
            assert!(!workspace.is_close_confirmation_open());
            assert_eq!(workspace.tab_count(), 1);
        });
    });
}
