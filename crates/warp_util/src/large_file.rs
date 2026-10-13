//! Decides whether a file is opened in an editor, and reads the start of one that is not.
//!
//! An editor lays out every line of its buffer up front and keeps the result: measured on a
//! debug build, about 3.7 KB per line plus 57 bytes per byte of text, and about 40 µs of blocked
//! UI thread per line. A 60 MB log of half a million lines costs 5.8 GB and 20 seconds; a 640 MB
//! dump runs the machine out of memory. The limits below are what keeps that from happening
//! without being asked for.

use std::io::{self, BufRead, BufReader, Read};

const MIB: u64 = 1024 * 1024;

/// Largest file that opens in an editor with every feature on. Above it the editor is in
/// reduced mode: no syntax highlighting, blame, formatting, language server or gutter diff.
pub const FULL_EDITOR_MAX_BYTES: u64 = 2 * MIB;

/// Most lines a file can have and still open with every feature on.
pub const FULL_EDITOR_MAX_LINES: usize = 50_000;

/// Largest file that opens in an editor without being asked for: by a click, a link or a
/// restored session. Laying it out stays around a gigabyte and a few seconds.
pub const AUTO_OPEN_MAX_BYTES: u64 = 10 * MIB;

/// Most lines a file can have and still open in an editor without being asked for.
pub const AUTO_OPEN_MAX_LINES: usize = 150_000;

/// Longest line, in bytes, of a file that opens in an editor without being asked for.
pub const AUTO_OPEN_MAX_LINE_BYTES: usize = 1024 * 1024;

/// Largest file "Open in editor anyway" is offered for. Past it the editor would need more
/// memory than a machine can be assumed to have.
pub const OPEN_ANYWAY_MAX_BYTES: u64 = 64 * MIB;

/// Most lines a file can have for "Open in editor anyway" to be offered.
pub const OPEN_ANYWAY_MAX_LINES: usize = 600_000;

/// Lines shown of a file that is not opened in an editor.
pub const PEEK_MAX_LINES: usize = 300;

/// Characters shown of each of those lines.
pub const PEEK_MAX_LINE_CHARS: usize = 2_000;

/// How far into the file the peek reads at most, however long its lines are.
pub const PEEK_MAX_SCANNED_BYTES: u64 = 4 * MIB;

/// Memory an editor needs per line of its buffer, as measured.
const EDITOR_BYTES_PER_LINE: u64 = 3_700;

/// Memory an editor needs per byte of its buffer, as measured.
const EDITOR_BYTES_PER_BYTE: u64 = 57;

/// How much of an editor's feature set a buffer gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    Full,
    /// Editable, but without the features that work on the whole buffer.
    Reduced,
}

/// The limit a file is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    Bytes,
    Lines,
    LineLength,
}

/// Who wants the file in an editor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OpenIntent {
    /// Something that did not ask for an editor in so many words.
    #[default]
    Automatic,
    /// The user, after having been told the file is large.
    Anyway,
}

