//! Keeps a shell's prompt on a line of its own in the output that is sent on.
//!
//! The app's shell integration has the shell print its prompt marked as taking no room, since
//! the app draws the prompt itself. The shell's line editor then draws the command line from the
//! first column, over the prompt. A line break after each prompt keeps both visible on an
//! ordinary terminal. The line editor does not know about that line break: when it draws the
//! prompt again, as it does after a change of size, it starts on the command line, one row too
//! low. The cursor is then brought back to the prompt's first row before the prompt is drawn.

/// What the app's shell integration prints around a prompt: this, a letter, and a bell.
const MARK_START: &[u8] = b"\x1b]133;";
const MARK_END: u8 = 0x07;
const PROMPT_START: u8 = b'A';
const PROMPT_END: u8 = b'B';

#[derive(Clone, Copy)]
enum Mark {
    PromptStart,
    PromptEnd,
}

/// Follows a shell's prompts through output that arrives in pieces.
#[derive(Default)]
pub struct PromptLines {
    /// How much of a mark the output read so far ends with.
    matched: usize,
    /// The mark that is being read, once its letter has been.
    mark: Option<Mark>,
    /// Line feeds in the prompt that is being printed; `None` between prompts.
    prompt_line_feeds: Option<usize>,
    /// The rows of the prompt right above the command line, for as long as the output stays on
    /// that line.
    prompt_above: Option<usize>,
}

impl PromptLines {
    /// Tells where the output read so far has left the cursor: on the command line right below
    /// a prompt of `rows` rows, or with `None` anywhere else.
    pub fn set_prompt_above(&mut self, rows: Option<usize>) {
        self.prompt_above = rows;
    }

    /// Hands `bytes` to `send` in pieces, with what keeps the prompts on lines of their own in
    /// between.
    pub fn rewrite(&mut self, bytes: &[u8], mut send: impl FnMut(&[u8])) {
        let mut sent = 0;
        for (index, byte) in bytes.iter().enumerate() {
            match self.read(*byte) {
                Some(Mark::PromptStart) => {
                    send(&bytes[sent..=index]);
                    sent = index + 1;
                    // No line feed since the last prompt: the line editor is drawing that
                    // prompt again and takes the cursor to be on its first row.
                    if let Some(rows) = self.prompt_above.take() {
                        send(format!("\x1b[{rows}A\x1b[J").as_bytes());
                    }
                    self.prompt_line_feeds = Some(0);
                }
                Some(Mark::PromptEnd) => {
                    send(&bytes[sent..=index]);
                    sent = index + 1;
                    send(b"\r\n");
                    self.prompt_above = Some(self.prompt_line_feeds.take().unwrap_or(0) + 1);
                }
                None if *byte == b'\n' => match &mut self.prompt_line_feeds {
                    Some(line_feeds) => *line_feeds += 1,
                    None => self.prompt_above = None,
                },
                None => {}
            }
        }
        send(&bytes[sent..]);
    }

    /// Takes in one byte and returns the mark it completes.
    fn read(&mut self, byte: u8) -> Option<Mark> {
        let is_expected = if let Some(expected) = MARK_START.get(self.matched) {
            byte == *expected
        } else if self.matched == MARK_START.len() {
            self.mark = match byte {
                PROMPT_START => Some(Mark::PromptStart),
                PROMPT_END => Some(Mark::PromptEnd),
                _ => None,
            };
            self.mark.is_some()
        } else {
            byte == MARK_END
        };
        if !is_expected {
            // The mark's first byte occurs nowhere else in it, so a mismatch can only be the
            // start of another attempt.
            self.matched = usize::from(byte == MARK_START[0]);
            return None;
        }
        self.matched += 1;
        if byte != MARK_END {
            return None;
        }
        self.matched = 0;
        self.mark.take()
    }
}

#[cfg(test)]
#[path = "prompt_lines_tests.rs"]
mod tests;
