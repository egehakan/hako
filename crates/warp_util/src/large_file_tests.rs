use std::io::{self, Read};

use super::*;

/// Hands out its bytes a few at a time, the way a file read in small chunks would.
struct Trickle<'a> {
    bytes: &'a [u8],
    step: usize,
}

impl Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = self.step.min(buf.len()).min(self.bytes.len());
        buf[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}

fn shape(bytes: u64, lines: usize, longest_line_bytes: usize) -> FileShape {
    FileShape {
        bytes,
        lines,
        longest_line_bytes,
        binary: false,
    }
}

fn texts(peek: &Peek) -> Vec<&str> {
    peek.lines.iter().map(|line| line.text.as_str()).collect()
}

#[test]
fn size_alone_decides_only_for_files_over_the_automatic_limit() {
    assert_eq!(decision_for_size(0), None);
    assert_eq!(decision_for_size(AUTO_OPEN_MAX_BYTES), None);
    assert_eq!(
        decision_for_size(AUTO_OPEN_MAX_BYTES + 1),
        Some(OpenDecision::LargeFile {
            limit: Limit::Bytes,
            can_open_anyway: true,
        })
    );
    assert_eq!(
        decision_for_size(OPEN_ANYWAY_MAX_BYTES),
        Some(OpenDecision::LargeFile {
            limit: Limit::Bytes,
            can_open_anyway: true,
        })
    );
    assert_eq!(
        decision_for_size(OPEN_ANYWAY_MAX_BYTES + 1),
        Some(OpenDecision::LargeFile {
            limit: Limit::Bytes,
            can_open_anyway: false,
        })
    );
}

#[test]
fn a_dump_of_hundreds_of_megabytes_is_never_offered_to_the_editor() {
    let dump = shape(639 * 1024 * 1024, 630_788, 7_863_739);
    assert_eq!(
        dump.decision(),
        OpenDecision::LargeFile {
            limit: Limit::Bytes,
            can_open_anyway: false,
        }
    );
    assert_eq!(
        decision_for_size(dump.bytes),
        Some(OpenDecision::LargeFile {
            limit: Limit::Bytes,
            can_open_anyway: false,
        })
    );
    // More than any machine this runs on has.
    assert!(dump.estimated_editor_bytes() > 32 * 1024 * 1024 * 1024);
}

#[test]
fn small_files_open_with_every_feature() {
    assert_eq!(
        shape(0, 0, 0).decision(),
        OpenDecision::Editor(EditorMode::Full)
    );
    assert_eq!(
        shape(FULL_EDITOR_MAX_BYTES, FULL_EDITOR_MAX_LINES, 200).decision(),
        OpenDecision::Editor(EditorMode::Full)
    );
}

#[test]
fn files_over_the_full_limits_open_in_reduced_mode() {
    assert_eq!(
        shape(FULL_EDITOR_MAX_BYTES + 1, 10, 200).decision(),
        OpenDecision::Editor(EditorMode::Reduced)
    );
    assert_eq!(
        shape(1_000_000, FULL_EDITOR_MAX_LINES + 1, 20).decision(),
        OpenDecision::Editor(EditorMode::Reduced)
    );
    assert_eq!(
        shape(AUTO_OPEN_MAX_BYTES, AUTO_OPEN_MAX_LINES, 200).decision(),
        OpenDecision::Editor(EditorMode::Reduced)
    );
}

#[test]
fn too_many_lines_keep_a_small_file_out_of_the_editor() {
    assert_eq!(
        shape(1_000_000, AUTO_OPEN_MAX_LINES + 1, 5).decision(),
        OpenDecision::LargeFile {
            limit: Limit::Lines,
            can_open_anyway: true,
        }
    );
    assert_eq!(
        shape(9_000_000, OPEN_ANYWAY_MAX_LINES + 1, 5).decision(),
        OpenDecision::LargeFile {
            limit: Limit::Lines,
            can_open_anyway: false,
        }
    );
}

#[test]
fn a_very_long_line_keeps_a_file_out_of_the_editor_until_asked() {
    let minified = shape(5_000_000, 1, 5_000_000);
    assert_eq!(
        minified.decision(),
        OpenDecision::LargeFile {
            limit: Limit::LineLength,
            can_open_anyway: true,
        }
    );
    assert_eq!(minified.exceeded_limit(OpenIntent::Anyway), None);
    assert_eq!(
        shape(900_000, 1, AUTO_OPEN_MAX_LINE_BYTES).decision(),
        OpenDecision::Editor(EditorMode::Full)
    );
}

#[test]
fn asking_lifts_the_automatic_limits_but_not_the_ceiling() {
    let log = shape(60 * 1024 * 1024, 530_000, 130);
    assert_eq!(
        log.exceeded_limit(OpenIntent::Automatic),
        Some(Limit::Bytes)
    );
    assert_eq!(log.exceeded_limit(OpenIntent::Anyway), None);
    assert_eq!(log.editor_mode(), EditorMode::Reduced);

    let dump = shape(OPEN_ANYWAY_MAX_BYTES + 1, 10, 100);
    assert_eq!(dump.exceeded_limit(OpenIntent::Anyway), Some(Limit::Bytes));
}

#[test]
fn the_memory_estimate_follows_the_measured_cost() {
    // The 60 MB log of 531,569 lines that was measured at 5.5 GB over an empty editor.
    let log = shape(62_914_646, 531_569, 130);
    let estimate = log.estimated_editor_bytes() as f64 / (1024. * 1024. * 1024.);
    assert!((5.0..6.0).contains(&estimate), "{estimate}");
}

