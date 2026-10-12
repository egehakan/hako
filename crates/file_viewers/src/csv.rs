//! Delimited text as a table.
//!
//! A table keeps the text it was read from and the position of each record in it. Fields are
//! parsed when a row is shown, so a large file costs its text plus a few bytes per row.

use std::borrow::Cow;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

/// Files larger than this open as text.
pub const MAX_TABLE_BYTES: u64 = 50 * 1024 * 1024;

/// Tables with more rows than this open as text.
pub const MAX_TABLE_ROWS: usize = 2_000_000;

/// Records looked at to tell the delimiter.
const DELIMITER_SAMPLE_RECORDS: usize = 50;

/// Rows measured to size the columns.
const WIDTH_SAMPLE_ROWS: usize = 1000;

/// Characters of a cell that count towards the width of its column.
const MAX_MEASURED_CHARS: usize = 60;

const DELIMITERS: [u8; 4] = [b',', b';', b'\t', b'|'];

const BYTE_ORDER_MARK: char = '\u{feff}';

pub struct Table {
    text: Arc<str>,
    delimiter: u8,
    /// Where each record sits in `text`, without its line ending. The first one is the header.
    records: Vec<Range<usize>>,
    column_count: usize,
}

impl Table {
    /// Indexes `text`, splitting fields on `delimiter`, or on the delimiter the text appears
    /// to use when there is none.
    pub fn parse(text: Arc<str>, delimiter: Option<u8>) -> Self {
        let delimiter = delimiter.unwrap_or_else(|| detect_delimiter(&text));
        let records = scan_records(&text, usize::MAX);
        let column_count = records
            .iter()
            .map(|record| count_fields(&text[record.clone()], delimiter))
            .max()
            .unwrap_or(0);
        Self {
            text,
            delimiter,
            records,
            column_count,
        }
    }

    pub fn delimiter(&self) -> u8 {
        self.delimiter
    }

    pub fn column_count(&self) -> usize {
        self.column_count
    }

    /// Number of rows below the header.
    pub fn row_count(&self) -> usize {
        self.records.len().saturating_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn header(&self) -> Vec<Cow<'_, str>> {
        self.record(0)
    }

    /// The fields of the row at `index`, counted from the first row below the header.
    pub fn row(&self, index: usize) -> Vec<Cow<'_, str>> {
        self.record(index + 1)
    }

    fn record(&self, index: usize) -> Vec<Cow<'_, str>> {
        self.records
            .get(index)
            .map(|record| split_record(&self.text[record.clone()], self.delimiter))
            .unwrap_or_default()
    }

    /// The longest cell of each column, in characters, among the header and the first rows.
    /// Long cells are counted as [`MAX_MEASURED_CHARS`].
    pub fn column_chars(&self) -> Vec<usize> {
        let mut chars = vec![0; self.column_count];
        for index in 0..self.records.len().min(WIDTH_SAMPLE_ROWS + 1) {
            for (column, field) in self.record(index).iter().enumerate() {
                let length = field.chars().take(MAX_MEASURED_CHARS).count();
                if let Some(longest) = chars.get_mut(column) {
                    *longest = (*longest).max(length);
                }
            }
        }
        chars
    }
}

/// The delimiter implied by the name of a file, when the name settles it.
pub fn delimiter_for_path(path: &Path) -> Option<u8> {
    let extension = path.extension()?.to_str()?;
    extension.eq_ignore_ascii_case("tsv").then_some(b'\t')
}

/// Name of a delimiter for display.
pub fn delimiter_name(delimiter: u8) -> &'static str {
    match delimiter {
        b',' => "comma",
        b';' => "semicolon",
        b'\t' => "tab",
        b'|' => "pipe",
        _ => "custom",
    }
}

