use super::*;

fn branch(name: &str, is_main: bool) -> BranchEntry {
    BranchEntry {
        name: name.to_string(),
        is_main,
    }
}

fn revision_entry(kind: RevisionKind, rev: &str, subject: &str) -> RevisionEntry {
    RevisionEntry {
        kind,
        rev: rev.to_string(),
        short_sha: "abc1234".to_string(),
        subject: subject.to_string(),
        author: match kind {
            RevisionKind::Commit => "Ada".to_string(),
            RevisionKind::RemoteBranch | RevisionKind::Tag => String::new(),
        },
        relative_date: "3 days ago".to_string(),
    }
}

const COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn revisions() -> RevisionCandidates {
    RevisionCandidates {
        remote_branches: vec![revision_entry(
            RevisionKind::RemoteBranch,
            "origin/main",
            "Tip of main",
        )],
        tags: vec![revision_entry(RevisionKind::Tag, "v1.0", "Release")],
        commits: vec![
            revision_entry(RevisionKind::Commit, COMMIT_SHA, "Fix the parser"),
            revision_entry(
                RevisionKind::Commit,
                "89abcdef0123456789abcdef0123456789abcdef",
                "",
            ),
        ],
    }
}

fn targets(
    current_mode: &DiffMode,
    current_branch: Option<&str>,
    branches: &[BranchEntry],
    revisions: Option<&RevisionCandidates>,
) -> Vec<DiffTarget> {
    build_diff_targets(DiffTargetSources {
        current_mode,
        current_branch,
        branches,
        revisions,
    })
}

fn selected_labels(targets: &[DiffTarget]) -> Vec<&str> {
    targets
        .iter()
        .filter(|target| target.is_selected)
        .map(|target| target.label.as_str())
        .collect()
}

#[test]
fn targets_merge_every_source_in_a_fixed_order() {
    let branches = [
        branch("feature", false),
        branch("main", true),
        branch("current", false),
    ];
    let revisions = revisions();
    let targets = targets(
        &DiffMode::Head,
        Some("current"),
        &branches,
        Some(&revisions),
    );

    let listed: Vec<_> = targets
        .iter()
        .map(|target| (target.section, target.label.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            (DiffTargetSection::WorkingTree, "Uncommitted changes"),
            // The main branch leads, and the checked-out branch is left out.
            (DiffTargetSection::Branches, "main"),
            (DiffTargetSection::Branches, "feature"),
            (DiffTargetSection::RemoteBranches, "origin/main"),
            (DiffTargetSection::Tags, "v1.0"),
            (DiffTargetSection::Commits, "Fix the parser"),
            (DiffTargetSection::Commits, "(no commit message)"),
        ]
    );
    assert_eq!(selected_labels(&targets), ["Uncommitted changes"]);
}

#[test]
fn targets_describe_revisions_by_commit_author_and_age() {
    let revisions = revisions();
    let targets = targets(&DiffMode::Head, None, &[], Some(&revisions));

    let details: Vec<_> = targets
        .iter()
        .map(|target| target.detail.as_deref())
        .collect();
    assert_eq!(
        details,
        [
            None,
            Some("abc1234 \u{b7} 3 days ago"),
            Some("abc1234 \u{b7} 3 days ago"),
            Some("abc1234 \u{b7} Ada \u{b7} 3 days ago"),
            Some("abc1234 \u{b7} Ada \u{b7} 3 days ago"),
        ]
    );
    assert_eq!(
        targets[3].base,
        DiffTargetBase::Revision(COMMIT_SHA.to_string())
    );
}

#[test]
fn targets_without_listed_revisions_are_the_branches_alone() {
    let branches = [branch("main", true), branch("feature", false)];
    let targets = targets(&DiffMode::Head, None, &branches, None);
    assert_eq!(targets.len(), 3);
    assert!(targets.iter().all(|target| {
        target.section == DiffTargetSection::WorkingTree
            || target.section == DiffTargetSection::Branches
    }));
}

