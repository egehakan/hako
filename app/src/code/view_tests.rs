use super::{PreviewPlacement, preview_placement};
use crate::code::buffer_location::LocalOrRemotePath;

fn local(path: &str) -> LocalOrRemotePath {
    LocalOrRemotePath::Local(path.into())
}

fn placement(tabs: &[(Option<LocalOrRemotePath>, bool)], opened: &str) -> PreviewPlacement {
    preview_placement(
        tabs.iter()
            .map(|(location, preview)| (location.as_ref(), *preview)),
        &local(opened),
    )
}

#[test]
fn test_first_preview_gets_a_new_tab() {
    assert_eq!(placement(&[], "/repo/a.rs"), PreviewPlacement::NewTab);
    assert_eq!(
        placement(&[(Some(local("/repo/kept.rs")), false)], "/repo/a.rs"),
        PreviewPlacement::NewTab
    );
}

#[test]
fn test_next_preview_replaces_the_preview_tab() {
    let tabs = [
        (Some(local("/repo/kept.rs")), false),
        (Some(local("/repo/previewed.rs")), true),
        (Some(local("/repo/other.rs")), false),
    ];

    assert_eq!(
        placement(&tabs, "/repo/a.rs"),
        PreviewPlacement::ReplacePreview(1)
    );
}

#[test]
fn test_file_with_a_kept_tab_is_shown_in_it_and_leaves_the_preview_alone() {
    let tabs = [
        (Some(local("/repo/previewed.rs")), true),
        (Some(local("/repo/kept.rs")), false),
    ];

    assert_eq!(
        placement(&tabs, "/repo/kept.rs"),
        PreviewPlacement::ExistingTab(1)
    );
}

#[test]
fn test_previewed_file_is_shown_in_its_own_tab() {
    let tabs = [
        (Some(local("/repo/kept.rs")), false),
        (Some(local("/repo/previewed.rs")), true),
    ];

    assert_eq!(
        placement(&tabs, "/repo/previewed.rs"),
        PreviewPlacement::ExistingTab(1)
    );
}

#[test]
fn test_untitled_tabs_are_never_taken_for_the_opened_file() {
    let tabs = [(None, false), (None, true)];

    assert_eq!(
        placement(&tabs, "/repo/a.rs"),
        PreviewPlacement::ReplacePreview(1)
    );
}
