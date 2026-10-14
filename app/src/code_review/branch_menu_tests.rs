use super::*;

const NOW: i64 = 2_000_000;

fn branch(name: &str, kind: GitBranchKind) -> GitBranch {
    GitBranch {
        name: name.to_owned(),
        kind,
        tip: "a".repeat(40),
        time: (NOW - 3 * 86_400) * 1000,
        subject: "Subject".to_owned(),
        upstream: None,
        ahead: None,
        behind: None,
        worktree: None,
    }
}

fn listed(branches: Vec<GitBranch>) -> BranchList {
    BranchList::Loaded(GitBranches {
        current: Some("main".to_owned()),
        detached: false,
        head: Some("a".repeat(40)),
        branches,
        truncated: false,
    })
}

fn fixture() -> BranchList {
    listed(vec![
        branch("main", GitBranchKind::Local),
        branch("feature/login", GitBranchKind::Local),
        GitBranch {
            worktree: Some("/work/app-hotfix".to_owned()),
            ..branch("hotfix", GitBranchKind::Local)
        },
        branch("origin/release", GitBranchKind::Remote),
    ])
}

fn plain(index: usize) -> BranchRow {
    BranchRow::Branch {
        index,
        highlighted: Vec::new(),
    }
}

#[test]
fn local_branches_are_listed_before_remote_ones_under_their_headings() {
    assert_eq!(
        branch_rows(&fixture(), "", MenuMode::Listing),
        [
            BranchRow::NewBranch,
            BranchRow::Header("Branches"),
            plain(0),
            plain(1),
            plain(2),
            BranchRow::Header("Remote branches"),
            plain(3),
        ]
    );
}

#[test]
fn the_search_keeps_the_branches_it_matches_and_their_headings() {
    assert_eq!(
        branch_rows(&fixture(), "RELEASE", MenuMode::Listing),
        [
            BranchRow::NewBranch,
            BranchRow::Header("Remote branches"),
            BranchRow::Branch {
                index: 3,
                highlighted: (7..14).collect(),
            },
        ]
    );
}

#[test]
fn a_name_typed_from_memory_still_finds_its_branch() {
    let rows = branch_rows(&fixture(), "ftlog", MenuMode::Listing);

    assert!(
        rows.iter()
            .any(|row| matches!(row, BranchRow::Branch { index: 1, .. })),
        "{rows:?}"
    );
}

#[test]
fn a_search_that_matches_nothing_says_so_and_still_offers_a_new_branch() {
    assert_eq!(
        branch_rows(&fixture(), "zzz", MenuMode::Listing),
        [BranchRow::NewBranch, BranchRow::Note(NO_MATCHES_NOTE)]
    );
}

#[test]
fn branches_that_are_still_being_listed_or_could_not_be_are_reported_in_the_list() {
    assert_eq!(
        branch_rows(&BranchList::Loading, "", MenuMode::Listing),
        [BranchRow::NewBranch, BranchRow::Note(LISTING_NOTE)]
    );
    assert_eq!(
        branch_rows(
            &BranchList::Failed("fatal: not a git\nrepository".to_owned()),
            "",
            MenuMode::Listing
        ),
        [
            BranchRow::NewBranch,
            BranchRow::Error("fatal: not a git repository".to_owned())
        ]
    );
}

#[test]
fn a_list_that_was_cut_short_says_that_more_branches_exist() {
    let BranchList::Loaded(mut branches) = fixture() else {
        unreachable!()
    };
    branches.truncated = true;

    let rows = branch_rows(&BranchList::Loaded(branches), "", MenuMode::Listing);

    assert_eq!(rows.last(), Some(&BranchRow::Note(TRUNCATED_NOTE)));
}

#[test]
fn naming_a_branch_offers_to_create_it_once_it_has_a_name() {
    assert_eq!(
        branch_rows(&fixture(), "  ", MenuMode::Naming),
        [BranchRow::Note(NAME_NOTE)]
    );
    assert_eq!(
        branch_rows(&fixture(), "topic", MenuMode::Naming),
        [BranchRow::Create]
    );
}

#[test]
fn the_current_branch_and_one_held_by_another_worktree_cannot_be_picked() {
    let list = fixture();

    assert!(!is_selectable(&plain(0), &list), "the current branch");
    assert!(is_selectable(&plain(1), &list));
    assert!(!is_selectable(&plain(2), &list), "held by a worktree");
    assert!(is_selectable(&plain(3), &list));
    assert!(is_selectable(&BranchRow::NewBranch, &list));
    assert!(is_selectable(&BranchRow::Create, &list));
    assert!(!is_selectable(&BranchRow::Header("Branches"), &list));
    assert!(!is_selectable(&plain(0), &BranchList::Loading));
}