impl OpenIntent {
    /// Largest file an editor takes for this intent.
    pub fn max_bytes(self) -> u64 {
        match self {
            Self::Automatic => AUTO_OPEN_MAX_BYTES,
            Self::Anyway => OPEN_ANYWAY_MAX_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenDecision {
    Editor(EditorMode),
    /// Not opened in an editor unless asked for, which `can_open_anyway` says is possible.
    LargeFile {
        limit: Limit,
        can_open_anyway: bool,
    },
}

/// What a file holds, as far as opening it is concerned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileShape {
    pub bytes: u64,
    /// Lines of text, counting a last line that does not end in a newline.
    pub lines: usize,
    pub longest_line_bytes: usize,
    /// A NUL byte was found, which text does not contain.
    pub binary: bool,
}

impl FileShape {
    pub fn of_bytes(content: &[u8]) -> Self {
        let mut scan = ShapeScan::default();
        scan.feed(content);
        scan.finish()
    }

    /// Reads `reader` to its end without keeping what it read.
    pub fn scan(reader: impl Read) -> io::Result<Self> {
        let mut reader = BufReader::with_capacity(256 * 1024, reader);
        let mut scan = ShapeScan::default();
        loop {
            let chunk = reader.fill_buf()?;
            if chunk.is_empty() {
                return Ok(scan.finish());
            }
            scan.feed(chunk);
            let consumed = chunk.len();
            reader.consume(consumed);
        }
    }

    /// The first limit this file is over for `intent`, if any.
    pub fn exceeded_limit(&self, intent: OpenIntent) -> Option<Limit> {
        let (max_lines, max_line_bytes) = match intent {
            OpenIntent::Automatic => (AUTO_OPEN_MAX_LINES, AUTO_OPEN_MAX_LINE_BYTES),
            // A long line costs no more than a short one once the editor stops drawing it.
            OpenIntent::Anyway => (OPEN_ANYWAY_MAX_LINES, usize::MAX),
        };
        if self.bytes > intent.max_bytes() {
            Some(Limit::Bytes)
        } else if self.lines > max_lines {
            Some(Limit::Lines)
        } else if self.longest_line_bytes > max_line_bytes {
            Some(Limit::LineLength)
        } else {
            None
        }
    }

    pub fn editor_mode(&self) -> EditorMode {
        if self.bytes > FULL_EDITOR_MAX_BYTES || self.lines > FULL_EDITOR_MAX_LINES {
            EditorMode::Reduced
        } else {
            EditorMode::Full
        }
    }

    /// What opening this file without being asked for an editor does.
    pub fn decision(&self) -> OpenDecision {
        match self.exceeded_limit(OpenIntent::Automatic) {
            Some(limit) => OpenDecision::LargeFile {
                limit,
                can_open_anyway: self.exceeded_limit(OpenIntent::Anyway).is_none(),
            },
            None => OpenDecision::Editor(self.editor_mode()),
        }
    }

    /// Roughly how much memory an editor showing this file takes.
    pub fn estimated_editor_bytes(&self) -> u64 {
        (self.lines as u64)
            .saturating_mul(EDITOR_BYTES_PER_LINE)
            .saturating_add(self.bytes.saturating_mul(EDITOR_BYTES_PER_BYTE))
    }
}

/// What can be decided from the size of a file alone, before any of it is read. `None` means
/// its lines have to be looked at.
pub fn decision_for_size(bytes: u64) -> Option<OpenDecision> {
    (bytes > AUTO_OPEN_MAX_BYTES).then_some(OpenDecision::LargeFile {
        limit: Limit::Bytes,
        can_open_anyway: bytes <= OPEN_ANYWAY_MAX_BYTES,
    })
}

#[derive(Default)]
struct ShapeScan {
    shape: FileShape,
    current_line_bytes: usize,
}

impl ShapeScan {
    fn feed(&mut self, chunk: &[u8]) {
        self.shape.bytes += chunk.len() as u64;
        self.shape.binary |= chunk.contains(&0);
        let mut rest = chunk;
        while let Some(newline) = rest.iter().position(|byte| *byte == b'\n') {
            self.end_line(newline);
            rest = &rest[newline + 1..];
        }
        self.current_line_bytes += rest.len();
    }

    fn end_line(&mut self, bytes_before_newline: usize) {
        let line_bytes = self.current_line_bytes + bytes_before_newline;
        self.shape.longest_line_bytes = self.shape.longest_line_bytes.max(line_bytes);
        self.shape.lines += 1;
        self.current_line_bytes = 0;
    }

    fn finish(mut self) -> FileShape {
        if self.current_line_bytes > 0 {
            self.end_line(0);
        }
        self.shape
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeekLine {
    pub text: String,
    /// The line goes on past `text`.
    pub truncated: bool,
}

/// The start of a file, cut down to what is cheap to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Peek {
    pub lines: Vec<PeekLine>,
    /// The file goes on past the last line.
    pub has_more: bool,
}

/// Reads the first [`PEEK_MAX_LINES`] lines of `reader`, each cut to [`PEEK_MAX_LINE_CHARS`].
/// Never holds more than one cut line at a time and stops after [`PEEK_MAX_SCANNED_BYTES`], so
/// that neither a huge file nor a huge line costs more than a small one. Bytes that are not
/// UTF-8 come out as replacement characters, and a carriage return before a newline is dropped.
pub fn read_peek(reader: impl Read) -> io::Result<Peek> {
    read_peek_within(
        reader,
        PEEK_MAX_LINES,
        PEEK_MAX_LINE_CHARS,
        PEEK_MAX_SCANNED_BYTES,
    )
}

fn read_peek_within(
    reader: impl Read,
    max_lines: usize,
    max_line_chars: usize,
    max_scanned_bytes: u64,
) -> io::Result<Peek> {
    // A character takes at most four bytes, so this many bytes always hold a whole cut line.
    let max_kept_bytes = max_line_chars.saturating_mul(4);
    let mut reader = BufReader::with_capacity(64 * 1024, reader.take(max_scanned_bytes));
    let mut peek = Peek::default();
    let mut kept = Vec::new();
    let mut line_bytes = 0usize;
    let mut scanned = 0u64;

    'read: loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            break;
        }
        let consumed = chunk.len();
        scanned += consumed as u64;
        let mut rest = chunk;
        loop {
            let newline = rest.iter().position(|byte| *byte == b'\n');
            let segment = &rest[..newline.unwrap_or(rest.len())];
            let room = max_kept_bytes.saturating_sub(kept.len());
            kept.extend_from_slice(&segment[..segment.len().min(room)]);
            line_bytes += segment.len();
            let Some(newline) = newline else {
                break;
            };
            peek.lines
                .push(peek_line(&kept, line_bytes, max_line_chars));
            kept.clear();
            line_bytes = 0;
            rest = &rest[newline + 1..];
            if peek.lines.len() == max_lines {
                peek.has_more = !rest.is_empty() || scanned == max_scanned_bytes;
                reader.consume(consumed);
                if !peek.has_more {
                    peek.has_more = !reader.fill_buf()?.is_empty();
                }
                break 'read;
            }
        }
        reader.consume(consumed);
    }

    if peek.lines.len() < max_lines {
        // The scan ran out before the end of the file, or the file ends without a newline.
        let stopped_early = scanned == max_scanned_bytes;
        if line_bytes > 0 {
            let mut line = peek_line(&kept, line_bytes, max_line_chars);
            line.truncated |= stopped_early;
            peek.lines.push(line);
        }
        peek.has_more = stopped_early;
    }
    Ok(peek)
}

/// Builds the line shown for `kept`, the first bytes of a line that is `line_bytes` long.
fn peek_line(kept: &[u8], line_bytes: usize, max_chars: usize) -> PeekLine {
    let cut_by_bytes = kept.len() < line_bytes;
    // A character split by the byte cut decodes as a replacement, which always lies past
    // `max_chars` and goes with the rest.
    let mut text = String::from_utf8_lossy(kept).into_owned();
    if !cut_by_bytes && text.ends_with('\r') {
        text.pop();
    }
    let cut_by_chars = match text.char_indices().nth(max_chars) {
        Some((end, _)) => {
            text.truncate(end);
            true
        }
        None => false,
    };
    PeekLine {
        text,
        truncated: cut_by_bytes || cut_by_chars,
    }
}

#[cfg(test)]
#[path = "large_file_tests.rs"]
mod tests;
