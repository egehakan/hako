use std::fs;

use settings::Setting as _;
use warp_core::features::FeatureFlag;
use warp_errors::report_if_error;
use warpui::{App, SingletonEntity};

use super::{RemoteCloseRefusal, RemoteTabPlace};
use crate::terminal::general_settings::GeneralSettings;
use crate::workspace::tab_folders::normalized_directory;
use crate::workspace::tab_settings::TabSettings;
use crate::workspace::view::tests::{initialize_app, mock_workspace};

#[test]
fn a_remote_close_closes_the_tab_without_asking() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.update(|ctx| {
            GeneralSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.confirm_before_closing.set_value(true, ctx));
            });
        });
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.add_terminal_tab(false, ctx);
            let kept = workspace.tabs[0].pane_group.id();
            let closed = workspace.tabs[1].pane_group.id();

            assert_eq!(workspace.close_tab_for_remote(closed, ctx), Ok(()));

            assert!(!workspace.is_close_confirmation_open());
            let remaining: Vec<_> = workspace.tab_views().map(|tab| tab.id()).collect();
            assert_eq!(remaining, [kept]);
        });
    });
}

#[test]
fn a_remote_close_of_an_unknown_tab_closes_nothing() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.add_terminal_tab(false, ctx);
            let not_a_tab = ctx.view_id();

            assert_eq!(
                workspace.close_tab_for_remote(not_a_tab, ctx),
                Err(RemoteCloseRefusal::NoSuchTab)
            );

            assert_eq!(workspace.tab_count(), 2);
        });
    });
}

#[test]
fn a_remote_tab_for_a_folder_opens_in_that_folder_and_becomes_active() {
    let _grouped_tabs_guard = FeatureFlag::GroupedTabs.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
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

            let opened = workspace
                .open_tab_for_remote(RemoteTabPlace::Folder(group_id), ctx)
                .expect("a tab opens in a folder whose directory exists");

            assert_eq!(workspace.active_tab_pane_group().id(), opened.id());
            let active_tab = &workspace.tabs[workspace.active_tab_index()];
            assert_eq!(active_tab.group_id, Some(group_id));
        });
    });
}

#[test]
fn a_remote_tab_for_a_folder_whose_directory_is_gone_does_not_open() {
    let _grouped_tabs_guard = FeatureFlag::GroupedTabs.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
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
            fs::remove_dir(&directory).expect("remove project dir");

            let opened = workspace.open_tab_for_remote(RemoteTabPlace::Folder(group_id), ctx);

            assert!(opened.is_none());
            assert_eq!(workspace.tab_count(), 1);
        });
    });
}
