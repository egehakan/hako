use super::*;

/// One record as git prints it: every field ended by a NUL byte, and a line break after it.
fn record(fields: &[&str]) -> Vec<u8> {
    let mut record = Vec::new();
    for field in fields {
        record.extend_from_slice(field.as_bytes());
        record.push(0);
    }
    record.push(b'\n');
    record
}

fn branch_record(reference: &str, time: &str, upstream: &str, track: &str) -> Vec<u8> {
    record(&[reference, "0a1b", time, "Subject", upstream, track, "", ""])
}

fn names(branches: &[GitBranch]) -> Vec<&str> {
    branches.iter().map(|branch| branch.name.as_str()).collect()
}

#[test]
fn orders_branches_and_leaves_out_what_a_local_branch_tracks() {
    let output = [
        branch_record("refs/heads/old", "100", "", ""),
        branch_record(
            "refs/heads/main",
            "300",
            "refs/remotes/origin/main",
            "ahead 2, behind 1",
        ),
        branch_record("refs/heads/b", "200", "refs/remotes/origin/b", "behind 4"),
        branch_record("refs/heads/a", "200", "refs/remotes/origin/a", "gone"),
        record(&[
            "refs/remotes/origin/HEAD",
            "0a1b",
            "300",
            "Subject",
            "",
            "",
            "refs/remotes/origin/main",
            "",
        ]),
        branch_record("refs/remotes/origin/main", "300", "", ""),
        branch_record("refs/remotes/origin/b", "200", "", ""),
        branch_record("refs/remotes/origin/zeta", "50", "", ""),
        branch_record("refs/remotes/upstream/topic", "400", "", ""),
    ]
    .concat();

    let (branches, truncated) = parse_branches(&output, 500);

    assert_eq!(
        names(&branches),
        ["main", "a", "b", "old", "upstream/topic", "origin/zeta"]
    );
    assert!(!truncated);
    let main = &branches[0];
    assert_eq!(main.kind, GitBranchKind::Local);
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!((main.ahead, main.behind), (Some(2), Some(1)));
    assert_eq!(main.time, 300_000);
    let gone = &branches[1];
    assert_eq!(
        (gone.upstream.as_deref(), gone.ahead, gone.behind),
        (None, None, None)
    );
    assert_eq!((branches[2].ahead, branches[2].behind), (Some(0), Some(4)));
    assert_eq!(branches[4].kind, GitBranchKind::Remote);
    assert_eq!(branches[4].ahead, None);

    let (first, truncated) = parse_branches(&output, 5);
    assert_eq!(names(&first), ["main", "a", "b", "old", "upstream/topic"]);
    assert!(truncated);
}

#[test]
fn keeps_a_branch_whole_whatever_its_subject_and_its_working_tree_hold() {
    let long = "é".repeat(SUBJECT_MAX_CHARS + 50);
    let output = record(&[
        "refs/heads/main",
        "0a1b",
        "1",
        &long,
        "",
        "",
        "",
        "/code/with\nline break",
    ]);

    let (branches, _) = parse_branches(&output, 500);

    assert_eq!(branches[0].subject.chars().count(), SUBJECT_MAX_CHARS);
    assert_eq!(
        branches[0].worktree.as_deref(),
        Some("/code/with\nline break")
    );
}

#[test]
fn reads_refs_by_the_commit_they_point_at() {
    let output = [
        record(&["refs/heads/main", "aaaa", "", ""]),
        record(&["refs/heads/topic", "aaaa", "", ""]),
        record(&[
            "refs/remotes/origin/HEAD",
            "aaaa",
            "",
            "refs/remotes/origin/main",
        ]),
        record(&["refs/remotes/origin/main", "aaaa", "", ""]),
        record(&["refs/tags/v1", "cccc", "aaaa", ""]),
        record(&["refs/tags/light", "bbbb", "", ""]),
    ]
    .concat();

    let mut refs = parse_refs(&output, Some("refs/heads/topic"));
    let mut at_a = refs.remove("aaaa").unwrap();
    sort_refs(&mut at_a);

    let described: Vec<(&str, GitRefKind)> = at_a
        .iter()
        .map(|reference| (reference.name.as_str(), reference.kind))
        .collect();
    assert_eq!(
        described,
        [
            ("topic", GitRefKind::Head),
            ("main", GitRefKind::Local),
            ("origin/main", GitRefKind::Remote),
            ("v1", GitRefKind::Tag),
        ]
    );
    assert_eq!(refs["bbbb"][0].name, "light");
    assert!(!refs.contains_key("cccc"));
}

#[test]
fn reads_commits_past_the_line_rev_list_puts_before_each() {
    let output = [
        b"commit aaaa\n".to_vec(),
        record(&[
            "aaaa",
            "bbbb cccc",
            "Ada",
            "1700000000",
            "Merge",
            "\nWhy.\n\nMore.\n",
        ]),
        b"commit bbbb\n".to_vec(),
        record(&["bbbb", "", "", "5", "", ""]),
    ]
    .concat();

    let commits = parse_commits(&output, true);

    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].id, "aaaa");
    assert_eq!(commits[0].parents, ["bbbb", "cccc"]);
    assert_eq!(commits[0].author, "Ada");
    assert_eq!(commits[0].time, 1_700_000_000_000);
    assert_eq!(commits[0].subject, "Merge");
    assert_eq!(commits[0].body, "Why.\n\nMore.");
    assert_eq!(commits[1].id, "bbbb");
    assert!(commits[1].parents.is_empty());
    assert_eq!(commits[1].body, "");
}

#[test]
fn cuts_a_long_body_between_characters() {
    let long = format!("a{}", "é".repeat(BODY_LIMIT));
    let output = record(&["aaaa", "", "Ada", "1", "Subject", &long]);

    let commits = parse_commits(&output, true);

    assert_eq!(commits[0].body.len(), BODY_LIMIT - 1);
    assert!(commits[0].body.ends_with('é'));
}

#[test]
fn reads_the_files_a_commit_changed() {
    let output = b"M\0b.txt\0R100\0old.txt\0new.txt\0A\0a.txt\0D\0gone.txt\0C75\0a.txt\0copy.txt\0\
        T\0link\0";

    let files = parse_name_status(output);

    let described: Vec<(&str, Option<&str>, GitFileStatus)> = files
        .iter()
        .map(|file| (file.path.as_str(), file.old_path.as_deref(), file.status))
        .collect();
    assert_eq!(
        described,
        [
            ("a.txt", None, GitFileStatus::Added),
            ("b.txt", None, GitFileStatus::Modified),
            ("copy.txt", None, GitFileStatus::Added),
            ("gone.txt", None, GitFileStatus::Deleted),
            ("link", None, GitFileStatus::Modified),
            ("new.txt", Some("old.txt"), GitFileStatus::Renamed),
        ]
    );
}
