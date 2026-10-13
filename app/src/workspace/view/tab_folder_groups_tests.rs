use std::fs;
use std::path::{Path, PathBuf};

use settings::Setting as _;
use tempfile::TempDir;
use warp_core::features::FeatureFlag;
use warp_errors::report_if_error;
use warpui::{App, SingletonEntity, TypedActionView, ViewContext};

use super::resolve_folder_for_start_directory;
use crate::app_state::{
    LeafContents, LeafSnapshot, PaneNodeSnapshot, TabSnapshot, TerminalPaneSnapshot,
};
use crate::pane_group::{self, NewTerminalOptions, PanesLayout};
use crate::tab::SelectedTabColor;
use crate::workspace::tab_folders::{
    TabFolders, TabGroupIcon, TabListItem, normalized_directory, parse_hex_color,
};
use crate::workspace::tab_group::{TabGroup, TabGroupId};
use crate::workspace::tab_settings::TabSettings;
use crate::workspace::util::TabMovement;
use crate::workspace::view::tests::{initialize_app, mock_workspace, restored_workspace};
use crate::workspace::{Workspace, WorkspaceAction};

/// Directories for the tests: a repository with a nested package, and two plain directories.
struct Fixture {
    _temp: TempDir,
    repo: PathBuf,
    package: PathBuf,
    plain: PathBuf,
    other: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = normalized_directory(temp.path());
        let repo = root.join("repo");
        let package = repo.join("packages").join("api");
        let plain = root.join("plain");
        let other = root.join("other");
        for directory in [&package, &plain, &other] {
            fs::create_dir_all(directory).expect("fixture dir");
        }
        fs::create_dir(repo.join(".git")).expect("git dir");
        Self {
            _temp: temp,
            repo,
            package,
            plain,
            other,
        }
    }
}

/// Runs `test` in an app that shows tabs in the sidebar, which is where folders live.
fn with_tab_sidebar(test: impl FnOnce(&mut App) + 'static) {
    let _vertical_tabs_guard = FeatureFlag::VerticalTabs.override_enabled(true);
    let _grouped_tabs_guard = FeatureFlag::GroupedTabs.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.update(|ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(true, ctx));
            });
        });
        test(&mut app);
    });
}

fn open_terminal_in(workspace: &mut Workspace, directory: &Path, ctx: &mut ViewContext<Workspace>) {
    workspace.add_tab_with_pane_layout(
        PanesLayout::SingleTerminal(Box::new(
            NewTerminalOptions::default().with_initial_directory_opt(Some(directory.to_path_buf())),
        )),
        Default::default(),
        None,
        ctx,
    );
}

fn folder_of_active_tab(workspace: &Workspace) -> Option<PathBuf> {
    let group_id = workspace.tabs[workspace.active_tab_index].group_id?;
    workspace.tab_groups[&group_id].directory.clone()
}

fn folder_id(workspace: &Workspace, directory: &Path) -> TabGroupId {
    workspace
        .tab_groups
        .values()
        .find(|group| group.directory.as_deref() == Some(directory))
        .map(|group| group.id)
        .unwrap_or_else(|| panic!("no folder for {}", directory.display()))
}

fn folders_in_order(workspace: &Workspace) -> Vec<PathBuf> {
    workspace
        .ordered_tab_group_ids()
        .iter()
        .filter_map(|id| workspace.tab_groups[id].directory.clone())
        .collect()
}

fn saved_terminal_tab(uuid: u8, cwd: &Path, stays_ungrouped: Option<bool>) -> TabSnapshot {
    TabSnapshot {
        custom_title: None,
        root: PaneNodeSnapshot::Leaf(LeafSnapshot {
            is_focused: true,
            custom_vertical_tabs_title: None,
            contents: LeafContents::Terminal(TerminalPaneSnapshot {
                uuid: vec![uuid],
                cwd: Some(cwd.to_string_lossy().into_owned()),
                shell_launch_data: None,
                is_active: true,
                is_read_only: false,
                input_config: None,
                llm_model_override: None,
                active_profile_id: None,
                conversation_ids_to_restore: vec![],
                active_conversation_id: None,
            }),
        }),
        default_directory_color: None,
        selected_color: SelectedTabColor::default(),
        left_panel: None,
        right_panel: None,
        group_id: None,
        pinned: false,
        stays_ungrouped,
    }
}

