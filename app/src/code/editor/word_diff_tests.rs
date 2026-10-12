use super::*;

fn token_texts(line: &str) -> Vec<&str> {
    tokenize(line).into_iter().map(|token| token.text).collect()
}

#[test]
fn tokenize_separates_words_whitespace_and_punctuation() {
    assert_eq!(token_texts("foo(bar)"), ["foo", "(", "bar", ")"]);
    assert_eq!(
        token_texts("let snake_case2 =  x1;"),
        ["let", " ", "snake_case2", " ", "=", "  ", "x1", ";"]
    );
    assert_eq!(
        token_texts("a.b::c->d"),
        ["a", ".", "b", ":", ":", "c", "-", ">", "d"]
    );
    assert_eq!(
        token_texts("\tif x != 3.14 {"),
        [
            "\t", "if", " ", "x", " ", "!", "=", " ", "3", ".", "14", " ", "{"
        ]
    );
    assert!(tokenize("").is_empty());
}

#[test]
fn tokenize_keeps_non_ascii_words_and_combined_characters_whole() {
    assert_eq!(
        token_texts("größe = naïve"),
        ["größe", " ", "=", " ", "naïve"]
    );
    // A letter followed by a combining accent is one character of its word.
    assert_eq!(token_texts("cafe\u{301}s!"), ["cafe\u{301}s", "!"]);
    // Emoji joined into one glyph are a single token.
    assert_eq!(
        token_texts("a \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} b"),
        [
            "a",
            " ",
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
            " ",
            "b"
        ]
    );
    assert_eq!(token_texts("Привет, мир"), ["Привет", ",", " ", "мир"]);
}

#[test]
fn tokenize_splits_ideographs_into_single_characters() {
    assert_eq!(token_texts("日本語 text"), ["日", "本", "語", " ", "text"]);
    assert_eq!(token_texts("かな。"), ["か", "な", "。"]);
}

#[test]
fn tokenize_reports_positions_in_characters() {
    let tokens = tokenize("größe=日本 x");
    let positions: Vec<_> = tokens.iter().map(|token| token.chars.clone()).collect();
    assert_eq!(positions, [0..5, 5..6, 6..7, 7..8, 8..9, 9..10]);

    let combined = tokenize("e\u{301}x y");
    assert_eq!(combined[0].chars, 0..3);
    assert_eq!(combined[2].chars, 4..5);
}

#[test]
fn line_changes_mark_only_the_changed_words() {
    assert_eq!(
        line_changes("foo(bar)", "foo(baz)"),
        Some((vec![4..7], vec![4..7]))
    );
    assert_eq!(
        line_changes(
            "    let total = compute(a, b);",
            "    let total = compute(a, b, c);"
        ),
        Some((vec![], vec![28..31]))
    );
    assert_eq!(
        line_changes("return größe + 1;", "return größe + 22;"),
        Some((vec![15..16], vec![15..17]))
    );
}

#[test]
fn line_changes_join_reworded_phrases_across_whitespace() {
    assert_eq!(
        line_changes(
            "the quick brown fox jumps over",
            "the slow red fox jumps over"
        ),
        Some((vec![4..15], vec![4..12]))
    );
}

#[test]
fn line_changes_mark_whitespace_when_only_it_changed() {
    assert_eq!(
        line_changes("  foo(bar)", "      foo(bar)"),
        Some((vec![0..2], vec![0..6]))
    );
}

#[test]
fn line_changes_skip_lines_with_too_little_in_common() {
    assert_eq!(line_changes("Hello World", "Hallo Welt"), None);
    assert_eq!(line_changes("foo(bar);", "quux(zap);"), None);
    assert_eq!(line_changes("x = 1", "y = 2"), None);
    assert_eq!(line_changes("", "anything"), None);
    assert_eq!(line_changes("   ", "\t"), None);
}

