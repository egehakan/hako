use super::*;
fn config(
    mode: CodeEditorLineNumberMode,
    starting_line_number: Option<usize>,
    active_line_number: Option<LineCount>,
) -> LineNumberConfig {
    LineNumberConfig {
        font_family: FamilyId(0),
        font_size: 0.,
        text_color: ColorU::transparent_black(),
        highlight_text_color: ColorU::transparent_black(),
        starting_line_number,
        mode,
        active_line_number,
        active_cursor_is_visible: true,
    }
}

#[test]
fn absolute_line_numbers_default_to_one_based_values() {
    let config = config(CodeEditorLineNumberMode::Absolute, None, None);

    assert_eq!(config.absolute_line_number(LineCount::from(0)), 1);
    assert_eq!(config.absolute_line_number(LineCount::from(4)), 5);
}

#[test]
fn absolute_line_numbers_honor_starting_line_number() {
    let config = config(CodeEditorLineNumberMode::Absolute, Some(10), None);

    assert_eq!(config.absolute_line_number(LineCount::from(0)), 10);
    assert_eq!(config.absolute_line_number(LineCount::from(4)), 14);
}

#[test]
fn relative_line_numbers_show_absolute_value_on_active_line() {
    let config = config(
        CodeEditorLineNumberMode::Relative,
        None,
        Some(LineCount::from(4)),
    );
    assert_eq!(config.display_line_number(LineCount::from(4)), 5);
}

#[test]
fn relative_line_numbers_show_distance_above_and_below_active_line() {
    let config = config(
        CodeEditorLineNumberMode::Relative,
        None,
        Some(LineCount::from(5)),
    );
    assert_eq!(config.display_line_number(LineCount::from(2)), 3);
    assert_eq!(config.display_line_number(LineCount::from(8)), 3);
}

#[test]
fn relative_line_numbers_fall_back_to_absolute_without_active_line() {
    let config = config(CodeEditorLineNumberMode::Relative, None, None);
    assert_eq!(config.display_line_number(LineCount::from(4)), 5);
}

#[test]
fn relative_line_numbers_use_starting_line_number_for_active_line_only() {
    let config = config(
        CodeEditorLineNumberMode::Relative,
        Some(10),
        Some(LineCount::from(4)),
    );
    assert_eq!(config.display_line_number(LineCount::from(4)), 14);
    assert_eq!(config.display_line_number(LineCount::from(1)), 3);
}

/// The last character of a line whose final visual row starts at `y` and ends at `end_x`.
fn line_end(end_x: f32, y: f32) -> RectF {
    RectF::new(vec2f(end_x - 8., y), vec2f(8., 20.))
}

#[test]
fn trailing_annotation_starts_a_gap_after_the_line_end() {
    assert_eq!(
        trailing_annotation_placement(line_end(120., 40.), 800., 32., 96.),
        Some((vec2f(152., 40.), 648.))
    );
}

#[test]
fn trailing_annotation_follows_the_last_row_of_a_soft_wrapped_line() {
    // A line wrapped onto three rows of 20px: its end is on the third row, 60px wide.
    let last_row = line_end(60., 40.);
    let (origin, max_width) = trailing_annotation_placement(last_row, 400., 32., 96.)
        .expect("there is room after the last row");
    assert_eq!(origin, vec2f(92., 40.));
    assert_eq!(max_width, 308.);
}

#[test]
fn trailing_annotation_is_hidden_when_the_line_leaves_too_little_room() {
    // Exactly the minimum still fits.
    assert!(trailing_annotation_placement(line_end(672., 0.), 800., 32., 96.).is_some());
    assert_eq!(
        trailing_annotation_placement(line_end(673., 0.), 800., 32., 96.),
        None
    );
    // A line that runs past the right edge.
    assert_eq!(
        trailing_annotation_placement(line_end(5000., 0.), 800., 32., 96.),
        None
    );
}

#[test]
fn trailing_annotation_moves_with_horizontal_scrolling() {
    // The same line end, seen after scrolling 300px to the right: the annotation moves left
    // with the text and gains the room that frees up.
    let unscrolled = trailing_annotation_placement(line_end(700., 0.), 800., 32., 96.);
    let scrolled = trailing_annotation_placement(line_end(400., 0.), 800., 32., 96.);
    assert_eq!(unscrolled, None);
    assert_eq!(scrolled, Some((vec2f(432., 0.), 368.)));

    // Scrolled until the line end is left of the viewport: the annotation keeps its place after
    // the text, part of it off-screen.
    assert_eq!(
        trailing_annotation_placement(line_end(-100., 0.), 800., 32., 96.),
        Some((vec2f(-68., 0.), 868.))
    );
}