#[test]
fn a_start_directory_resolves_to_the_deepest_folder_inside_its_repository() {
    let fixture = Fixture::new();
    let repo_id = TabGroupId::new();
    let package_id = TabGroupId::new();
    let above_id = TabGroupId::new();
    let above = fixture.repo.parent().expect("parent").to_path_buf();
    let folders = || {
        [
            (above_id, above.as_path()),
            (repo_id, fixture.repo.as_path()),
            (package_id, fixture.package.as_path()),
        ]
        .into_iter()
    };

    assert_eq!(
        resolve_folder_for_start_directory(&fixture.package.join("src"), folders()),
        Ok(package_id)
    );
    assert_eq!(
        resolve_folder_for_start_directory(&fixture.repo.join("docs"), folders()),
        Ok(repo_id)
    );
    // A folder above the repository does not swallow the repository's tabs.
    assert_eq!(
        resolve_folder_for_start_directory(
            &fixture.repo,
            [(above_id, above.as_path())].into_iter()
        ),
        Err(fixture.repo.clone())
    );
    assert_eq!(
        resolve_folder_for_start_directory(&fixture.plain, folders()),
        Err(fixture.plain.clone())
    );
}

#[test]
fn a_new_terminal_tab_is_filed_under_the_folder_of_its_directory() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let workspace = mock_workspace(app);
        workspace.update(app, |workspace, ctx| {
            // The tab the window opened with is in a folder too.
            assert!(folder_of_active_tab(workspace).is_some());
            let folders_at_start = workspace.tab_groups.len();

            open_terminal_in(workspace, &fixture.package, ctx);
            assert_eq!(folder_of_active_tab(workspace), Some(fixture.repo.clone()));

            // Another directory of the same repository joins the same folder.
            open_terminal_in(workspace, &fixture.repo, ctx);
            assert_eq!(folder_of_active_tab(workspace), Some(fixture.repo.clone()));
            assert_eq!(workspace.tab_groups.len(), folders_at_start + 1);

            open_terminal_in(workspace, &fixture.plain, ctx);
            assert_eq!(folder_of_active_tab(workspace), Some(fixture.plain.clone()));
            assert_eq!(workspace.tab_groups.len(), folders_at_start + 2);

            let repo_id = folder_id(workspace, &fixture.repo);
            let repo_tabs: Vec<usize> =
                super::group_member_indices(&workspace.tabs, repo_id).collect();
            assert_eq!(repo_tabs.len(), 2);
            assert_eq!(
                repo_tabs[1],
                repo_tabs[0] + 1,
                "a folder's tabs sit together"
            );

            let registered = TabFolders::as_ref(ctx);
            assert!(registered.folder(&fixture.repo).is_some());
            assert!(registered.folder(&fixture.plain).is_some());
        });
    });
}

#[test]
fn tabs_in_the_horizontal_tab_bar_are_not_filed_under_folders() {
    let _grouped_tabs_guard = FeatureFlag::GroupedTabs.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let fixture = Fixture::new();
        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            open_terminal_in(workspace, &fixture.repo, ctx);
            assert!(workspace.tab_groups.is_empty());
            assert!(workspace.tabs.iter().all(|tab| tab.group_id.is_none()));
        });
    });
}

#[test]
fn a_tab_stays_in_its_folder_when_its_terminal_changes_directory() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let workspace = mock_workspace(app);
        workspace.update(app, |workspace, ctx| {
            open_terminal_in(workspace, &fixture.repo, ctx);
            open_terminal_in(workspace, &fixture.plain, ctx);
            let groups_before: Vec<_> = workspace.tabs.iter().map(|tab| tab.group_id).collect();

            // This is the event a terminal sends when its working directory changes. Folders
            // are resolved when a tab is opened and at no other time.
            for index in 0..workspace.tab_count() {
                let pane_group = workspace.tabs[index].pane_group.clone();
                workspace.handle_file_tree_event(
                    pane_group,
                    &pane_group::Event::AppStateChanged,
                    ctx,
                );
            }

            let groups_after: Vec<_> = workspace.tabs.iter().map(|tab| tab.group_id).collect();
            assert_eq!(groups_after, groups_before);
        });
    });
}

