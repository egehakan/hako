use super::*;

/// A repository with `files` changed files whose graph is closed.
fn repo(files: usize, is_expanded: bool) -> RepoRows {
    RepoRows {
        files,
        is_expanded,
        graph: None,
    }
}

fn file(repo_index: usize, file_index: usize) -> OverviewRow {
    OverviewRow::File {
        repo_index,
        file_index,
    }
}

fn commit(repo_index: usize, commit_index: usize) -> OverviewRow {
    OverviewRow::Commit {
        repo_index,
        commit_index,
    }
}

#[test]
fn flattens_expanded_repositories_into_header_file_and_graph_heading_rows() {
    let rows = flatten_rows([repo(2, true), repo(0, true), repo(1, true)]);

    assert_eq!(
        rows,
        [
            OverviewRow::Repo { repo_index: 0 },
            file(0, 0),
            file(0, 1),
            OverviewRow::GraphHeader { repo_index: 0 },
            OverviewRow::Repo { repo_index: 1 },
            OverviewRow::GraphHeader { repo_index: 1 },
            OverviewRow::Repo { repo_index: 2 },
            file(2, 0),
            OverviewRow::GraphHeader { repo_index: 2 },
        ]
    );
}

#[test]
fn collapsed_repositories_contribute_only_their_header() {
    let open_graph = RepoRows {
        graph: Some(GraphRows {
            commits: 50,
            open: None,
            has_note: true,
        }),
        ..repo(280, false)
    };
    let rows = flatten_rows([open_graph, repo(3, true)]);

    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0], OverviewRow::Repo { repo_index: 0 });
    assert_eq!(rows[1], OverviewRow::Repo { repo_index: 1 });
    assert_eq!(rows[4], file(1, 2));
    assert_eq!(rows[5], OverviewRow::GraphHeader { repo_index: 1 });
}

#[test]
fn no_repositories_produce_no_rows() {
    assert!(flatten_rows([]).is_empty());
}

#[test]
fn an_open_graph_lists_its_commits_below_the_changed_files() {
    let rows = flatten_rows([RepoRows {
        graph: Some(GraphRows {
            commits: 2,
            open: None,
            has_note: false,
        }),
        ..repo(1, true)
    }]);

    assert_eq!(
        rows,
        [
            OverviewRow::Repo { repo_index: 0 },
            file(0, 0),
            OverviewRow::GraphHeader { repo_index: 0 },
            commit(0, 0),
            commit(0, 1),
        ]
    );
}

#[test]
fn the_open_commit_has_its_notes_and_files_right_under_it() {
    let rows = flatten_rows([RepoRows {
        graph: Some(GraphRows {
            commits: 3,
            open: Some(OpenCommitRows {
                index: 1,
                notes: 1,
                files: 2,
            }),
            has_note: true,
        }),
        ..repo(0, true)
    }]);

    assert_eq!(
        rows,
        [
            OverviewRow::Repo { repo_index: 0 },
            OverviewRow::GraphHeader { repo_index: 0 },
            commit(0, 0),
            commit(0, 1),
            OverviewRow::CommitNote {
                repo_index: 0,
                commit_index: 1,
                line: 0
            },
            OverviewRow::CommitFile {
                repo_index: 0,
                commit_index: 1,
                file_index: 0
            },
            OverviewRow::CommitFile {
                repo_index: 0,
                commit_index: 1,
                file_index: 1
            },
            commit(0, 2),
            OverviewRow::GraphNote { repo_index: 0 },
        ]
    );
}

#[test]
fn a_graph_with_nothing_read_yet_is_its_heading_and_one_line() {
    let rows = flatten_rows([RepoRows {
        graph: Some(GraphRows {
            commits: 0,
            open: None,
            has_note: true,
        }),
        ..repo(0, true)
    }]);

    assert_eq!(
        rows,
        [
            OverviewRow::Repo { repo_index: 0 },
            OverviewRow::GraphHeader { repo_index: 0 },
            OverviewRow::GraphNote { repo_index: 0 },
        ]
    );
}

#[test]
fn only_repositories_with_many_changes_start_collapsed() {
    assert!(is_expanded_by_default(0));
    assert!(is_expanded_by_default(MAX_FILES_EXPANDED_BY_DEFAULT));
    assert!(!is_expanded_by_default(MAX_FILES_EXPANDED_BY_DEFAULT + 1));
    assert!(!is_expanded_by_default(280));
}

#[test]
fn every_status_has_a_distinct_letter() {
    let statuses = [
        GitFileStatus::New,
        GitFileStatus::Modified,
        GitFileStatus::Deleted,
        GitFileStatus::Renamed {
            old_path: "old".to_owned(),
        },
        GitFileStatus::Copied {
            old_path: "old".to_owned(),
        },
        GitFileStatus::Untracked,
        GitFileStatus::Conflicted,
    ];

    let letters: Vec<&str> = statuses.iter().map(status_letter).collect();

    assert_eq!(letters, ["A", "M", "D", "R", "C", "U", "!"]);
}

#[test]
fn change_counts_are_singular_only_for_one() {
    assert_eq!(change_count_label(0), "0 changes");
    assert_eq!(change_count_label(1), "1 change");
    assert_eq!(change_count_label(300), "300 changes");
}

#[test]
fn main_branch_is_recognised_with_or_without_its_remote_prefix() {
    assert!(is_main_branch("main", "main"));
    assert!(is_main_branch("main", "origin/main"));
    assert!(!is_main_branch("feature", "origin/main"));
    assert!(!is_main_branch("origin/main", "main"));
}
