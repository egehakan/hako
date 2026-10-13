//! What is shown of a text file that is too large for an editor.

use std::fs::File;
use std::io;
use std::path::Path;

use warp_util::large_file::{
    FileShape, Limit, OPEN_ANYWAY_MAX_BYTES, OpenDecision, Peek, read_peek,
};

use crate::format_bytes;

/// The start of a large file and what is known about the rest of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LargeText {
    pub bytes: u64,
    /// `None` for a file so large that no editor takes it, which is then not read through.
    pub shape: Option<FileShape>,
    pub peek: Peek,
}

impl LargeText {
    /// Reads the start of the file at `path`, and all of it once if an editor could still take
    /// it, to count its lines. Nothing but the start is kept.
    pub fn read(path: &Path) -> io::Result<Self> {
        let bytes = path.metadata()?.len();
        let peek = read_peek(File::open(path)?)?;
        let shape = if bytes <= OPEN_ANYWAY_MAX_BYTES {
            Some(FileShape::scan(File::open(path)?)?)
        } else {
            None
        };
        Ok(Self { bytes, shape, peek })
    }

    /// Whether the file may be handed to an editor on request.
    pub fn can_open_in_editor(&self) -> bool {
        match self.shape.map(|shape| shape.decision()) {
            Some(OpenDecision::Editor(_)) => true,
            Some(OpenDecision::LargeFile {
                can_open_anyway, ..
            }) => can_open_anyway,
            None => false,
        }
    }

    /// Says why the file is not in an editor and what opening it there would take.
    pub fn notice(&self) -> String {
        let size = format_bytes(self.bytes);
        let why = match self.shape.map(|shape| (shape, shape.decision())) {
            None => format!("This file is {size}, too large to open in the editor."),
            Some((shape, OpenDecision::LargeFile { limit, .. })) => {
                let lines = format_count(shape.lines);
                match limit {
                    Limit::Bytes => format!("This file is {size} ({lines} lines)."),
                    Limit::Lines => format!("This file has {lines} lines ({size})."),
                    Limit::LineLength => format!(
                        "This file ({size}) has a line of {}.",
                        format_bytes(shape.longest_line_bytes as u64)
                    ),
                }
            }
            Some((_, OpenDecision::Editor(_))) => {
                format!("This file is {size} and can be opened in the editor.")
            }
        };
        let cost = match self.shape {
            Some(shape) if self.can_open_in_editor() => format!(
                " Opening it in the editor takes about {} of memory, and the app does not \
                 respond while it loads.",
                format_bytes(shape.estimated_editor_bytes())
            ),
            Some(_) => " That is too large to open in the editor.".to_string(),
            None => String::new(),
        };
        let shown = if self.peek.has_more {
            format!(" Its first {} lines are shown here.", self.peek.lines.len())
        } else {
            " It is shown here whole.".to_string()
        };
        format!("{why}{cost}{shown} Nothing here can be edited.")
    }
}

/// Formats a count with separators between the thousands, e.g. `630,788`.
pub fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

#[cfg(test)]
#[path = "large_text_tests.rs"]
mod tests;