#[test]
fn a_folder_keeps_its_place_when_its_last_tab_is_closed() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let workspace = mock_workspace(app);
        workspace.update(app, |workspace, ctx| {
            open_terminal_in(workspace, &fixture.repo, ctx);
            open_terminal_in(workspace, &fixture.plain, ctx);
            open_terminal_in(workspace, &fixture.other, ctx);
            let order_before = folders_in_order(workspace);
            let plain_id = folder_id(workspace, &fixture.plain);
            let plain_tab = super::group_member_indices(&workspace.tabs, plain_id)
                .next()
                .expect("the folder has a tab");

            workspace.close_tab(plain_tab, true, true, ctx);

            assert!(workspace.tab_groups.contains_key(&plain_id));
            assert_eq!(folders_in_order(workspace), order_before);
            let item = workspace
                .tab_list_items()
                .into_iter()
                .find(|item| matches!(item, TabListItem::Group { id, .. } if *id == plain_id));
            assert!(
                matches!(item, Some(TabListItem::Group { ref members, .. }) if members.is_empty())
            );

            // The folder is saved along with the window although it holds no tab.
            let snapshot = workspace.snapshot(ctx.window_id(), false, ctx);
            assert!(
                snapshot
                    .tab_groups
                    .iter()
                    .any(|group| group.directory.as_deref() == Some(fixture.plain.as_path()))
            );
        });
    });
}

#[test]
fn a_folder_added_again_is_the_one_already_there_and_opens_no_tab() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let workspace = mock_workspace(app);
        workspace.update(app, |workspace, ctx| {
            let tabs_before = workspace.tab_count();

            let plain_id = workspace.add_tab_folder_for_directory(fixture.plain.clone(), ctx);
            let again = workspace.add_tab_folder_for_directory(fixture.plain.clone(), ctx);

            assert_eq!(again, plain_id);
            let added: Vec<_> = folders_in_order(workspace)
                .into_iter()
                .filter(|directory| *directory == fixture.plain)
                .collect();
            assert_eq!(added, [fixture.plain.clone()]);
            assert_eq!(workspace.tab_count(), tabs_before);
        });
    });
}

#[test]
fn the_add_button_opens_a_tab_in_the_folder_it_belongs_to() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let workspace = mock_workspace(app);
        workspace.update(app, |workspace, ctx| {
            open_terminal_in(workspace, &fixture.repo, ctx);
            // A folder added by hand for a directory inside the repository.
            let package_id = workspace.add_tab_folder_for_directory(fixture.package.clone(), ctx);
            open_terminal_in(workspace, &fixture.other, ctx);
            let order_before = folders_in_order(workspace);
            let tabs_before = workspace.tab_count();

            workspace.handle_action(&WorkspaceAction::NewTabInGroup(package_id), ctx);

            assert_eq!(workspace.tab_count(), tabs_before + 1);
            assert_eq!(
                workspace.tabs[workspace.active_tab_index].group_id,
                Some(package_id)
            );
            // The folder stays between its neighbors now that it holds a tab.
            assert_eq!(folders_in_order(workspace), order_before);
            let other_id = folder_id(workspace, &fixture.other);
            let other_tab = super::group_member_indices(&workspace.tabs, other_id)
                .next()
                .expect("the folder has a tab");
            assert_eq!(workspace.active_tab_index + 1, other_tab);
        });
    });
}

#[test]
fn the_add_button_reports_a_folder_whose_directory_is_gone() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let workspace = mock_workspace(app);
        workspace.update(app, |workspace, ctx| {
            let plain_id = workspace.add_tab_folder_for_directory(fixture.plain.clone(), ctx);
            assert!(!workspace.tab_groups[&plain_id].directory_missing);
            fs::remove_dir(&fixture.plain).expect("remove dir");
            let tabs_before = workspace.tab_count();

            workspace.handle_action(&WorkspaceAction::NewTabInGroup(plain_id), ctx);

            assert_eq!(workspace.tab_count(), tabs_before);
            assert!(workspace.tab_groups[&plain_id].directory_missing);
        });
    });
}

#[test]
fn a_group_without_tabs_moves_among_the_other_groups() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let workspace = mock_workspace(app);
        workspace.update(app, |workspace, ctx| {
            open_terminal_in(workspace, &fixture.repo, ctx);
            let plain_id = workspace.add_tab_folder_for_directory(fixture.plain.clone(), ctx);
            let order = workspace.ordered_tab_group_ids();
            let slot = order.iter().position(|id| *id == plain_id).expect("slot");
            assert_eq!(slot, order.len() - 1);
            assert!(!workspace.can_move_tab_group_in_list(plain_id, TabMovement::Right));

            workspace.handle_action(&WorkspaceAction::MoveTabGroupUp(plain_id), ctx);
            let moved = workspace.ordered_tab_group_ids();
            assert_eq!(moved[slot - 1], plain_id);
            assert_eq!(moved[slot], order[slot - 1]);

            workspace.handle_action(&WorkspaceAction::MoveTabGroupDown(plain_id), ctx);
            assert_eq!(workspace.ordered_tab_group_ids(), order);
        });
    });
}

