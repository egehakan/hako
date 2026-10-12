use super::*;

const ADA: &str = "6f1d5a36588d0ceafd2b2308a6d24c291477e891";
const GRACE: &str = "2f6510a11a70dd87536a816fb8173acd92ba3199";
const UNCOMMITTED: &str = "0000000000000000000000000000000000000000";

/// `git blame --porcelain` of a four-line file: three lines from a root commit made when the
/// file had another name, and a CRLF-terminated line added by the commit that renamed it.
const PORCELAIN: &[u8] = b"6f1d5a36588d0ceafd2b2308a6d24c291477e891 1 1 3
author Ada Lovelace
author-mail <ada@example.com>
author-time 1704161045
author-tz +0100
committer Ada
committer-mail <ada@example.com>
committer-time 1704161045
committer-tz +0100
summary Add the file
boundary
filename old.txt
\tone
6f1d5a36588d0ceafd2b2308a6d24c291477e891 2 2
\ttwo
6f1d5a36588d0ceafd2b2308a6d24c291477e891 3 3
\tthree
2f6510a11a70dd87536a816fb8173acd92ba3199 4 4 1
author Grace Hopper
author-mail <grace@example.com>
author-time 1749301750
author-tz -0500
committer Grace
committer-mail <grace@example.com>
committer-time 1749301750
committer-tz -0500
summary Rename and add a line
previous 6f1d5a36588d0ceafd2b2308a6d24c291477e891 old.txt
filename new.txt
\tfour\r
";

/// `git blame --incremental --contents -` of the same file with a new first line: groups come
/// in the order git settles them, without the text of the lines.
const INCREMENTAL: &[u8] = b"0000000000000000000000000000000000000000 1 1 1
author External file (--contents)
author-mail <external.file>
author-time 1791831205
author-tz +0300
committer External file (--contents)
committer-mail <external.file>
committer-time 1791831205
committer-tz +0300
summary Version of new.txt from standard input
previous 2f6510a11a70dd87536a816fb8173acd92ba3199 new.txt
filename new.txt
2f6510a11a70dd87536a816fb8173acd92ba3199 4 5 1
author Grace Hopper
author-mail <grace@example.com>
author-time 1749301750
author-tz -0500
committer Grace
committer-mail <grace@example.com>
committer-time 1749301750
committer-tz -0500
summary Rename and add a line
previous 6f1d5a36588d0ceafd2b2308a6d24c291477e891 old.txt
filename new.txt
6f1d5a36588d0ceafd2b2308a6d24c291477e891 1 2 3
author Ada Lovelace
author-mail <ada@example.com>
author-time 1704161045
author-tz +0100
committer Ada
committer-mail <ada@example.com>
committer-time 1704161045
committer-tz +0100
summary Add the file
boundary
filename old.txt
";

fn parse(output: &[u8]) -> FileBlame {
    let mut parser = BlameParser::default();
    parser.feed(output);
    parser.finish()
}

fn shas(blame: &FileBlame, lines: std::ops::Range<usize>) -> Vec<&str> {
    lines
        .map(|line| {
            blame
                .commit_for_line(line)
                .map_or("", |commit| commit.sha.as_str())
        })
        .collect()
}

#[test]
fn porcelain_attributes_every_line_of_a_group() {
    let blame = parse(PORCELAIN);

    assert_eq!(shas(&blame, 0..5), [ADA, ADA, ADA, GRACE, ""]);
    assert_eq!(blame.commits().len(), 2);
    assert_eq!(blame.blamed_line_count(), 4);
    // The group header names the first line only: the others count up from it.
    let original_lines: Vec<_> = (0..4)
        .map(|line| blame.line(line).expect("line is blamed").original_line)
        .collect();
    assert_eq!(original_lines, [1, 2, 3, 4]);
}

#[test]
fn commit_fields_are_read() {
    let blame = parse(PORCELAIN);

    assert_eq!(
        blame.commit_for_line(3),
        Some(&BlameCommit {
            sha: GRACE.to_string(),
            author: "Grace Hopper".to_string(),
            author_mail: "grace@example.com".to_string(),
            author_time: 1749301750,
            author_tz_minutes: -300,
            summary: "Rename and add a line".to_string(),
            path: "new.txt".to_string(),
            is_boundary: false,
        })
    );
}

#[test]
fn boundary_commit_is_marked_and_still_blamed() {
    let blame = parse(PORCELAIN);
    let root = blame.commit_for_line(0).expect("line is blamed");

    assert!(root.is_boundary);
    assert!(!root.is_uncommitted());
    assert_eq!(root.author, "Ada Lovelace");
    assert_eq!(root.author_tz_minutes, 60);
    assert!(!blame.commit_for_line(3).expect("line is blamed").is_boundary);
}

