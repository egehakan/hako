use ::hako_remote::git_graph::LaneKind;
use ::hako_remote::protocol::{GitFileStatus, GitSide};

use super::*;

fn commit(id: &str, parents: &[&str]) -> GitLogCommit {
    GitLogCommit {
        id: id.to_owned(),
        parents: parents.iter().map(|parent| (*parent).to_owned()).collect(),
        subject: format!("Commit {id}"),
        author: "Ada".to_owned(),
        time: 0,
        refs: Vec::new(),
        side: GitSide::Both,
    }
}

fn log(commits: Vec<GitLogCommit>, has_more: bool, head: &str) -> GitLog {
    GitLog {
        commits,
        has_more,
        head: Some(head.to_owned()),
        upstream: None,
    }
}

fn shown(id: &str, body: &str, files: &[&str]) -> GitShow {
    GitShow {
        commit: commit(id, &["p"]),
        body: body.to_owned(),
        files: files
            .iter()
            .map(|path| GitFile {
                path: (*path).to_owned(),
                old_path: None,
                status: GitFileStatus::Modified,
                additions: Some(1),
                deletions: Some(0),
                binary: false,
            })
            .collect(),
        truncated: false,
    }
}

/// A history whose first page has been read: a merge of `f` into the line `m`, `b`, `a`.
fn first_page() -> RepoHistory {
    let mut history = RepoHistory::default();
    let request = history.begin_reload().unwrap();
    let page = vec![
        commit("m", &["b", "f"]),
        commit("f", &["e"]),
        commit("b", &["a"]),
    ];
    history.apply(request, Ok(log(page, true, "m")));
    history
}

#[test]
fn a_history_is_first_read_one_page_from_its_newest_commit() {
    let mut history = RepoHistory::default();

    assert_eq!(history.note().as_deref(), Some("Loading history\u{2026}"));
    assert_eq!(
        history.begin_reload(),
        Some(PageRequest {
            limit: HISTORY_PAGE,
            skip: 0
        })
    );
    assert!(history.is_loading());
}

#[test]
fn a_page_that_was_read_becomes_the_rows_of_the_graph() {
    let history = first_page();

    assert!(!history.is_loading());
    assert_eq!(history.commits().len(), 3);
    assert_eq!(history.rows().len(), 3);
    assert_eq!(history.lane_count(), 2);
    assert!(history.has_more());
    assert_eq!(history.note().as_deref(), Some("Loading more\u{2026}"));
}

#[test]
fn the_next_page_continues_the_lanes_that_were_open() {
    let mut history = first_page();
    let request = history.begin_next_page().unwrap();
    assert_eq!(
        request,
        PageRequest {
            limit: HISTORY_PAGE,
            skip: 3
        }
    );

    let page = vec![commit("e", &["a"]), commit("a", &[])];
    let reload = history.apply(request, Ok(log(page, false, "m")));

    assert!(!reload);
    assert_eq!(history.commits().len(), 5);
    // The merged line was still open in the second lane when the first page ended.
    assert_eq!(history.rows()[3].column, 1);
    assert_eq!(history.rows()[3].kind, LaneKind::Other);
    assert_eq!(history.rows()[4].column, 0);
    assert!(!history.has_more());
    assert_eq!(history.note(), None);
}

#[test]
fn only_one_page_is_read_at_a_time() {
    let mut history = first_page();

    assert!(history.begin_next_page().is_some());
    assert_eq!(history.begin_next_page(), None);
}

#[test]
fn nothing_more_is_read_once_the_history_has_been_read_to_its_end() {
    let mut history = RepoHistory::default();
    let request = history.begin_reload().unwrap();
    history.apply(request, Ok(log(vec![commit("a", &[])], false, "a")));

    assert_eq!(history.begin_next_page(), None);
}

#[test]
fn a_change_while_a_page_is_read_makes_the_history_be_read_again() {
    let mut history = first_page();
    let request = history.begin_next_page().unwrap();

    assert_eq!(history.begin_reload(), None);
    let page = vec![commit("e", &["a"]), commit("a", &[])];
    let reload = history.apply(request, Ok(log(page, false, "m")));

    assert!(reload);
    assert_eq!(
        history.begin_reload(),
        Some(PageRequest {
            limit: HISTORY_PAGE,
            skip: 0
        })
    );
}

#[test]
fn a_page_of_a_history_that_has_moved_is_left_out_and_the_history_read_again() {
    let mut history = first_page();
    let request = history.begin_next_page().unwrap();

    let page = vec![commit("e", &["a"])];
    let reload = history.apply(request, Ok(log(page, false, "n")));

    assert!(reload);
    assert_eq!(history.commits().len(), 3);
}

