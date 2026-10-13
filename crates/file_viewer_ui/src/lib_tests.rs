use super::*;

#[test]
fn documents_open_fitted_to_the_width_and_images_to_the_window() {
    assert_eq!(default_zoom_mode(ViewerKind::Pdf), ZoomMode::FitWidth);
    assert_eq!(default_zoom_mode(ViewerKind::Office), ZoomMode::FitWidth);
    assert_eq!(default_zoom_mode(ViewerKind::Image), ZoomMode::Fit);
    assert_eq!(default_zoom_mode(ViewerKind::Svg), ZoomMode::Fit);
}

#[test]
fn a_cell_is_laid_out_on_one_line_and_bounded() {
    assert_eq!(cell_text("line one\nline\ttwo"), "line one line two");
    assert_eq!(cell_text("şehir"), "şehir");
    let long = "x".repeat(1000);
    assert_eq!(cell_text(&long).chars().count(), TABLE_MAX_CELL_CHARS);
}

#[test]
fn the_row_number_column_fits_the_last_row_number() {
    let narrow = row_number_width(7, 13.);
    let wide = row_number_width(1_000_000, 13.);
    assert_eq!(narrow, row_number_width(99, 13.));
    assert!(wide > narrow);
    assert_eq!(wide, 7. * 13. * MONOSPACE_ADVANCE + 2. * TABLE_CELL_PADDING);
}

#[test]
fn rectangles_convert_to_the_renderer_type() {
    let rect = to_rect_f(Rect::new(1., 2., 30., 40.));
    assert_eq!(rect.origin(), vec2f(1., 2.));
    assert_eq!(rect.size(), vec2f(30., 40.));
}

#[test]
fn every_key_binding_is_a_valid_keystroke() {
    // Building a binding panics on a keystroke the keymap cannot parse.
    let (editable, fixed) = bindings();
    assert_eq!(editable.len(), 5);
    // Ten digits, enter and escape.
    assert_eq!(fixed.len(), 12);
}

#[test]
fn the_busy_dots_sit_centred_under_the_message() {
    let dots = busy_dots(Size::new(400., 300.));
    assert_eq!(dots.len(), BUSY_DOTS);
    let left = dots[0].x;
    let right = dots[BUSY_DOTS - 1].right();
    assert_eq!(left, 400. - right);
    assert!(
        dots.iter()
            .all(|dot| dot.y == 178. && dot.height == BUSY_DOT_SIZE)
    );
    assert_eq!(dots[1].x - dots[0].x, BUSY_DOT_SIZE + BUSY_DOT_GAP);
}

#[test]
fn peeked_lines_are_drawn_on_one_row_each_and_say_when_they_go_on() {
    use warp_util::large_file::{Peek, PeekLine};

    let line = |text: &str, truncated| PeekLine {
        text: text.to_string(),
        truncated,
    };
    let text = LargeText {
        bytes: 1_000_000_000,
        shape: None,
        peek: Peek {
            lines: vec![line("a\tb", false), line("long", true), line("", false)],
            has_more: true,
        },
    };
    assert_eq!(
        peek_rows(&text),
        [
            "a b",
            "long  … line truncated",
            "",
            "… the rest of the file is not shown"
        ]
    );

    let whole = LargeText {
        peek: Peek {
            lines: vec![line("only", false)],
            has_more: false,
        },
        ..text
    };
    assert_eq!(peek_rows(&whole), ["only"]);
}

#[test]
fn only_whole_peek_rows_count_as_visible() {
    assert_eq!(peek_visible_rows(0.), 0);
    assert_eq!(peek_visible_rows(PEEK_ROW_HEIGHT * 10.), 10);
    assert_eq!(peek_visible_rows(PEEK_ROW_HEIGHT * 10.9), 10);
    assert_eq!(peek_visible_rows(-5.), 0);
}
