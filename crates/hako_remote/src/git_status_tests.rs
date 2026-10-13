use super::*;

const OID: &str = "1111111111111111111111111111111111111111";

fn ordinary(xy: &str, path: &str) -> String {
    format!("1 {xy} N... 100644 100644 100644 {OID} {OID} {path}\0")
}

fn renamed(xy: &str, score: &str, path: &str, original: &str) -> String {
    format!("2 {xy} N... 100644 100644 100644 {OID} {OID} {score} {path}\0{original}\0")
}

fn files(output: &str) -> Vec<(String, Option<String>, GitFileStatus)> {
    parse_status(output.as_bytes())
        .files
        .into_iter()
        .map(|entry| (entry.path, entry.old_path, entry.status))
        .collect()
}

fn file(path: &str, status: GitFileStatus) -> (String, Option<String>, GitFileStatus) {
    (path.to_owned(), None, status)
}

#[test]
fn reads_the_branch_and_what_it_tracks() {
    let output = format!(
        "# branch.oid {OID}\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -5\0"
    );

    let status = parse_status(output.as_bytes());

    assert_eq!(status.head.as_deref(), Some(OID));
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.upstream.as_deref(), Some("origin/main"));
    assert_eq!((status.ahead, status.behind), (2, 5));
}

#[test]
fn has_no_branch_while_detached_and_no_head_before_the_first_commit() {
    let detached =
        parse_status(format!("# branch.oid {OID}\0# branch.head (detached)\0").as_bytes());
    let unborn = parse_status(b"# branch.oid (initial)\0# branch.head main\0");

    assert_eq!(detached.branch, None);
    assert_eq!(detached.head.as_deref(), Some(OID));
    assert_eq!(unborn.head, None);
    assert_eq!(unborn.branch.as_deref(), Some("main"));
}

#[test]
fn has_no_upstream_when_the_one_it_tracks_is_gone() {
    let output =
        format!("# branch.oid {OID}\0# branch.head topic\0# branch.upstream origin/topic\0");

    let status = parse_status(output.as_bytes());

    assert_eq!(status.upstream, None);
    assert_eq!((status.ahead, status.behind), (0, 0));
}

#[test]
fn maps_each_record_to_the_status_of_its_file() {
    let output = [
        ordinary(".M", "modified.txt"),
        ordinary("T.", "type changed.txt"),
        ordinary("A.", "added.txt"),
        ordinary("AM", "added then edited.txt"),
        ordinary("D.", "deleted in index.txt"),
        ordinary(".D", "deleted on disk.txt"),
        ordinary("MD", "edited then deleted.txt"),
        ordinary("AD", "added then deleted.txt"),
        format!("u UU N... 100644 100644 100644 100644 {OID} {OID} {OID} in conflict.txt\0"),
        "? new file.txt\0".to_owned(),
        "? nested/\0".to_owned(),
    ]
    .concat();

    assert_eq!(
        files(&output),
        [
            file("added then edited.txt", GitFileStatus::Added),
            file("added.txt", GitFileStatus::Added),
            file("deleted in index.txt", GitFileStatus::Deleted),
            file("deleted on disk.txt", GitFileStatus::Deleted),
            file("edited then deleted.txt", GitFileStatus::Deleted),
            file("in conflict.txt", GitFileStatus::Conflicted),
            file("modified.txt", GitFileStatus::Modified),
            file("nested", GitFileStatus::Untracked),
            file("new file.txt", GitFileStatus::Untracked),
            file("type changed.txt", GitFileStatus::Modified),
        ]
    );
}

#[test]
fn reads_renames_and_copies_with_the_original_path_that_follows() {
    let output = [
        renamed("R.", "R100", "new name.txt", "old name.txt"),
        renamed("RD", "R100", "gone.txt", "was here.txt"),
        renamed("C.", "C90", "copy.txt", "source.txt"),
        renamed("CD", "C90", "copy gone.txt", "source.txt"),
        ordinary(".M", "after.txt"),
    ]
    .concat();

    assert_eq!(
        files(&output),
        [
            file("after.txt", GitFileStatus::Modified),
            file("copy.txt", GitFileStatus::Added),
            (
                "new name.txt".to_owned(),
                Some("old name.txt".to_owned()),
                GitFileStatus::Renamed
            ),
            file("was here.txt", GitFileStatus::Deleted),
        ]
    );
}

#[test]
fn counts_a_file_deleted_in_the_index_and_back_as_untracked_once_as_modified() {
    let output = [ordinary("D.", "back.txt"), "? back.txt\0".to_owned()].concat();

    assert_eq!(files(&output), [file("back.txt", GitFileStatus::Modified)]);
}

#[test]
fn leaves_out_a_file_whose_name_is_not_utf8() {
    let mut output = ordinary(".M", "fine.txt").into_bytes();
    output.extend_from_slice(b"? caf\xe9.txt\0");

    assert_eq!(parse_status(&output).files.len(), 1);
}

#[test]
fn orders_files_by_code_point() {
    let output = "? b\0? a/z\0? B\0? \u{e9}\0? a b\0";

    let paths: Vec<_> = files(output).into_iter().map(|file| file.0).collect();

    assert_eq!(paths, ["B", "a b", "a/z", "b", "\u{e9}"]);
}

#[test]
fn reads_line_counts_by_path() {
    let output = b"3\t1\tplain.txt\x000\t0\t\0old name.txt\0new name.txt\0-\t-\timage.png\x0012\t0\twith\ttab.txt\0";

    let counts = parse_numstat(output);

    let lines = |additions, deletions| LineCounts {
        additions: Some(additions),
        deletions: Some(deletions),
        binary: false,
    };
    assert_eq!(counts.len(), 4);
    assert_eq!(counts["plain.txt"], lines(3, 1));
    assert_eq!(counts["new name.txt"], lines(0, 0));
    assert_eq!(counts["image.png"], LineCounts::BINARY);
    assert_eq!(counts["with\ttab.txt"], lines(12, 0));
}
