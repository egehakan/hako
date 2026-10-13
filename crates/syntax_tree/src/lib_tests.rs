use super::*;

fn ranges(spans: &[(usize, usize)]) -> RangeSet<CharOffset> {
    spans
        .iter()
        .map(|(start, end)| CharOffset::from(*start)..CharOffset::from(*end))
        .collect()
}

#[test]
fn buffers_over_either_parse_limit_are_left_unparsed() {
    assert!(!exceeds_parse_limits(0, 0));
    assert!(!exceeds_parse_limits(MAX_PARSE_BYTES, MAX_PARSE_LINES));
    assert!(exceeds_parse_limits(MAX_PARSE_BYTES + 1, 10));
    // Short lines: small in bytes, but as many syntax nodes as a file many times its size.
    assert!(exceeds_parse_limits(300_000, MAX_PARSE_LINES + 1));
}

#[test]
fn a_screen_of_code_is_highlighted_and_a_minified_line_is_not() {
    assert!(!exceeds_highlight_limit(&ranges(&[])));
    assert!(!exceeds_highlight_limit(&ranges(&[(1, 6_000)])));
    assert!(!exceeds_highlight_limit(&ranges(&[(
        1,
        MAX_HIGHLIGHT_CHARS + 1
    )])));
    assert!(exceeds_highlight_limit(&ranges(&[(
        1,
        MAX_HIGHLIGHT_CHARS + 2
    )])));
    // The limit is on everything asked for at once, not on each range.
    assert!(exceeds_highlight_limit(&ranges(&[
        (1, 150_000),
        (200_000, 350_000)
    ])));
}