#[test]
fn line_changes_skip_lines_past_the_length_limit() {
    let at_limit = format!("{} tail", "a".repeat(MAX_LINE_LEN - 5));
    let edited = format!("{} tale", "a".repeat(MAX_LINE_LEN - 5));
    assert_eq!(at_limit.len(), MAX_LINE_LEN);
    assert!(line_changes(&at_limit, &edited).is_some());

    let too_long = format!("a{at_limit}");
    assert_eq!(line_changes(&too_long, &edited), None);
    assert_eq!(line_changes(&edited, &too_long), None);
}

#[test]
fn paired_lines_skip_an_inserted_line_between_two_edited_ones() {
    let old = ["let first = load(path);", "let second = parse(first);"];
    let new = [
        "let first = load(&path);",
        "log::info!(\"loaded\");",
        "let second = parse(&first);",
    ];
    assert_eq!(paired_lines(&old, &new), [(0, 0), (1, 2)]);
}

#[test]
fn paired_lines_skip_a_removed_line_between_two_edited_ones() {
    let old = [
        "let first = load(path);",
        "log::info!(\"loaded\");",
        "let second = parse(first);",
    ];
    let new = ["let first = load(&path);", "let second = parse(&first);"];
    assert_eq!(paired_lines(&old, &new), [(0, 0), (2, 1)]);
}

#[test]
fn paired_lines_keep_both_sides_in_order() {
    // The lines swapped places, so only one of the two can keep its counterpart.
    let old = ["alpha(one, two)", "beta(three, four)"];
    let new = ["beta(three, four, five)", "alpha(one, two, six)"];
    let pairs = paired_lines(&old, &new);
    assert_eq!(pairs.len(), 1);
    assert!(pairs == [(0, 1)] || pairs == [(1, 0)]);
}

#[test]
fn paired_lines_leave_unrelated_lines_unpaired() {
    let old = ["fn compute(a: u32) -> u32 {", "}"];
    let new = [
        "struct Totally { different: bool }",
        "impl Other for Thing {}",
    ];
    assert!(paired_lines(&old, &new).is_empty());
    assert!(paired_lines(&[], &new).is_empty());
    assert!(paired_lines(&old, &[]).is_empty());
}

#[test]
fn hunk_word_changes_cover_each_paired_line() {
    let old = ["let first = load(path);\n", "let second = parse(first);\n"];
    let new = [
        "let first = load(&path);\n",
        "log::info!(\"loaded\");\n",
        "let second = parse(&first);\n",
    ];
    assert_eq!(
        hunk_word_changes(&old, &new),
        HunkWordChanges {
            removed: vec![vec![], vec![]],
            added: vec![vec![17..18], vec![], vec![19..20]],
        }
    );
}

#[test]
fn hunk_word_changes_ignore_line_terminators() {
    let changes = hunk_word_changes(&["value = 1;\r\n"], &["value = 2;"]);
    assert_eq!(
        changes,
        HunkWordChanges {
            removed: vec![vec![8..9]],
            added: vec![vec![8..9]],
        }
    );
}

#[test]
fn hunk_word_changes_are_empty_for_a_hunk_too_large_to_pair() {
    let old: Vec<String> = (0..=MAX_PAIRING_CELLS / 100)
        .map(|index| format!("let value_{index} = compute(input);"))
        .collect();
    let new: Vec<String> = (0..100)
        .map(|index| format!("let value_{index} = compute(other);"))
        .collect();
    let old_lines: Vec<&str> = old.iter().map(String::as_str).collect();
    let new_lines: Vec<&str> = new.iter().map(String::as_str).collect();
    assert!(old_lines.len() * new_lines.len() > MAX_PAIRING_CELLS);

    let changes = hunk_word_changes(&old_lines, &new_lines);
    assert_eq!(changes.removed.len(), old_lines.len());
    assert!(changes.removed.iter().all(Vec::is_empty));
    assert!(changes.added.iter().all(Vec::is_empty));

    // One line fewer fits, and is paired line by line.
    let changes = hunk_word_changes(&old_lines[..old_lines.len() - 1], &new_lines);
    assert!(changes.added.iter().all(|ranges| !ranges.is_empty()));
}
