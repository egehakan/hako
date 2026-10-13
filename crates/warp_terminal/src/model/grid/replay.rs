//! Writes a grid as the bytes that reproduce it in another terminal emulator: its text with the
//! attributes of every cell, and the state that decides how later output is drawn and how input
//! is encoded.

use std::fmt::Write as _;
use std::ops::Range;

use super::{GridHandler, TermMode};
use crate::model::ansi::{CharsetIndex, Color, CursorShape, NamedColor, StandardCharset};
use crate::model::cell::{Cell, DEFAULT_CHAR, Flags};
use crate::model::char_or_str::PushCharOrStr as _;
use crate::model::grid::Dimensions as _;
use crate::model::grid::row::Row;
use crate::model::index::Point;

const ATTRIBUTE_FLAGS: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::UNDERLINE)
    .union(Flags::DOUBLE_UNDERLINE)
    .union(Flags::INVERSE)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT);

const SPACER_FLAGS: Flags = Flags::WIDE_CHAR_SPACER.union(Flags::LEADING_WIDE_CHAR_SPACER);

/// The attributes a cell is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Attributes {
    foreground: Color,
    background: Color,
    flags: Flags,
}

impl Attributes {
    fn of(cell: &Cell) -> Self {
        Self {
            foreground: cell.fg,
            background: cell.bg,
            flags: cell.flags.intersection(ATTRIBUTE_FLAGS),
        }
    }
}

impl Default for Attributes {
    fn default() -> Self {
        Self {
            foreground: Color::Named(NamedColor::Foreground),
            background: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
        }
    }
}

/// The bytes written so far, together with the attributes the replaying terminal has selected,
/// so that a run of cells drawn alike selects them once.
#[derive(Default)]
pub struct Replay {
    output: String,
    selected: Attributes,
}

impl Replay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.output.len()
    }

    pub fn is_empty(&self) -> bool {
        self.output.is_empty()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.output.into_bytes()
    }

    /// Appends a control sequence that leaves the selected attributes as they are.
    pub fn control(&mut self, sequence: &str) {
        self.output.push_str(sequence);
    }

    /// Ends the line. The attributes are cleared first, because a terminal that scrolls for the
    /// new line fills it with the selected background.
    pub fn line_break(&mut self) {
        self.select(Attributes::default());
        self.output.push_str("\r\n");
    }

    fn select(&mut self, attributes: Attributes) {
        if attributes == self.selected {
            return;
        }
        self.selected = attributes;
        self.output.push_str("\x1b[0");
        for (flag, parameter) in [
            (Flags::BOLD, ";1"),
            (Flags::DIM, ";2"),
            (Flags::ITALIC, ";3"),
            (Flags::UNDERLINE, ";4"),
            (Flags::DOUBLE_UNDERLINE, ";21"),
            (Flags::INVERSE, ";7"),
            (Flags::HIDDEN, ";8"),
            (Flags::STRIKEOUT, ";9"),
        ] {
            if attributes.flags.contains(flag) {
                self.output.push_str(parameter);
            }
        }
        self.push_color(attributes.foreground, Layer::Foreground);
        self.push_color(attributes.background, Layer::Background);
        self.output.push('m');
    }

    fn push_color(&mut self, color: Color, layer: Layer) {
        let (extended, named_code) = match layer {
            Layer::Foreground => (38, NamedColor::to_ansi_fg_escape_code as NamedCode),
            Layer::Background => (48, NamedColor::to_ansi_bg_escape_code as NamedCode),
        };
        // Writing to a String cannot fail.
        let _ = match color {
            // A name with no code of its own is the default colour, which the leading 0 selects.
            Color::Named(name) => match named_code(&name) {
                Ok(code) if code != 39 && code != 49 => write!(self.output, ";{code}"),
                Ok(_) | Err(_) => Ok(()),
            },
            Color::Indexed(index) => write!(self.output, ";{extended};5;{index}"),
            Color::Spec(color) => write!(
                self.output,
                ";{extended};2;{};{};{}",
                color.r, color.g, color.b
            ),
        };
    }

    fn cell(&mut self, cell: &Cell) {
        self.select(Attributes::of(cell));
        self.output.push_char_or_str(cell.content_for_display());
    }

    fn cursor_position(&mut self, row: usize, column: usize) {
        let _ = write!(self.output, "\x1b[{};{}H", row + 1, column + 1);
    }
}