/// Picks the delimiter that splits the first records of `text` into the most consistent
/// number of fields. Text with no delimiter at all is a single comma-separated column.
pub fn detect_delimiter(text: &str) -> u8 {
    let records = scan_records(text, DELIMITER_SAMPLE_RECORDS);
    let mut best = (b',', 0, 0);
    for delimiter in DELIMITERS {
        let counts: Vec<usize> = records
            .iter()
            .map(|record| count_fields(&text[record.clone()], delimiter))
            .collect();
        let Some(&typical) = counts
            .iter()
            .max_by_key(|count| counts.iter().filter(|other| other == count).count())
        else {
            continue;
        };
        if typical < 2 {
            continue;
        }
        let agreeing = counts.iter().filter(|count| **count == typical).count();
        if (agreeing, typical) > (best.1, best.2) {
            best = (delimiter, agreeing, typical);
        }
    }
    best.0
}

/// Finds up to `limit` records of `text`. A line break inside a quoted field does not end a
/// record, and empty lines are not records.
fn scan_records(text: &str, limit: usize) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let start = if text.starts_with(BYTE_ORDER_MARK) {
        BYTE_ORDER_MARK.len_utf8()
    } else {
        0
    };

    let mut records = Vec::new();
    let mut record_start = start;
    let mut in_quotes = false;
    let push = |range: Range<usize>, records: &mut Vec<Range<usize>>| {
        let end = if range.end > range.start && bytes[range.end - 1] == b'\r' {
            range.end - 1
        } else {
            range.end
        };
        if end > range.start {
            records.push(range.start..end);
        }
    };
    for (index, byte) in bytes.iter().enumerate().skip(start) {
        match byte {
            // An escaped quote (`""`) leaves and re-enters the quoted state, which comes to the
            // same thing for finding where records end.
            b'"' => in_quotes = !in_quotes,
            b'\n' if !in_quotes => {
                push(record_start..index, &mut records);
                record_start = index + 1;
                if records.len() >= limit {
                    return records;
                }
            }
            _ => {}
        }
    }
    push(record_start..bytes.len(), &mut records);
    records
}

fn count_fields(record: &str, delimiter: u8) -> usize {
    let mut in_quotes = false;
    let mut fields = 1;
    for byte in record.bytes() {
        if byte == b'"' {
            in_quotes = !in_quotes;
        } else if byte == delimiter && !in_quotes {
            fields += 1;
        }
    }
    fields
}

/// Splits one record into its fields, removing the quotes around quoted ones.
fn split_record(record: &str, delimiter: u8) -> Vec<Cow<'_, str>> {
    let bytes = record.as_bytes();
    let mut fields = Vec::new();
    let mut position = 0;
    loop {
        let (field, next) = if bytes.get(position) == Some(&b'"') {
            read_quoted_field(record, position, delimiter)
        } else {
            let end = bytes[position..]
                .iter()
                .position(|byte| *byte == delimiter)
                .map_or(bytes.len(), |offset| position + offset);
            (Cow::Borrowed(&record[position..end]), end)
        };
        fields.push(field);
        if next >= bytes.len() {
            break;
        }
        position = next + 1;
    }
    fields
}

/// Reads the quoted field that opens at `start`. Returns its value and the position of the
/// delimiter after it, or the length of the record when it is the last field.
fn read_quoted_field(record: &str, start: usize, delimiter: u8) -> (Cow<'_, str>, usize) {
    let bytes = record.as_bytes();
    let content_start = start + 1;
    let mut value: Option<String> = None;
    let mut segment_start = content_start;
    let mut position = content_start;
    while position < bytes.len() {
        if bytes[position] != b'"' {
            position += 1;
            continue;
        }
        if bytes.get(position + 1) == Some(&b'"') {
            let value = value.get_or_insert_with(String::new);
            value.push_str(&record[segment_start..=position]);
            position += 2;
            segment_start = position;
            continue;
        }

        let end = bytes[position..]
            .iter()
            .position(|byte| *byte == delimiter)
            .map_or(bytes.len(), |offset| position + offset);
        let quoted = &record[segment_start..position];
        // Text between the closing quote and the delimiter is malformed; it is kept rather
        // than dropped.
        let trailing = &record[position + 1..end];
        let field = match (value, trailing.is_empty()) {
            (None, true) => Cow::Borrowed(quoted),
            (value, _) => {
                let mut value = value.unwrap_or_default();
                value.push_str(quoted);
                value.push_str(trailing);
                Cow::Owned(value)
            }
        };
        return (field, end);
    }

    // The quote never closes: the rest of the record is the field.
    let rest = &record[segment_start..];
    let field = match value {
        Some(mut value) => {
            value.push_str(rest);
            Cow::Owned(value)
        }
        None => Cow::Borrowed(rest),
    };
    (field, bytes.len())
}

