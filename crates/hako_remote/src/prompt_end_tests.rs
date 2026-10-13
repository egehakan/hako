use super::*;

#[test]
fn finds_the_end_of_each_prompt() {
    let mut finder = PromptEndFinder::default();
    let output = b"\x1b]133;A\x07one % \x1b]133;B\x07ls\r\n\x1b]133;A\x07two % \x1b]133;B\x07";

    assert_eq!(finder.find(output), [22, output.len()]);
}

#[test]
fn finds_an_end_that_is_split_between_reads() {
    let mut finder = PromptEndFinder::default();

    assert!(finder.find(b"prompt % \x1b]13").is_empty());
    assert_eq!(finder.find(b"3;B\x07typed"), [4]);
    assert!(finder.find(b"more").is_empty());
}

#[test]
fn starts_over_at_an_escape_inside_a_partial_match() {
    let mut finder = PromptEndFinder::default();

    assert_eq!(finder.find(b"\x1b]13\x1b]133;B\x07"), [12]);
    assert!(finder.find(b"\x1b]133;A\x07\x1b]133;C\x07").is_empty());
}
