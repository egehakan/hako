use super::*;

fn unfinished_after(output: &[u8]) -> Vec<u8> {
    let mut boundary = Boundary::new();
    boundary.assume_ground();
    boundary.feed(output);
    boundary.unfinished().to_vec()
}

#[test]
fn has_nothing_unfinished_between_pieces() {
    assert!(unfinished_after(b"").is_empty());
    assert!(unfinished_after(b"plain text\r\n").is_empty());
    assert!(unfinished_after(b"\x1b[1;31mred\x1b[0m").is_empty());
    assert!(unfinished_after(b"\x1b]0;title\x07").is_empty());
    assert!(unfinished_after(b"\x1b]0;title\x1b\\").is_empty());
    assert!(unfinished_after(b"\x1bP$d7b7d\x1b\\prompt").is_empty());
    assert!(unfinished_after(b"\x1b(0\x1b7\x1b=").is_empty());
    assert!(unfinished_after("h\u{e9}llo \u{4e16}\u{754c} \u{1f600}".as_bytes()).is_empty());
}

#[test]
fn keeps_the_start_of_an_unfinished_sequence() {
    assert_eq!(unfinished_after(b"text\x1b"), b"\x1b");
    assert_eq!(unfinished_after(b"text\x1b[38;5"), b"\x1b[38;5");
    assert_eq!(unfinished_after(b"\x1b[0m\x1b]0;tit"), b"\x1b]0;tit");
    assert_eq!(unfinished_after(b"\x1b]0;title\x1b"), b"\x1b]0;title\x1b");
    assert_eq!(unfinished_after(b"\x1bP$d7b"), b"\x1bP$d7b");
    assert_eq!(unfinished_after(b"\x1b("), b"\x1b(");
}

#[test]
fn keeps_the_start_of_an_unfinished_character() {
    let bytes = "\u{4e16}".as_bytes();
    assert_eq!(unfinished_after(&bytes[..1]), &bytes[..1]);
    assert_eq!(unfinished_after(&bytes[..2]), &bytes[..2]);
    assert!(unfinished_after(bytes).is_empty());
}

#[test]
fn follows_a_sequence_across_reads() {
    let mut boundary = Boundary::new();
    boundary.assume_ground();
    boundary.feed(b"one\x1b[3");
    boundary.feed(b"8;2;1");
    assert_eq!(boundary.unfinished(), b"\x1b[38;2;1");
    boundary.feed(b";2;3mtwo");
    assert!(boundary.unfinished().is_empty());
}

#[test]
fn is_certain_only_from_the_start_of_a_sequence_when_joined_midway() {
    let mut boundary = Boundary::new();
    assert!(!boundary.is_known());

    // This may be text, or the inside of a string that began earlier.
    boundary.feed(b"8;5;12mtext");
    assert!(!boundary.is_known());
    assert!(boundary.unfinished().is_empty());

    boundary.feed(b"\x1b[1");
    assert!(boundary.is_known());
    assert_eq!(boundary.unfinished(), b"\x1b[1");
}

#[test]
fn starts_over_when_a_sequence_is_cut_short() {
    assert_eq!(unfinished_after(b"\x1b[12\x1b[3"), b"\x1b[3");
    assert!(unfinished_after(b"\x1b[12\x18").is_empty());
    // A string that ESC interrupts without ending is abandoned for the new sequence.
    assert_eq!(unfinished_after(b"\x1b]0;title\x1b[3"), b"\x1b[3");
}

#[test]
fn leaves_out_a_control_character_that_already_took_effect() {
    assert_eq!(unfinished_after(b"\x1b[3\r\n1"), b"\x1b[31");
}

#[test]
fn gives_up_on_a_string_longer_than_it_keeps() {
    let mut boundary = Boundary::new();
    boundary.assume_ground();
    boundary.feed(b"\x1bP");
    boundary.feed(&vec![b'a'; MAX_UNFINISHED + 1]);
    assert!(boundary.unfinished().is_empty());

    boundary.feed(b"\x1b\\\x1b[1");
    assert_eq!(boundary.unfinished(), b"\x1b[1");
}