#[test]
fn a_tab_moved_into_a_group_without_tabs_leaves_the_group_where_it_is() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let workspace = mock_workspace(app);
        workspace.update(app, |workspace, ctx| {
            let plain_id = workspace.add_tab_folder_for_directory(fixture.plain.clone(), ctx);
            open_terminal_in(workspace, &fixture.repo, ctx);
            open_terminal_in(workspace, &fixture.repo, ctx);
            let order_before = workspace.ordered_tab_group_ids();
            let tab_index = workspace.active_tab_index;

            workspace.handle_action(
                &WorkspaceAction::MoveTabToGroup {
                    tab_index,
                    group_id: plain_id,
                },
                ctx,
            );

            assert_eq!(workspace.ordered_tab_group_ids(), order_before);
            assert_eq!(
                super::group_member_indices(&workspace.tabs, plain_id).count(),
                1
            );
        });
    });
}

#[test]
fn a_tab_taken_out_of_its_folder_is_not_filed_again() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let workspace = mock_workspace(app);
        let snapshot = workspace.update(app, |workspace, ctx| {
            open_terminal_in(workspace, &fixture.repo, ctx);
            let tab_index = workspace.active_tab_index;
            workspace.handle_action(&WorkspaceAction::RemoveTabFromGroup(tab_index), ctx);
            let tab = &workspace.tabs[workspace.active_tab_index];
            assert_eq!(tab.group_id, None);
            assert!(tab.stays_ungrouped);
            // The folder it left is still there.
            assert!(
                workspace
                    .tab_groups
                    .values()
                    .any(|group| group.directory.as_deref() == Some(fixture.repo.as_path()))
            );
            workspace.snapshot(ctx.window_id(), false, ctx)
        });
        assert!(
            snapshot
                .tabs
                .iter()
                .any(|tab| tab.stays_ungrouped == Some(true))
        );
    });
}

#[test]
fn removing_a_folder_takes_it_out_of_every_window() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let first = mock_workspace(app);
        let plain_id = first.update(app, |workspace, ctx| {
            workspace.add_tab_folder_for_directory(fixture.plain.clone(), ctx)
        });
        let second = mock_workspace(app);
        second.update(app, |workspace, ctx| {
            // The second window shows the folder without having opened anything in it.
            let adopted = folder_id(workspace, &fixture.plain);
            assert_eq!(
                super::group_member_indices(&workspace.tabs, adopted).count(),
                0
            );
            open_terminal_in(workspace, &fixture.plain, ctx);
        });

        first.update(app, |workspace, ctx| {
            workspace.handle_action(&WorkspaceAction::RemoveTabGroup(plain_id), ctx);
            assert!(!workspace.tab_groups.contains_key(&plain_id));
            assert!(TabFolders::as_ref(ctx).folder(&fixture.plain).is_none());
        });
        second.read(app, |workspace, _| {
            assert!(
                workspace
                    .tab_groups
                    .values()
                    .all(|group| group.directory.as_deref() != Some(fixture.plain.as_path()))
            );
            // The tab that was open there is not closed behind the user's back.
            assert!(workspace.tabs.iter().any(|tab| tab.group_id.is_none()));
        });
    });
}

#[test]
fn a_new_window_lists_the_folders_in_the_order_of_the_last_one() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let first = mock_workspace(app);
        let order = first.update(app, |workspace, ctx| {
            workspace.add_tab_folder_for_directory(fixture.plain.clone(), ctx);
            let other_id = workspace.add_tab_folder_for_directory(fixture.other.clone(), ctx);
            workspace.handle_action(&WorkspaceAction::MoveTabGroupUp(other_id), ctx);
            folders_in_order(workspace)
        });
        assert_eq!(
            order[order.len() - 2..],
            [fixture.other.clone(), fixture.plain.clone()]
        );

        let second = mock_workspace(app);
        second.read(app, |workspace, _| {
            assert_eq!(folders_in_order(workspace), order);
        });
    });
}

#[test]
fn a_folder_looks_the_same_in_every_window() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let first = mock_workspace(app);
        let second = mock_workspace(app);
        let color = parse_hex_color("#1E90FF");
        let icon = TabGroupIcon::material("folder-api");
        first.update(app, |workspace, ctx| {
            let plain_id = workspace.add_tab_folder_for_directory(fixture.plain.clone(), ctx);
            workspace.set_tab_group_custom_color(plain_id, color, ctx);
            workspace.set_tab_group_icon(plain_id, icon.clone(), ctx);
        });

        second.read(app, |workspace, _| {
            let group = &workspace.tab_groups[&folder_id(workspace, &fixture.plain)];
            assert_eq!(group.custom_color, color);
            assert_eq!(group.icon, icon);
        });
    });
}

