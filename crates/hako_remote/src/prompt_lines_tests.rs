use super::*;

const START: &str = "\x1b]133;A\x07";
const END: &str = "\x1b]133;B\x07";

/// What a client receives for output that arrives as `pieces`.
fn rewritten(lines: &mut PromptLines, pieces: &[&str]) -> String {
    let mut sent = Vec::new();
    for piece in pieces {
        lines.rewrite(piece.as_bytes(), |bytes| sent.extend_from_slice(bytes));
    }
    String::from_utf8(sent).unwrap()
}

#[test]
fn breaks_the_line_after_each_prompt() {
    let mut lines = PromptLines::default();
    let output = format!("{START}one % {END}ls\r\nfile\r\n{START}two % {END}");

    assert_eq!(
        rewritten(&mut lines, &[&output]),
        format!("{START}one % {END}\r\nls\r\nfile\r\n{START}two % {END}\r\n")
    );
}

#[test]
fn finds_a_mark_that_is_split_between_reads() {
    let mut lines = PromptLines::default();

    assert_eq!(
        rewritten(&mut lines, &["% \x1b]13", "3;B\x07typed", "more"]),
        "% \x1b]133;B\x07\r\ntypedmore"
    );
}

#[test]
fn starts_over_at_an_escape_inside_a_partial_mark() {
    let mut lines = PromptLines::default();

    assert_eq!(
        rewritten(&mut lines, &["\x1b]13\x1b]133;B\x07", "\x1b]133;C\x07"]),
        "\x1b]13\x1b]133;B\x07\r\n\x1b]133;C\x07"
    );
}

#[test]
fn draws_a_prompt_again_where_it_was() {
    let mut lines = PromptLines::default();
    let prompt = format!("{START}% {END}");
    let redraw = format!("\r\x1b[J{prompt}\x1b[Ktyped");

    assert_eq!(
        rewritten(&mut lines, &[&prompt, "typed", &redraw, &redraw]),
        format!(
            "{prompt}\r\ntyped\
             \r\x1b[J{START}\x1b[1A\x1b[J% {END}\r\n\x1b[Ktyped\
             \r\x1b[J{START}\x1b[1A\x1b[J% {END}\r\n\x1b[Ktyped"
        )
    );
}

#[test]
fn goes_up_by_the_rows_of_the_prompt() {
    let mut lines = PromptLines::default();
    let prompt = format!("{START}dir\r\n% {END}");

    assert_eq!(
        rewritten(&mut lines, &[&prompt, "\r", &prompt]),
        format!("{START}dir\r\n% {END}\r\n\r{START}\x1b[2A\x1b[Jdir\r\n% {END}\r\n")
    );
}

#[test]
fn leaves_a_prompt_that_follows_a_new_line_where_it_is() {
    let mut lines = PromptLines::default();
    let prompt = format!("{START}% {END}");

    assert_eq!(
        rewritten(&mut lines, &[&prompt, "ls\r\nfile\r\n", &prompt]),
        format!("{prompt}\r\nls\r\nfile\r\n{prompt}\r\n")
    );
}

#[test]
fn takes_the_place_of_the_cursor_from_a_snapshot() {
    let mut lines = PromptLines::default();
    let prompt = format!("{START}% {END}");

    lines.set_prompt_above(Some(2));
    assert_eq!(
        rewritten(&mut lines, &[&prompt]),
        format!("{START}\x1b[2A\x1b[J% {END}\r\n")
    );

    lines.set_prompt_above(None);
    assert_eq!(rewritten(&mut lines, &[&prompt]), format!("{prompt}\r\n"));
}
