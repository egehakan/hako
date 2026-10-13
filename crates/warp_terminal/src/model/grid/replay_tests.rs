use std::io;

use super::*;
use crate::model::ansi::Processor;

const ROWS: usize = 6;
const COLUMNS: usize = 20;

fn empty_grid() -> GridHandler {
    GridHandler::new_for_test_with_scroll_limit(ROWS, COLUMNS, 100)
}

fn feed(grid: &mut GridHandler, bytes: &[u8]) {
    Processor::new().parse_bytes(grid, bytes, &mut io::sink());
}

fn grid(input: &str) -> GridHandler {
    let mut grid = empty_grid();
    feed(&mut grid, input.as_bytes());
    grid
}

/// What each row shows, with its attributes, from the row at `first` on.
fn drawn(grid: &GridHandler, first: usize, count: usize) -> Vec<Vec<(char, Attributes)>> {
    (first..first + count)
        .map(|index| {
            let row = grid.row(index).expect("the row exists");
            (0..drawn_length(&row))
                .filter_map(|column| row.get(column))
                .map(|cell| {
                    let character = if cell.c == DEFAULT_CHAR { ' ' } else { cell.c };
                    (character, Attributes::of(cell))
                })
                .collect()
        })
        .collect()
}

fn replayed(replay: Replay) -> GridHandler {
    let mut grid = empty_grid();
    feed(&mut grid, &replay.into_bytes());
    grid
}

#[test]
fn lines_keep_their_text_attributes_and_wrapping() {
    let original = grid(concat!(
        "plain \x1b[1;31mbold red\x1b[0m\r\n",
        "\x1b[38;5;208;48;2;1;2;3mcolours\x1b[0m and a line that is longer than the grid\r\n",
        "wide \u{4e16}\u{754c} end",
    ));
    let row_count = original.cursor_point().row + 1;

    let mut replay = Replay::new();
    original.replay_lines(0..row_count, &mut replay);
    let copy = replayed(replay);

    assert_eq!(drawn(&copy, 0, row_count), drawn(&original, 0, row_count));
    for row in 0..row_count {
        assert_eq!(copy.row_wraps(row), original.row_wraps(row), "row {row}");
    }
}

#[test]
fn a_run_of_cells_selects_its_attributes_once() {
    let original = grid("\x1b[32mgreen text\x1b[0m plain");
    let mut replay = Replay::new();
    original.replay_lines(0..1, &mut replay);

    assert_eq!(
        String::from_utf8(replay.into_bytes()).unwrap(),
        "\x1b[0;32mgreen text\x1b[0m plain\r\n"
    );
}

#[test]
fn a_screen_is_redrawn_with_its_cursor_modes_and_scroll_region() {
    let original = grid(concat!(
        "\x1b[2;3Hhi \x1b[4;33myellow\x1b[0m",
        "\x1b[5;1Hbottom",
        "\x1b[2;5r\x1b[?1h\x1b[?2004h\x1b[?25l\x1b[?1000h\x1b[?1006h",
        "\x1b[3;4H\x1b[7m",
    ));

    let mut replay = Replay::new();
    original.replay_screen(&mut replay);
    original.replay_modes(original.term_mode(), &mut replay);
    let mut copy = replayed(replay);

    assert_eq!(drawn(&copy, 0, ROWS), drawn(&original, 0, ROWS));
    assert_eq!(copy.cursor_point(), original.cursor_point());
    assert_eq!(copy.scroll_region(), original.scroll_region());
    for mode in [
        TermMode::APP_CURSOR,
        TermMode::BRACKETED_PASTE,
        TermMode::SHOW_CURSOR,
        TermMode::MOUSE_REPORT_CLICK,
        TermMode::SGR_MOUSE,
        TermMode::LINE_WRAP,
    ] {
        assert_eq!(copy.is_mode_set(mode), original.is_mode_set(mode));
    }

    // What is written next takes the attributes that were in effect at the cursor.
    let mut original = original;
    feed(&mut original, b"x");
    feed(&mut copy, b"x");
    assert_eq!(drawn(&copy, 0, ROWS), drawn(&original, 0, ROWS));
}

#[test]
fn a_screen_follows_its_history() {
    let original = grid("one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight");
    let history = original.history_size();
    assert!(history > 0);

    let mut replay = Replay::new();
    original.replay_lines(0..history, &mut replay);
    replay.control(&"\n".repeat(ROWS));
    original.replay_screen(&mut replay);
    let copy = replayed(replay);

    assert_eq!(
        drawn(&copy, copy.history_size(), ROWS),
        drawn(&original, history, ROWS)
    );
    assert_eq!(
        copy.cursor_point().row - copy.history_size(),
        original.cursor_point().row - history
    );
    assert_eq!(drawn(&copy, 0, history), drawn(&original, 0, history));
}

#[test]
fn a_cursor_waiting_to_wrap_still_wraps() {
    let full_row = "x".repeat(COLUMNS);
    let mut original = grid(&full_row);

    let mut replay = Replay::new();
    original.replay_screen(&mut replay);
    let mut copy = replayed(replay);

    feed(&mut original, b"y");
    feed(&mut copy, b"y");
    assert_eq!(drawn(&copy, 0, ROWS), drawn(&original, 0, ROWS));
    assert_eq!(copy.cursor_point(), original.cursor_point());
}

#[test]
fn text_around_the_cursor_leaves_the_cursor_in_place() {
    // The cursor is moved back onto the "c" of "cd".
    let original = grid("prompt % cd dir\x1b[6D");

    let mut replay = Replay::new();
    original.replay_lines_around_cursor(1, &mut replay);
    let copy = replayed(replay);

    assert_eq!(drawn(&copy, 0, 1), drawn(&original, 0, 1));
    assert_eq!(copy.cursor_point(), original.cursor_point());
}