#[test]
fn shape_counts_lines_with_and_without_a_trailing_newline() {
    assert_eq!(FileShape::of_bytes(b""), shape(0, 0, 0));
    assert_eq!(FileShape::of_bytes(b"\n"), shape(1, 1, 0));
    assert_eq!(FileShape::of_bytes(b"one\ntwo\n"), shape(8, 2, 3));
    assert_eq!(FileShape::of_bytes(b"one\nlonger"), shape(10, 2, 6));
    // The carriage return is part of the line as far as its length goes.
    assert_eq!(FileShape::of_bytes(b"ab\r\ncd\r\n"), shape(8, 2, 3));
}

#[test]
fn shape_notices_binary_content() {
    assert!(FileShape::of_bytes(b"MZ\0\0binary").binary);
    assert!(!FileShape::of_bytes("şehir\n".as_bytes()).binary);
}

#[test]
fn scanning_a_reader_gives_the_shape_of_its_bytes() {
    let mut content = Vec::new();
    for index in 0..500 {
        content.extend_from_slice(format!("line {index}\n").as_bytes());
    }
    content.extend_from_slice(&[b'x'; 5_000]);
    content.extend_from_slice(b"\nlast without newline");

    let expected = FileShape::of_bytes(&content);
    assert_eq!(expected.lines, 502);
    assert_eq!(expected.longest_line_bytes, 5_000);
    for step in [1, 7, 4096] {
        let scanned = FileShape::scan(Trickle {
            bytes: &content,
            step,
        })
        .unwrap();
        assert_eq!(scanned, expected, "step {step}");
    }
}

#[test]
fn peek_shows_short_files_whole() {
    let peek = read_peek("first\nsecond\n".as_bytes()).unwrap();
    assert_eq!(texts(&peek), ["first", "second"]);
    assert!(peek.lines.iter().all(|line| !line.truncated));
    assert!(!peek.has_more);

    assert_eq!(read_peek(&b""[..]).unwrap(), Peek::default());
}

#[test]
fn peek_keeps_a_last_line_without_a_newline() {
    let peek = read_peek("first\nlast".as_bytes()).unwrap();
    assert_eq!(texts(&peek), ["first", "last"]);
    assert!(!peek.lines[1].truncated);
    assert!(!peek.has_more);
}

#[test]
fn peek_drops_the_carriage_return_of_crlf_lines() {
    let peek = read_peek("one\r\ntwo\r\nthree".as_bytes()).unwrap();
    assert_eq!(texts(&peek), ["one", "two", "three"]);
}

#[test]
fn peek_replaces_bytes_that_are_not_utf8() {
    let peek = read_peek(&b"ok\n\xff\xfebad\nfine\n"[..]).unwrap();
    assert_eq!(texts(&peek), ["ok", "\u{fffd}\u{fffd}bad", "fine"]);
}

#[test]
fn peek_cuts_long_lines_and_goes_on_with_the_next() {
    let content = format!("{}\nshort\n", "x".repeat(10_000));
    let peek = read_peek_within(content.as_bytes(), 300, 100, 1024 * 1024).unwrap();
    assert_eq!(peek.lines.len(), 2);
    assert_eq!(peek.lines[0].text, "x".repeat(100));
    assert!(peek.lines[0].truncated);
    assert_eq!(peek.lines[1].text, "short");
    assert!(!peek.lines[1].truncated);
    assert!(!peek.has_more);
}

#[test]
fn peek_cuts_on_a_character_boundary() {
    // Three bytes each, so the byte cut lands inside one.
    let content = "€".repeat(1_000);
    for step in [1, 3, 64] {
        let reader = Trickle {
            bytes: content.as_bytes(),
            step,
        };
        let peek = read_peek_within(reader, 300, 10, 1024 * 1024).unwrap();
        assert_eq!(peek.lines[0].text, "€".repeat(10), "step {step}");
        assert!(peek.lines[0].truncated);
    }
}

#[test]
fn peek_stops_at_the_line_limit() {
    let content = "line\n".repeat(50);
    let peek = read_peek_within(content.as_bytes(), 10, 100, 1024 * 1024).unwrap();
    assert_eq!(peek.lines.len(), 10);
    assert!(peek.has_more);

    let exact = read_peek_within("a\nb\n".as_bytes(), 2, 100, 1024 * 1024).unwrap();
    assert_eq!(texts(&exact), ["a", "b"]);
    assert!(!exact.has_more);
}

#[test]
fn peek_gives_up_on_a_line_longer_than_what_it_scans() {
    // One line with no end in sight, as in a file that is a single 200 MB line.
    let content = vec![b'y'; 300_000];
    let peek = read_peek_within(&content[..], 300, 50, 100_000).unwrap();
    assert_eq!(peek.lines.len(), 1);
    assert_eq!(peek.lines[0].text, "y".repeat(50));
    assert!(peek.lines[0].truncated);
    assert!(peek.has_more);
}

#[test]
fn peek_reads_no_further_than_its_scan_limit() {
    struct Counting<'a> {
        inner: &'a [u8],
        read: &'a mut usize,
    }
    impl Read for Counting<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let count = self.inner.read(buf)?;
            *self.read += count;
            Ok(count)
        }
    }

    let content = vec![b'z'; 2_000_000];
    let mut read = 0;
    let reader = Counting {
        inner: &content,
        read: &mut read,
    };
    read_peek_within(reader, 300, 50, 100_000).unwrap();
    assert_eq!(read, 100_000);
}
