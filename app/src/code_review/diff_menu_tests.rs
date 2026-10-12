use super::*;

fn target(label: &str, detail: Option<&str>, section: DiffTargetSection) -> DiffTarget {
    let base = match section {
        DiffTargetSection::WorkingTree => DiffTargetBase::UncommittedChanges,
        DiffTargetSection::Branches => DiffTargetBase::Branch(label.to_string()),
        DiffTargetSection::RemoteBranches | DiffTargetSection::Tags => {
            DiffTargetBase::Revision(label.to_string())
        }
        DiffTargetSection::Commits => {
            DiffTargetBase::Revision("0123456789abcdef0123456789abcdef01234567".to_string())
        }
    };
    DiffTarget {
        label: label.to_string(),
        detail: detail.map(str::to_string),
        section,
        base,
        is_selected: false,
    }
}

fn all_targets() -> Vec<DiffTarget> {
    vec![
        target("Uncommitted changes", None, DiffTargetSection::WorkingTree),
        target("main", None, DiffTargetSection::Branches),
        target("feature/login", None, DiffTargetSection::Branches),
        target(
            "origin/feature/login",
            Some("abc1234 \u{b7} 2 days ago"),
            DiffTargetSection::RemoteBranches,
        ),
        target(
            "v1.0",
            Some("def5678 \u{b7} 5 months ago"),
            DiffTargetSection::Tags,
        ),
        target(
            "Fix the login form",
            Some("9a8b7c6 \u{b7} Ada Lovelace \u{b7} 3 hours ago"),
            DiffTargetSection::Commits,
        ),
        target(
            "Update dependencies",
            Some("1f2e3d4 \u{b7} Grace Hopper \u{b7} 2 days ago"),
            DiffTargetSection::Commits,
        ),
    ]
}

fn offering_revisions() -> MenuRowOptions {
    MenuRowOptions {
        offers_revisions: true,
        ..Default::default()
    }
}

/// The rows as text: headings in brackets, targets by label.
fn describe(rows: &[MenuRow], targets: &[DiffTarget]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            MenuRow::Header(title) => format!("[{title}]"),
            MenuRow::Target { index, .. } => targets[*index].label.clone(),
            MenuRow::TypedRevision => "<typed>".to_string(),
            MenuRow::Error(message) => format!("error: {message}"),
            MenuRow::Note(note) => format!("note: {note}"),
        })
        .collect()
}

#[test]
fn an_empty_search_lists_every_target_under_its_heading() {
    let targets = all_targets();
    let rows = menu_rows(&targets, "", &offering_revisions());
    assert_eq!(describe(&rows, &targets), [
        "Uncommitted changes",
        "[Branches]",
        "main",
        "feature/login",
        "[Remote branches]",
        "origin/feature/login",
        "[Tags]",
        "v1.0",
        "[Recent commits]",
        "Fix the login form",
        "Update dependencies",
    ]);
}

#[test]
fn a_search_filters_across_every_kind_and_drops_empty_headings() {
    let targets = all_targets();
    let rows = menu_rows(&targets, "login", &MenuRowOptions::default());
    assert_eq!(describe(&rows, &targets), [
        "[Branches]",
        "feature/login",
        "[Remote branches]",
        "origin/feature/login",
        "[Recent commits]",
        "Fix the login form",
    ]);
}

#[test]
fn a_search_matches_commit_ids_authors_and_ages() {
    let targets = all_targets();
    let search = |query: &str| {
        let rows = menu_rows(&targets, query, &MenuRowOptions::default());
        describe(&rows, &targets)
    };

    assert_eq!(search("9a8b7"), ["[Recent commits]", "Fix the login form"]);
    assert_eq!(search("hopper"), ["[Recent commits]", "Update dependencies"]);
    assert_eq!(search("2 days"), [
        "[Remote branches]",
        "origin/feature/login",
        "[Recent commits]",
        "Update dependencies",
    ]);
    assert_eq!(search("DEF5678"), ["[Tags]", "v1.0"]);
}

#[test]
fn names_match_loosely_but_commit_messages_only_literally() {
    let targets = all_targets();
    // The letters of "flog" appear in order in the branch names and in the first commit's
    // message, which is too loose a match for prose.
    let rows = menu_rows(&targets, "flog", &MenuRowOptions::default());
    assert_eq!(describe(&rows, &targets), [
        "[Branches]",
        "feature/login",
        "[Remote branches]",
        "origin/feature/login",
    ]);
}

