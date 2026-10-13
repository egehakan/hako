use std::fs;

use warp_util::large_file::{AUTO_OPEN_MAX_LINES, PEEK_MAX_LINE_CHARS, PEEK_MAX_LINES};

use super::*;

#[test]
fn counts_are_grouped_by_thousands() {
    assert_eq!(format_count(0), "0");
    assert_eq!(format_count(999), "999");
    assert_eq!(format_count(1_000), "1,000");
    assert_eq!(format_count(630_788), "630,788");
    assert_eq!(format_count(12_345_678), "12,345,678");
}

#[test]
fn a_single_line_is_not_called_lines() {
    assert_eq!(lines_text(1), "1 line");
    assert_eq!(lines_text(0), "0 lines");
    assert_eq!(lines_text(630_788), "630,788 lines");
}

#[test]
fn a_file_with_too_many_lines_is_read_through_and_can_still_be_opened() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("lines.log");
    let lines = AUTO_OPEN_MAX_LINES + 10;
    fs::write(&path, "entry\n".repeat(lines)).unwrap();

    let text = LargeText::read(&path).unwrap();
    assert_eq!(text.bytes, 6 * lines as u64);
    assert_eq!(text.shape.map(|shape| shape.lines), Some(lines));
    assert_eq!(text.peek.lines.len(), PEEK_MAX_LINES);
    assert!(text.peek.has_more);
    assert!(text.can_open_in_editor());

    let notice = text.notice();
    assert!(
        notice.starts_with("This file has 150,010 lines (900 KB)."),
        "{notice}"
    );
    assert!(notice.contains("takes about"), "{notice}");
    assert!(
        notice.contains("Its first 300 lines are shown here."),
        "{notice}"
    );
}

#[test]
fn a_file_over_the_ceiling_is_not_read_through_or_offered_to_the_editor() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dump.sql");
    let file = fs::File::create(&path).unwrap();
    fs::write(
        &path,
        "CREATE TABLE t (id INT);\nINSERT INTO t VALUES (1);\n",
    )
    .unwrap();
    file.set_len(OPEN_ANYWAY_MAX_BYTES + 1).unwrap();

    let text = LargeText::read(&path).unwrap();
    assert_eq!(text.shape, None);
    assert!(!text.can_open_in_editor());
    assert_eq!(text.peek.lines[0].text, "CREATE TABLE t (id INT);");
    assert!(text.peek.has_more);

    let notice = text.notice();
    assert!(
        notice.starts_with("This file is 67.1 MB, too large to open in the editor."),
        "{notice}"
    );
    assert!(!notice.contains("takes about"), "{notice}");
}

#[test]
fn a_long_line_is_named_as_the_reason_and_cut_in_the_peek() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bundle.min.js");
    fs::write(&path, "a".repeat(3 * 1024 * 1024)).unwrap();

    let text = LargeText::read(&path).unwrap();
    assert!(text.can_open_in_editor());
    assert_eq!(text.peek.lines.len(), 1);
    assert_eq!(text.peek.lines[0].text.len(), PEEK_MAX_LINE_CHARS);
    assert!(text.peek.lines[0].truncated);

    let notice = text.notice();
    assert!(
        notice.starts_with("This file (3.1 MB) has a line of 3.1 MB."),
        "{notice}"
    );
}

#[test]
fn a_missing_file_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    assert!(LargeText::read(&directory.path().join("gone.txt")).is_err());
}