/// Width of each column given the length of its longest cell in characters.
pub fn column_widths(
    chars: &[usize],
    char_width: f32,
    padding: f32,
    min_width: f32,
    max_width: f32,
) -> Vec<f32> {
    chars
        .iter()
        .map(|chars| (*chars as f32 * char_width + padding).clamp(min_width, max_width))
        .collect()
}

/// The columns to draw for a table scrolled `scroll_x` pixels to the right in a viewport
/// `viewport_width` wide, and how far left of the viewport the first of them starts.
pub fn visible_columns(widths: &[f32], scroll_x: f32, viewport_width: f32) -> (Range<usize>, f32) {
    let mut left = 0.;
    let mut first = None;
    let mut end = widths.len();
    for (index, width) in widths.iter().enumerate() {
        let right = left + width;
        if first.is_none() && right > scroll_x {
            first = Some((index, scroll_x - left));
        }
        if left >= scroll_x + viewport_width {
            end = index;
            break;
        }
        left = right;
    }
    match first {
        Some((first, offset)) => (first..end.max(first), offset.max(0.)),
        None => (widths.len()..widths.len(), 0.),
    }
}

/// The furthest a table with columns of `widths` scrolls to the right.
pub fn max_scroll_x(widths: &[f32], viewport_width: f32) -> f32 {
    (widths.iter().sum::<f32>() - viewport_width).max(0.)
}

/// How far a table is scrolled. Rows scroll whole, so that the header and the row numbers
/// never show a row cut in half.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GridScroll {
    /// Index of the first row shown.
    pub top_row: usize,
    /// Scroll distance not yet amounting to a whole row, in pixels.
    pending_y: f32,
    /// Horizontal scroll, in pixels.
    pub x: f32,
}

impl GridScroll {
    /// Scrolls by `delta_x` pixels to the right and `delta_y` pixels down, within a table of
    /// `row_count` rows of `row_height` pixels of which `visible_rows` fit in the viewport.
    pub fn scroll_by(
        &mut self,
        delta_x: f32,
        delta_y: f32,
        row_height: f32,
        row_count: usize,
        visible_rows: usize,
        max_x: f32,
    ) {
        self.x = (self.x + delta_x).clamp(0., max_x.max(0.));
        if row_height <= 0. {
            return;
        }

        self.pending_y += delta_y;
        let rows = (self.pending_y / row_height).trunc();
        self.pending_y -= rows * row_height;
        let last_top_row = row_count.saturating_sub(visible_rows.max(1)) as f32;
        let top_row = (self.top_row as f32 + rows).clamp(0., last_top_row);
        // Distance scrolled into an edge does not have to be scrolled back out of it.
        let into_top = top_row == 0. && self.pending_y < 0.;
        let into_bottom = top_row == last_top_row && self.pending_y > 0.;
        if into_top || into_bottom {
            self.pending_y = 0.;
        }
        self.top_row = top_row as usize;
    }
}

/// How many rows of `row_height` pixels are fully visible in `height` pixels under a header
/// row of the same height.
pub fn visible_rows(height: f32, row_height: f32) -> usize {
    if row_height <= 0. {
        return 0;
    }
    ((height / row_height).floor() as usize).saturating_sub(1)
}

#[cfg(test)]
#[path = "csv_tests.rs"]
mod tests;
