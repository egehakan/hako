use super::{DiffMode, DiffState, DiffStateModel, RevisionComparison, display_revision};

#[test]
fn new_for_test_creates_local_variant() {
    warpui::App::test((), |mut app| async move {
        let handle = app.add_model(DiffStateModel::new_for_test);
        handle.read(&app, |model, _ctx| {
            assert!(matches!(model, DiffStateModel::Local(_)));
        });
    });
}

#[test]
fn get_returns_not_in_repository_for_test_model() {
    warpui::App::test((), |mut app| async move {
        let handle = app.add_model(DiffStateModel::new_for_test);
        let state = handle.read(&app, |model, ctx| model.get(ctx));
        assert!(matches!(state, DiffState::NotInRepository));
    });
}

#[test]
fn diff_mode_defaults_to_head() {
    warpui::App::test((), |mut app| async move {
        let handle = app.add_model(DiffStateModel::new_for_test);
        let mode = handle.read(&app, |model, ctx| model.diff_mode(ctx));
        assert!(matches!(mode, DiffMode::Head));
    });
}

#[test]
fn has_head_false_for_test_model() {
    warpui::App::test((), |mut app| async move {
        let handle = app.add_model(DiffStateModel::new_for_test);
        let has_head = handle.read(&app, |model, ctx| model.has_head(ctx));
        assert!(!has_head);
    });
}

#[test]
fn branch_info_none_for_test_model() {
    warpui::App::test((), |mut app| async move {
        let handle = app.add_model(DiffStateModel::new_for_test);
        handle.read(&app, |model, ctx| {
            assert_eq!(model.get_main_branch_name(ctx), None);
            assert_eq!(model.get_current_branch_name(ctx), None);
            assert!(!model.is_on_main_branch(ctx));
            assert!(model.unpushed_commits(ctx).is_empty());
            assert_eq!(model.upstream_ref(ctx), None);
            assert!(!model.upstream_differs_from_main(ctx));
            assert!(!model.is_git_operation_blocked(ctx));
        });
    });
}

#[test]
fn uncommitted_stats_none_for_test_model() {
    warpui::App::test((), |mut app| async move {
        let handle = app.add_model(DiffStateModel::new_for_test);
        let stats = handle.read(&app, |model, ctx| model.get_uncommitted_stats(ctx));
        assert!(stats.is_none());
    });
}

fn revision(rev: &str, comparison: RevisionComparison) -> DiffMode {
    DiffMode::Revision {
        rev: rev.to_string(),
        comparison,
    }
}

#[test]
fn from_branch_distinguishes_the_main_branch() {
    assert_eq!(
        DiffMode::from_branch("main", Some("main")),
        DiffMode::MainBranch
    );
    assert_eq!(
        DiffMode::from_branch("feature", Some("main")),
        DiffMode::OtherBranch("feature".to_string())
    );
    assert_eq!(
        DiffMode::from_branch("main", None),
        DiffMode::OtherBranch("main".to_string())
    );
}

#[test]
fn only_revision_comparisons_are_historical() {
    assert!(!DiffMode::Head.is_historical());
    assert!(!DiffMode::MainBranch.is_historical());
    assert!(!DiffMode::OtherBranch("feature".to_string()).is_historical());
    assert!(revision("v1.0", RevisionComparison::MergeBase).is_historical());
    assert!(revision("HEAD~3", RevisionComparison::Direct).is_historical());
}

#[test]
fn base_label_names_the_base_and_how_it_is_compared() {
    assert_eq!(
        DiffMode::Head.base_label(Some("main")),
        "Uncommitted changes"
    );
    assert_eq!(
        DiffMode::MainBranch.base_label(Some("trunk")),
        "trunk \u{b7} common ancestor"
    );
    assert_eq!(
        DiffMode::MainBranch.base_label(None),
        "main \u{b7} common ancestor"
    );
    assert_eq!(
        DiffMode::OtherBranch("feature".to_string()).base_label(Some("main")),
        "feature \u{b7} common ancestor"
    );
    assert_eq!(
        revision("origin/main", RevisionComparison::MergeBase).base_label(None),
        "origin/main \u{b7} common ancestor"
    );
    assert_eq!(
        revision("HEAD~3", RevisionComparison::Direct).base_label(None),
        "HEAD~3 \u{b7} direct"
    );
    assert_eq!(
        revision(
            "0123456789abcdef0123456789abcdef01234567",
            RevisionComparison::Direct
        )
        .base_label(None),
        "0123456 \u{b7} direct"
    );
}

#[test]
fn display_revision_abbreviates_only_full_commit_ids() {
    assert_eq!(
        display_revision("0123456789ABCDEF0123456789abcdef01234567"),
        "0123456"
    );
    assert_eq!(display_revision("0123456789abcdef"), "0123456789abcdef");
    assert_eq!(display_revision("feature/branch"), "feature/branch");
    // Forty characters that are not all hexadecimal are a name, not a commit id.
    assert_eq!(
        display_revision("release-candidate-for-the-2026-10-12-cut"),
        "release-candidate-for-the-2026-10-12-cut"
    );
}

#[test]
fn diff_modes_compare_and_hash_by_revision_and_comparison() {
    use std::collections::HashSet;

    let modes: HashSet<DiffMode> = [
        DiffMode::Head,
        DiffMode::MainBranch,
        DiffMode::OtherBranch("main".to_string()),
        revision("main", RevisionComparison::MergeBase),
        revision("main", RevisionComparison::Direct),
        revision("main", RevisionComparison::Direct),
    ]
    .into_iter()
    .collect();
    assert_eq!(modes.len(), 5);
}

#[test]
fn serialized_diff_modes_leave_out_branch_and_revision_names() {
    let serialize = |mode: &DiffMode| serde_json::to_value(mode).expect("mode serializes");

    assert_eq!(serialize(&DiffMode::Head), serde_json::json!("Head"));
    assert_eq!(
        serialize(&DiffMode::MainBranch),
        serde_json::json!("MainBranch")
    );
    assert_eq!(
        serialize(&revision("secret-branch", RevisionComparison::Direct)),
        serde_json::json!({ "Revision": { "comparison": "Direct" } })
    );
    assert_eq!(
        serialize(&revision("secret-branch", RevisionComparison::MergeBase)),
        serde_json::json!({ "Revision": { "comparison": "MergeBase" } })
    );
    let other_branch = serialize(&DiffMode::OtherBranch("secret-branch".to_string()));
    assert!(!other_branch.to_string().contains("secret-branch"));
}