type NamedCode = fn(&NamedColor) -> anyhow::Result<u8>;

#[derive(Clone, Copy)]
enum Layer {
    Foreground,
    Background,
}

/// The number of leading cells of `row` that hold anything a blank line would not show.
fn drawn_length(row: &Row) -> usize {
    if row
        .last()
        .is_some_and(|cell| cell.flags.contains(Flags::WRAPLINE))
    {
        return row.len();
    }
    let blank_background = Color::Named(NamedColor::Background);
    (0..row.len())
        .rev()
        .find(|column| {
            row.get(*column)
                .is_some_and(|cell| cell.c != DEFAULT_CHAR || cell.bg != blank_background)
        })
        .map_or(0, |column| column + 1)
}

impl GridHandler {
    /// Appends the text from `start` up to, not including, `end`. A row that wraps into the
    /// next is followed by no line break, so that it wraps again by itself.
    pub fn replay_text(&self, start: Point, end: Point, replay: &mut Replay) {
        for index in start.row..=end.row {
            let is_last = index == end.row;
            if is_last && end.col == 0 {
                break;
            }
            let Some(row) = self.row(index) else {
                break;
            };
            let first_column = if index == start.row { start.col } else { 0 };
            let end_column = if is_last { end.col } else { row.len() };
            self.replay_cells(&row, first_column..end_column, replay);
            if !is_last && !self.row_wraps(index) {
                replay.line_break();
            }
        }
    }

    /// Appends rows as whole lines.
    pub fn replay_lines(&self, rows: Range<usize>, replay: &mut Replay) {
        self.replay_text(Point::new(rows.start, 0), Point::new(rows.end, 0), replay);
    }

    /// Appends the first `row_count` rows so that the replaying terminal's cursor ends where
    /// this grid's is: the text before the cursor, then the text after it with the cursor put
    /// back.
    pub fn replay_lines_around_cursor(&self, row_count: usize, replay: &mut Replay) {
        let cursor = self.cursor_point();
        let cursor = if self.grid.cursor().input_needs_wrap {
            Point::new(cursor.row, self.columns())
        } else {
            cursor
        };
        self.replay_text(Point::new(0, 0), cursor, replay);
        replay.select(Attributes::default());
        let end = Point::new(row_count.max(cursor.row + 1), 0);
        let mut rest = Replay::new();
        self.replay_text(cursor, end, &mut rest);
        // A line break after the last row would scroll a full screen for nothing.
        let rest = rest.output.trim_end_matches("\r\n");
        if !rest.is_empty() {
            // Restoring the cursor also restores the attributes saved with it.
            replay.control("\x1b7");
            replay.control(rest);
            replay.control("\x1b8");
        }
    }

    /// Appends the visible rows at their own positions, for a replaying terminal of the same
    /// size whose screen is blank, then the scroll region, the origin mode and the cursor.
    pub fn replay_screen(&self, replay: &mut Replay) {
        let first_visible_row = self.history_size();
        let mut continues_previous_row = false;
        for visible_row in 0..self.visible_rows() {
            let Some(row) = self.row(first_visible_row + visible_row) else {
                break;
            };
            let length = drawn_length(&row);
            if length > 0 {
                if !continues_previous_row {
                    replay.cursor_position(visible_row, 0);
                }
                self.replay_cells(&row, 0..length, replay);
            }
            continues_previous_row =
                length == row.len() && self.row_wraps(first_visible_row + visible_row);
        }

        let region = self.scroll_region();
        if region.start.0 != 0 || region.end.0 != self.visible_rows() {
            let _ = write!(
                replay.output,
                "\x1b[{};{}r",
                region.start.0 + 1,
                region.end.0
            );
        }
        let cursor = self.grid.cursor();
        let cursor_row = if self.is_mode_set(TermMode::ORIGIN) {
            replay.control("\x1b[?6h");
            cursor.point.row.0.saturating_sub(region.start.0)
        } else {
            cursor.point.row.0
        };
        if cursor.input_needs_wrap {
            // A cursor waiting to wrap sits past the last column, where only writing that
            // column again puts it.
            let row = self.row(first_visible_row + cursor.point.row.0);
            let last_cell = row.as_ref().and_then(|row| {
                (0..row.len())
                    .rev()
                    .find(|column| {
                        row.get(*column)
                            .is_some_and(|cell| !cell.flags.intersects(SPACER_FLAGS))
                    })
                    .and_then(|column| Some((column, row.get(column)?)))
            });
            if let Some((column, cell)) = last_cell {
                replay.cursor_position(cursor_row, column);
                replay.cell(cell);
                return;
            }
        }
        replay.cursor_position(cursor_row, cursor.point.col);
    }