#[test]
fn renamed_file_keeps_the_name_it_had_in_each_commit() {
    let blame = parse(PORCELAIN);

    assert_eq!(blame.commit_for_line(0).map(|c| c.path.as_str()), Some("old.txt"));
    assert_eq!(blame.commit_for_line(3).map(|c| c.path.as_str()), Some("new.txt"));
}

#[test]
fn incremental_output_places_groups_by_their_final_line() {
    let blame = parse(INCREMENTAL);

    assert_eq!(shas(&blame, 0..6), [UNCOMMITTED, ADA, ADA, ADA, GRACE, ""]);
    // The three lines of the group moved down by one, and keep their original numbers.
    let ada_lines: Vec<_> = (1..4)
        .map(|line| blame.line(line).expect("line is blamed").original_line)
        .collect();
    assert_eq!(ada_lines, [1, 2, 3]);
}

#[test]
fn lines_not_in_head_are_uncommitted() {
    let blame = parse(INCREMENTAL);
    let unsaved = blame.commit_for_line(0).expect("line is blamed");

    assert!(unsaved.is_uncommitted());
    assert!(!blame.commit_for_line(1).expect("line is blamed").is_uncommitted());
}

#[test]
fn output_can_arrive_in_pieces_of_any_size() {
    for output in [PORCELAIN, INCREMENTAL] {
        let whole = parse(output);
        for piece_size in [1, 2, 7, 64, 1000] {
            let mut parser = BlameParser::default();
            for piece in output.chunks(piece_size) {
                parser.feed(piece);
            }
            assert_eq!(parser.finish(), whole, "pieces of {piece_size} bytes");
        }
    }
}

#[test]
fn text_of_a_line_is_never_read_as_a_header() {
    // A file whose own text is blame output, with CRLF line endings.
    let output = format!(
        "{ADA} 1 1 2\nauthor Ada\nfilename a.txt\n\t{GRACE} 9 9 9\r\n{ADA} 2 2\n\tauthor Mallory\r\n"
    );
    let blame = parse(output.as_bytes());

    assert_eq!(shas(&blame, 0..3), [ADA, ADA, ""]);
    assert_eq!(blame.commits().len(), 1);
    assert_eq!(blame.commit_for_line(0).map(|c| c.author.as_str()), Some("Ada"));
}

#[test]
fn author_name_that_is_not_utf8_is_kept_readable() {
    // "José" in Latin-1: the é is a lone 0xE9 byte.
    let mut output = format!("{ADA} 1 1 1\nauthor Jos").into_bytes();
    output.push(0xE9);
    output.extend_from_slice(b" Garc\xEDa\nauthor-mail <jose@example.com>\nsummary Fix\nfilename a.txt\n");
    let blame = parse(&output);
    let commit = blame.commit_for_line(0).expect("line is blamed");

    assert_eq!(commit.author, "Jos\u{FFFD} Garc\u{FFFD}a");
    assert_eq!(commit.author_mail, "jose@example.com");
    assert_eq!(commit.summary, "Fix");
}

#[test]
fn output_cut_short_keeps_the_lines_already_attributed() {
    // Stopped in the middle of the second group's description.
    let cut = INCREMENTAL
        .windows(b"committer Grace".len())
        .position(|window| window == b"committer Grace")
        .expect("marker is in the output");
    let blame = parse(&INCREMENTAL[..cut]);

    assert_eq!(shas(&blame, 0..5), [UNCOMMITTED, "", "", "", GRACE]);
    assert_eq!(blame.commit_for_line(4).map(|c| c.author.as_str()), Some("Grace Hopper"));
}

#[test]
fn garbage_and_empty_output_blame_nothing() {
    assert_eq!(parse(b""), FileBlame::default());
    assert_eq!(parse(b"fatal: no such path 'x' in HEAD\n"), FileBlame::default());
    // A line number no file has must not make the parser allocate for it.
    let huge = format!("{ADA} 1 4000000000 5\n");
    assert_eq!(parse(huge.as_bytes()).blamed_line_count(), 0);
    // Line numbers count from one.
    let zero = format!("{ADA} 1 0 1\n");
    assert_eq!(parse(zero.as_bytes()).blamed_line_count(), 0);
}

#[test]
fn sha256_ids_are_accepted() {
    let sha = "a".repeat(64);
    let blame = parse(format!("{sha} 1 1 1\nauthor Ada\nfilename a.txt\n").as_bytes());

    assert_eq!(blame.commit_for_line(0).map(|c| c.sha.as_str()), Some(sha.as_str()));
}

#[test]
fn time_zone_offsets_convert_to_minutes() {
    assert_eq!(parse_tz_minutes("+0000"), 0);
    assert_eq!(parse_tz_minutes("+0530"), 330);
    assert_eq!(parse_tz_minutes("-0800"), -480);
    assert_eq!(parse_tz_minutes("-0330"), -210);
    assert_eq!(parse_tz_minutes(""), 0);
    assert_eq!(parse_tz_minutes("nonsense"), 0);
}