#[test]
fn a_remote_branch_named_like_the_current_one_can_be_picked() {
    let list = listed(vec![
        branch("main", GitBranchKind::Local),
        branch("main", GitBranchKind::Remote),
    ]);

    assert!(!is_selectable(&plain(0), &list));
    assert!(is_selectable(&plain(1), &list));
}

#[test]
fn keyboard_selection_passes_over_rows_that_offer_nothing() {
    let list = fixture();
    let rows = branch_rows(&list, "", MenuMode::Listing);

    // From the way to a new branch, past the heading and the current branch.
    assert_eq!(next_selectable_row(&rows, &list, 0, true), Some(3));
    // The branch a worktree holds cannot be switched to, but it can be merged.
    assert_eq!(next_selectable_row(&rows, &list, 3, true), Some(4));
    assert_eq!(next_selectable_row(&rows, &list, 4, true), Some(6));
    assert_eq!(next_selectable_row(&rows, &list, 6, true), None);
    assert_eq!(next_selectable_row(&rows, &list, 6, false), Some(4));
    assert_eq!(next_selectable_row(&rows, &list, 3, false), Some(0));
}

#[test]
fn every_branch_but_the_current_one_can_be_merged_into_it() {
    let list = fixture();
    let target = |index| merge_target(&plain(index), &list);

    assert_eq!(target(0), None, "the current branch");
    assert_eq!(
        target(1),
        Some(("feature/login".to_owned(), GitMergeKind::Local))
    );
    assert_eq!(target(2), Some(("hotfix".to_owned(), GitMergeKind::Local)));
    assert_eq!(
        target(3),
        Some(("origin/release".to_owned(), GitMergeKind::Remote))
    );
    assert_eq!(merge_target(&BranchRow::NewBranch, &list), None);

    let BranchList::Loaded(mut detached) = fixture() else {
        unreachable!();
    };
    detached.current = None;
    detached.detached = true;
    assert_eq!(merge_target(&plain(1), &BranchList::Loaded(detached)), None);
}

#[test]
fn every_row_that_offers_a_merge_shows_its_button_and_the_focused_one_names_the_current_branch() {
    let list = fixture();
    let rows = branch_rows(&list, "", MenuMode::Listing);
    let labels = |is_focused| {
        rows.iter()
            .map(|row| merge_button_label(row, &list, is_focused))
            .collect::<Vec<_>>()
    };
    let merge = Some("Merge".to_owned());
    let merge_into_main = Some("Merge into main".to_owned());

    // New branch, heading, main, feature/login, hotfix, heading, origin/release.
    assert_eq!(
        labels(false),
        [None, None, None, merge.clone(), merge.clone(), None, merge]
    );
    assert_eq!(
        labels(true),
        [
            None,
            None,
            None,
            merge_into_main.clone(),
            merge_into_main.clone(),
            None,
            merge_into_main,
        ]
    );
    assert_eq!(
        merge_button_label(&plain(1), &BranchList::Loading, false),
        None
    );
}

#[test]
fn a_branch_shows_how_far_it_is_from_its_upstream_and_when_it_last_changed() {
    let tracked = GitBranch {
        upstream: Some("origin/main".to_owned()),
        ahead: Some(2),
        behind: Some(1),
        ..branch("main", GitBranchKind::Local)
    };
    let in_step = GitBranch {
        ahead: Some(0),
        behind: Some(0),
        ..tracked.clone()
    };

    assert_eq!(
        branch_detail(&tracked, NOW),
        "\u{2191}2 \u{2193}1 \u{b7} 3 days ago"
    );
    assert_eq!(branch_detail(&in_step, NOW), "3 days ago");
    assert_eq!(
        branch_detail(&branch("topic", GitBranchKind::Local), NOW),
        "3 days ago"
    );
}

#[test]
fn a_branch_held_by_another_worktree_says_where_it_is_checked_out() {
    let held = GitBranch {
        worktree: Some("/work/app-hotfix".to_owned()),
        ahead: Some(4),
        ..branch("hotfix", GitBranchKind::Local)
    };

    assert_eq!(branch_detail(&held, NOW), "checked out in /work/app-hotfix");
}