    /// The modes that have been set on this grid.
    pub fn term_mode(&self) -> TermMode {
        self.ansi_handler_state.mode
    }

    /// Appends the modes, character sets, cursor shape and attributes that differ from those of
    /// a terminal that has just been reset. A terminal has one set of modes where each grid
    /// keeps its own, so the caller says which are in effect.
    pub fn replay_modes(&self, mode: TermMode, replay: &mut Replay) {
        for (flag, sequence) in [
            (TermMode::APP_CURSOR, "\x1b[?1h"),
            (TermMode::APP_KEYPAD, "\x1b="),
            (TermMode::INSERT, "\x1b[4h"),
            (TermMode::LINE_FEED_NEW_LINE, "\x1b[20h"),
            (TermMode::MOUSE_REPORT_CLICK, "\x1b[?1000h"),
            (TermMode::MOUSE_DRAG, "\x1b[?1002h"),
            (TermMode::MOUSE_MOTION, "\x1b[?1003h"),
            (TermMode::UTF8_MOUSE, "\x1b[?1005h"),
            (TermMode::SGR_MOUSE, "\x1b[?1006h"),
            (TermMode::FOCUS_IN_OUT, "\x1b[?1004h"),
            (TermMode::BRACKETED_PASTE, "\x1b[?2004h"),
        ] {
            if mode.contains(flag) {
                replay.control(sequence);
            }
        }
        if !mode.contains(TermMode::LINE_WRAP) {
            replay.control("\x1b[?7l");
        }
        if !mode.contains(TermMode::SHOW_CURSOR) {
            replay.control("\x1b[?25l");
        }
        let keyboard_mode = self.ansi_handler_state.keyboard_mode.bits();
        if keyboard_mode != 0 {
            let _ = write!(replay.output, "\x1b[={keyboard_mode};1u");
        }

        let cursor = self.grid.cursor();
        for (index, designate) in [
            (CharsetIndex::G0, "\x1b(0"),
            (CharsetIndex::G1, "\x1b)0"),
            (CharsetIndex::G2, "\x1b*0"),
            (CharsetIndex::G3, "\x1b+0"),
        ] {
            match cursor.charsets[index] {
                StandardCharset::SpecialCharacterAndusizeDrawing => replay.control(designate),
                StandardCharset::Ascii => {}
            }
        }
        match self.ansi_handler_state.active_charset {
            CharsetIndex::G0 => {}
            CharsetIndex::G1 => replay.control("\x0e"),
            CharsetIndex::G2 => replay.control("\x1bn"),
            CharsetIndex::G3 => replay.control("\x1bo"),
        }

        let style = self.cursor_style();
        let shape = match (style.shape, style.blinking) {
            // A steady block is what a reset terminal shows, and the other two are not shapes a
            // program can ask for.
            (CursorShape::Block, false) | (CursorShape::HollowBlock | CursorShape::Hidden, _) => {
                None
            }
            (CursorShape::Block, true) => Some(1),
            (CursorShape::Underline, true) => Some(3),
            (CursorShape::Underline, false) => Some(4),
            (CursorShape::Beam, true) => Some(5),
            (CursorShape::Beam, false) => Some(6),
        };
        if let Some(shape) = shape {
            let _ = write!(replay.output, "\x1b[{shape} q");
        }

        replay.select(Attributes::of(&cursor.template));
    }

    fn replay_cells(&self, row: &Row, columns: Range<usize>, replay: &mut Replay) {
        let end = columns.end.min(drawn_length(row));
        let mut in_tab = false;
        for column in columns.start..end {
            let Some(cell) = row.get(column) else {
                break;
            };
            // A tab is one character in the cell where it started; the cells it skipped hold
            // nothing of their own.
            if in_tab {
                if !self.ansi_handler_state.tabs[column] {
                    continue;
                }
                in_tab = false;
            }
            if cell.flags.intersects(SPACER_FLAGS) {
                continue;
            }
            in_tab = cell.c == '\t';
            replay.cell(cell);
        }
    }
}

#[cfg(test)]
#[path = "replay_tests.rs"]
mod tests;