#[test]
fn a_history_that_was_scrolled_is_read_again_as_far_as_it_had_been_read() {
    let mut history = RepoHistory::default();
    let request = history.begin_reload().unwrap();
    let ids: Vec<String> = (0..120).map(|index| format!("c{index}")).collect();
    let page = ids
        .iter()
        .enumerate()
        .map(|(index, id)| match ids.get(index + 1) {
            Some(parent) => commit(id, &[parent]),
            None => commit(id, &["older"]),
        })
        .collect();
    history.apply(request, Ok(log(page, true, "c0")));

    assert_eq!(
        history.begin_reload(),
        Some(PageRequest {
            limit: 120,
            skip: 0
        })
    );
}

#[test]
fn a_history_that_could_not_be_read_keeps_what_it_had_and_says_why() {
    let mut history = first_page();
    let request = history.begin_reload().unwrap();

    history.apply(request, Err("fatal: bad\nobject".to_owned()));

    assert_eq!(history.commits().len(), 3);
    assert_eq!(history.note().as_deref(), Some("fatal: bad object"));
    assert_eq!(history.begin_next_page(), None);
}

#[test]
fn a_repository_without_commits_says_so() {
    let mut history = RepoHistory::default();
    let request = history.begin_reload().unwrap();
    let empty = GitLog {
        commits: Vec::new(),
        has_more: false,
        head: None,
        upstream: None,
    };

    history.apply(request, Ok(empty));

    assert_eq!(history.note().as_deref(), Some("No commits yet"));
    assert_eq!(history.lane_count(), 0);
}

#[test]
fn opening_a_commit_shows_who_wrote_it_its_message_and_its_files() {
    let mut history = first_page();

    assert!(history.toggle_commit("f"));
    assert_eq!(history.open_index(), Some(1));
    let open = history.open().unwrap();
    assert_eq!(open.notes(), ["Loading the commit\u{2026}"]);
    assert!(open.files().is_empty());

    history.set_commit_files(
        "f",
        Ok(shown("f", "\nWhy it was done.\n\nMore.\n", &["a.rs"])),
    );
    let open = history.open().unwrap();
    let notes = open.notes();
    assert!(notes[0].starts_with("f \u{b7} Ada \u{b7} "), "{notes:?}");
    assert_eq!(notes[1..], ["Why it was done.", "More."]);
    assert_eq!(open.files().len(), 1);
    assert_eq!(open.parent(), Some("p"));
}

#[test]
fn a_long_message_is_cut_and_a_commit_without_files_says_so() {
    let mut history = first_page();
    history.toggle_commit("f");
    let body: String = (0..20).map(|line| format!("line {line}\n")).collect();

    history.set_commit_files("f", Ok(shown("f", &body, &[])));

    let notes = history.open().unwrap().notes();
    assert_eq!(notes.len(), 1 + MAX_BODY_LINES + 2);
    assert_eq!(notes[MAX_BODY_LINES + 1], "\u{2026}");
    assert_eq!(notes[MAX_BODY_LINES + 2], "No files changed");
}

#[test]
fn opening_the_open_commit_again_closes_it() {
    let mut history = first_page();
    history.toggle_commit("f");

    assert!(!history.toggle_commit("f"));
    assert_eq!(history.open(), None);
}

#[test]
fn files_that_arrive_for_a_commit_that_is_no_longer_open_are_dropped() {
    let mut history = first_page();
    history.toggle_commit("f");
    history.toggle_commit("b");

    history.set_commit_files("f", Ok(shown("f", "", &["a.rs"])));

    assert_eq!(history.open().unwrap().files, CommitFiles::Loading);
}

#[test]
fn a_commit_that_could_not_be_read_says_why() {
    let mut history = first_page();
    history.toggle_commit("f");

    history.set_commit_files("f", Err("No commit f\nin this repository".to_owned()));

    assert_eq!(
        history.open().unwrap().notes(),
        ["No commit f in this repository"]
    );
}

#[test]
fn the_open_commit_closes_when_the_history_no_longer_has_it() {
    let mut history = first_page();
    history.toggle_commit("f");
    let request = history.begin_reload().unwrap();

    history.apply(request, Ok(log(vec![commit("x", &[])], false, "x")));

    assert_eq!(history.open(), None);
}

#[test]
fn the_open_commit_stays_open_when_the_history_still_has_it() {
    let mut history = first_page();
    history.toggle_commit("b");
    let request = history.begin_reload().unwrap();

    let page = vec![
        commit("n", &["m"]),
        commit("m", &["b", "f"]),
        commit("b", &["a"]),
    ];
    history.apply(request, Ok(log(page, true, "n")));

    assert_eq!(history.open_index(), Some(2));
}

#[test]
fn what_git_printed_is_put_on_one_line() {
    assert_eq!(
        one_line("error: Your local changes\n\tfile.txt\nAborting\n"),
        "error: Your local changes file.txt Aborting"
    );
}