#[test]
fn the_selected_target_follows_the_diff_mode() {
    let branches = [branch("main", true), branch("feature", false)];
    let revisions = revisions();
    let selected = |mode: DiffMode| {
        let targets = targets(&mode, None, &branches, Some(&revisions));
        selected_labels(&targets)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };

    assert_eq!(selected(DiffMode::MainBranch), ["main"]);
    assert_eq!(
        selected(DiffMode::OtherBranch("feature".to_string())),
        ["feature"]
    );
    // A branch stays selected when it is compared directly.
    assert_eq!(
        selected(DiffMode::Revision {
            rev: "feature".to_string(),
            comparison: RevisionComparison::Direct,
        }),
        ["feature"]
    );
    assert_eq!(
        selected(DiffMode::Revision {
            rev: "v1.0".to_string(),
            comparison: RevisionComparison::MergeBase,
        }),
        ["v1.0"]
    );
    assert_eq!(
        selected(DiffMode::Revision {
            rev: COMMIT_SHA.to_string(),
            comparison: RevisionComparison::Direct,
        }),
        ["Fix the parser"]
    );
}

#[test]
fn a_selected_base_that_no_source_lists_follows_the_uncommitted_changes() {
    let branches = [branch("main", true)];
    let revisions = revisions();

    let typed = DiffMode::Revision {
        rev: "HEAD~3".to_string(),
        comparison: RevisionComparison::Direct,
    };
    let targets_for_typed = targets(&typed, None, &branches, Some(&revisions));
    assert_eq!(targets_for_typed[1].label, "HEAD~3");
    assert_eq!(targets_for_typed[1].section, DiffTargetSection::WorkingTree);
    assert_eq!(
        targets_for_typed[1].base,
        DiffTargetBase::Revision("HEAD~3".to_string())
    );
    assert_eq!(selected_labels(&targets_for_typed), ["HEAD~3"]);

    // A commit that is not among the recent ones is shown by its abbreviated id.
    let old_commit = DiffMode::Revision {
        rev: "fedcba9876543210fedcba9876543210fedcba98".to_string(),
        comparison: RevisionComparison::Direct,
    };
    let targets_for_commit = targets(&old_commit, None, &branches, Some(&revisions));
    assert_eq!(selected_labels(&targets_for_commit), ["fedcba9"]);

    let unlisted_branch = DiffMode::OtherBranch("gone".to_string());
    let targets_for_branch = targets(&unlisted_branch, None, &branches, None);
    assert_eq!(selected_labels(&targets_for_branch), ["gone"]);
    assert_eq!(
        targets_for_branch[1].base,
        DiffTargetBase::Branch("gone".to_string())
    );
}

#[test]
fn a_base_turns_into_the_mode_of_the_chosen_comparison() {
    let merge_base = RevisionComparison::MergeBase;
    let direct = RevisionComparison::Direct;
    let revision = |rev: &str, comparison| DiffMode::Revision {
        rev: rev.to_string(),
        comparison,
    };

    for comparison in [merge_base, direct] {
        assert_eq!(
            DiffTargetBase::UncommittedChanges.mode(comparison),
            DiffMode::Head
        );
    }

    let main = DiffTargetBase::MainBranch("main".to_string());
    assert_eq!(main.mode(merge_base), DiffMode::MainBranch);
    assert_eq!(main.mode(direct), revision("main", direct));

    let feature = DiffTargetBase::Branch("feature".to_string());
    assert_eq!(
        feature.mode(merge_base),
        DiffMode::OtherBranch("feature".to_string())
    );
    assert_eq!(feature.mode(direct), revision("feature", direct));

    let tag = DiffTargetBase::Revision("v1.0".to_string());
    assert_eq!(tag.mode(merge_base), revision("v1.0", merge_base));
    assert_eq!(tag.mode(direct), revision("v1.0", direct));
}

#[test]
fn a_mode_round_trips_through_the_base_that_produced_it() {
    let bases = [
        DiffTargetBase::UncommittedChanges,
        DiffTargetBase::MainBranch("main".to_string()),
        DiffTargetBase::Branch("feature".to_string()),
        DiffTargetBase::Revision("origin/main".to_string()),
    ];
    for base in &bases {
        for comparison in [RevisionComparison::MergeBase, RevisionComparison::Direct] {
            let mode = base.mode(comparison);
            assert!(base.is_base_of(&mode), "{base:?} {comparison:?}");
            let others = bases.iter().filter(|other| *other != base);
            for other in others {
                assert!(!other.is_base_of(&mode), "{other:?} matched {mode:?}");
            }
        }
    }
}
