use super::*;

fn commit(repo_index: usize, commit_index: usize) -> OverviewRow {
    OverviewRow::Commit {
        repo_index,
        commit_index,
    }
}

#[test]
fn a_graph_is_open_from_the_start_only_where_one_repository_is_listed() {
    assert!(is_graph_expanded_by_default(1));
    assert!(!is_graph_expanded_by_default(0));
    assert!(!is_graph_expanded_by_default(2));
    assert!(!is_graph_expanded_by_default(12));
}

#[test]
fn more_history_is_wanted_once_the_end_of_what_was_read_is_near() {
    let rows: Vec<OverviewRow> = [OverviewRow::Repo { repo_index: 0 }]
        .into_iter()
        .chain((0..50).map(|commit_index| commit(0, commit_index)))
        .collect();

    assert!(repos_wanting_more(&rows, 0..20, |_| 50).is_empty());
    assert_eq!(
        repos_wanting_more(&rows, 30..50, |_| 50),
        [0],
        "the forty-first commit is on screen"
    );
}

#[test]
fn the_line_after_the_last_commit_asks_for_more_history_too() {
    let rows = [
        OverviewRow::Repo { repo_index: 0 },
        OverviewRow::Repo { repo_index: 1 },
        OverviewRow::GraphHeader { repo_index: 1 },
        OverviewRow::GraphNote { repo_index: 1 },
    ];

    assert_eq!(repos_wanting_more(&rows, 0..4, |_| 0), [1]);
    assert!(repos_wanting_more(&rows, 0..3, |_| 0).is_empty());
}

#[test]
fn each_repository_is_asked_once_however_many_of_its_rows_are_on_screen() {
    let rows = [
        commit(0, 0),
        commit(0, 1),
        OverviewRow::CommitNote {
            repo_index: 0,
            commit_index: 1,
            line: 0,
        },
        OverviewRow::CommitFile {
            repo_index: 0,
            commit_index: 1,
            file_index: 0,
        },
        commit(2, 0),
    ];

    assert_eq!(repos_wanting_more(&rows, 0..5, |_| 2), [0, 2]);
}

#[test]
fn rows_past_the_end_of_the_list_are_not_looked_at() {
    let rows = [commit(0, 0)];

    assert_eq!(repos_wanting_more(&rows, 0..40, |_| 1), [0]);
    assert!(repos_wanting_more(&rows, 5..40, |_| 1).is_empty());
}

#[test]
fn every_status_of_a_commit_s_file_has_a_letter() {
    let letters: Vec<&str> = [
        CommitFileStatus::Modified,
        CommitFileStatus::Added,
        CommitFileStatus::Deleted,
        CommitFileStatus::Renamed,
    ]
    .into_iter()
    .map(commit_status_letter)
    .collect();

    assert_eq!(letters, ["M", "A", "D", "R"]);
}

#[test]
fn only_what_is_still_to_be_pulled_is_drawn_as_a_ring() {
    assert_eq!(dot_style(GitSide::Both), DotStyle::Filled);
    assert_eq!(dot_style(GitSide::Local), DotStyle::Filled);
    assert_eq!(dot_style(GitSide::Remote), DotStyle::Hollow);
}

#[test]
fn a_commit_is_signed_with_its_author_and_its_age() {
    let commit = GitLogCommit {
        id: "a".repeat(40),
        parents: Vec::new(),
        subject: "Subject".to_owned(),
        author: "Ada".to_owned(),
        time: 1_000_000,
        refs: Vec::new(),
        side: GitSide::Both,
    };

    assert_eq!(
        commit_byline(&commit, 1_000 + 3 * 3600),
        "Ada \u{b7} 3 hours ago"
    );
    let anonymous = GitLogCommit {
        author: String::new(),
        ..commit
    };
    assert_eq!(commit_byline(&anonymous, 1_030), "just now");
}