#[test]
fn folders_come_back_as_they_were_after_a_restart() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        let workspace = mock_workspace(app);
        let color = parse_hex_color("#C0FFEE");
        let icon = TabGroupIcon::text("🚀");
        let (snapshot, order) = workspace.update(app, |workspace, ctx| {
            open_terminal_in(workspace, &fixture.repo, ctx);
            let plain_id = workspace.add_tab_folder_for_directory(fixture.plain.clone(), ctx);
            open_terminal_in(workspace, &fixture.other, ctx);
            workspace.set_tab_group_custom_color(plain_id, color, ctx);
            workspace.set_tab_group_icon(plain_id, icon.clone(), ctx);
            let repo_id = folder_id(workspace, &fixture.repo);
            workspace.toggle_tab_group_collapsed(repo_id, ctx);
            workspace.tab_groups.get_mut(&repo_id).expect("group").name = Some("Mono".to_string());
            (
                workspace.snapshot(ctx.window_id(), false, ctx),
                folders_in_order(workspace),
            )
        });
        assert_eq!(snapshot.tab_groups.len(), order.len());

        let restored = restored_workspace(app, snapshot);
        restored.read(app, |workspace, _| {
            assert_eq!(folders_in_order(workspace), order);

            let plain = &workspace.tab_groups[&folder_id(workspace, &fixture.plain)];
            assert_eq!(plain.custom_color, color);
            assert_eq!(plain.icon, icon);
            assert_eq!(
                super::group_member_indices(&workspace.tabs, plain.id).count(),
                0
            );

            let repo = &workspace.tab_groups[&folder_id(workspace, &fixture.repo)];
            assert!(repo.collapsed);
            assert_eq!(repo.name.as_deref(), Some("Mono"));
            assert_eq!(
                super::group_member_indices(&workspace.tabs, repo.id).count(),
                1
            );
        });
    });
}

#[test]
fn tabs_saved_before_folders_are_filed_by_directory_when_restored() {
    with_tab_sidebar(|app| {
        let fixture = Fixture::new();
        // The shape of a window saved by a build without folders: no groups, and tabs that
        // carry nothing about having been ungrouped.
        let seed = mock_workspace(app);
        let mut snapshot = seed.update(app, |workspace, ctx| {
            workspace.snapshot(ctx.window_id(), false, ctx)
        });
        snapshot.tab_groups = Vec::new();
        snapshot.tabs = vec![
            saved_terminal_tab(1, &fixture.package, None),
            saved_terminal_tab(2, &fixture.plain, None),
            saved_terminal_tab(3, &fixture.repo, None),
            saved_terminal_tab(4, &fixture.other, Some(true)),
        ];
        snapshot.active_tab_index = 1;

        let restored = restored_workspace(app, snapshot);
        restored.read(app, |workspace, _| {
            assert_eq!(workspace.tab_count(), 4);
            let repo_id = folder_id(workspace, &fixture.repo);
            assert_eq!(
                super::group_member_indices(&workspace.tabs, repo_id).count(),
                2
            );
            assert_eq!(folder_of_active_tab(workspace), Some(fixture.plain.clone()));
            // The tab that was taken out of its group by hand stays out.
            let loose: Vec<_> = workspace
                .tabs
                .iter()
                .filter(|tab| tab.group_id.is_none())
                .collect();
            assert_eq!(loose.len(), 1);
            assert!(loose[0].stays_ungrouped);
        });
    });
}

#[test]
fn a_plain_group_still_goes_when_its_last_tab_is_moved_out() {
    with_tab_sidebar(|app| {
        let workspace = mock_workspace(app);
        workspace.update(app, |workspace, ctx| {
            let group = TabGroup::new();
            let group_id = group.id;
            workspace.tab_groups.insert(group_id, group);
            let folder = workspace.tabs[0]
                .group_id
                .expect("the first tab is in a folder");
            workspace.tabs[0].group_id = Some(group_id);

            workspace.handle_action(&WorkspaceAction::RemoveTabFromGroup(0), ctx);

            assert!(!workspace.tab_groups.contains_key(&group_id));
            assert!(workspace.tab_groups.contains_key(&folder));
        });
    });
}