#[test]
fn a_matched_label_reports_the_characters_to_emphasize() {
    let targets = all_targets();
    let rows = menu_rows(&targets, "LOGIN", &MenuRowOptions::default());
    assert_eq!(rows[1], MenuRow::Target {
        index: 2,
        highlighted: vec![8, 9, 10, 11, 12],
    });

    // A match in the detail emphasizes nothing in the label.
    let rows = menu_rows(&targets, "hopper", &MenuRowOptions::default());
    assert_eq!(rows[1], MenuRow::Target {
        index: 6,
        highlighted: vec![],
    });
}

#[test]
fn typed_text_is_offered_as_a_revision_unless_a_target_has_that_name() {
    let targets = all_targets();

    let rows = menu_rows(&targets, "HEAD~3", &offering_revisions());
    assert_eq!(describe(&rows, &targets), ["<typed>"]);

    // Text that also matches targets comes after them.
    let rows = menu_rows(&targets, " v1 ", &offering_revisions());
    assert_eq!(describe(&rows, &targets), ["[Tags]", "v1.0", "<typed>"]);

    let rows = menu_rows(&targets, "v1.0", &offering_revisions());
    assert_eq!(describe(&rows, &targets), ["[Tags]", "v1.0"]);
    let rows = menu_rows(&targets, "main", &offering_revisions());
    assert!(!rows.contains(&MenuRow::TypedRevision));
}

#[test]
fn typed_text_is_not_offered_where_revisions_cannot_be_compared_with() {
    let targets = all_targets();
    let rows = menu_rows(&targets, "HEAD~3", &MenuRowOptions::default());
    assert_eq!(describe(&rows, &targets), ["note: No matches"]);
}

#[test]
fn a_failed_revision_shows_its_error_under_the_typed_text() {
    let targets = all_targets();
    let options = MenuRowOptions {
        offers_revisions: true,
        typed_revision: TypedRevisionState::Failed("Not a commit".to_string()),
        ..Default::default()
    };
    let rows = menu_rows(&targets, "nope", &options);
    assert_eq!(describe(&rows, &targets), ["<typed>", "error: Not a commit"]);

    let checking = MenuRowOptions {
        offers_revisions: true,
        typed_revision: TypedRevisionState::Checking,
        ..Default::default()
    };
    let rows = menu_rows(&targets, "nope", &checking);
    assert_eq!(describe(&rows, &targets), ["<typed>"]);
}

#[test]
fn a_listing_in_progress_is_noted_after_the_rows() {
    let targets = all_targets()[..3].to_vec();
    let options = MenuRowOptions {
        offers_revisions: true,
        is_loading_revisions: true,
        ..Default::default()
    };
    let rows = menu_rows(&targets, "", &options);
    assert_eq!(
        describe(&rows, &targets).last().map(String::as_str),
        Some("note: Listing remote branches, tags and commits\u{2026}")
    );
    assert_eq!(rows.len(), 5);
}

#[test]
fn keyboard_selection_skips_headings_errors_and_notes() {
    let targets = all_targets();
    let rows = menu_rows(&targets, "", &offering_revisions());
    // Rows: 0 target, 1 heading, 2 target, 3 target, 4 heading, 5 target, ...
    assert_eq!(next_selectable_row(&rows, 0, true), Some(2));
    assert_eq!(next_selectable_row(&rows, 3, true), Some(5));
    assert_eq!(next_selectable_row(&rows, 5, false), Some(3));
    assert_eq!(next_selectable_row(&rows, 2, false), Some(0));
    assert_eq!(next_selectable_row(&rows, 0, false), None);
    assert_eq!(next_selectable_row(&rows, rows.len() - 1, true), None);

    let options = MenuRowOptions {
        offers_revisions: true,
        typed_revision: TypedRevisionState::Failed("Not a commit".to_string()),
        ..Default::default()
    };
    let rows = menu_rows(&targets, "nope", &options);
    assert_eq!(next_selectable_row(&rows, 0, true), None);
    assert!(rows[0].is_selectable());
    assert!(!rows[1].is_selectable());
}
