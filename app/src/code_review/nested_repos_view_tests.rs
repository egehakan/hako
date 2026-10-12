use super::*;

#[test]
fn flattens_expanded_repositories_into_header_and_file_rows() {
    let rows = flatten_rows([(2, true), (0, true), (1, true)]);

    assert_eq!(
        rows,
        [
            OverviewRow::Repo { repo_index: 0 },
            OverviewRow::File {
                repo_index: 0,
                file_index: 0
            },
            OverviewRow::File {
                repo_index: 0,
                file_index: 1
            },
            OverviewRow::Repo { repo_index: 1 },
            OverviewRow::Repo { repo_index: 2 },
            OverviewRow::File {
                repo_index: 2,
                file_index: 0
            },
        ]
    );
}

#[test]
fn collapsed_repositories_contribute_only_their_header() {
    let rows = flatten_rows([(280, false), (3, true)]);

    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0], OverviewRow::Repo { repo_index: 0 });
    assert_eq!(rows[1], OverviewRow::Repo { repo_index: 1 });
    assert_eq!(
        rows[4],
        OverviewRow::File {
            repo_index: 1,
            file_index: 2
        }
    );
}

#[test]
fn no_repositories_produce_no_rows() {
    assert!(flatten_rows([]).is_empty());
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
