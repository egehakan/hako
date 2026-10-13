//! Finds where a shell's prompt ends in its output.

/// What the app's shell integration prints right after a prompt.
const PROMPT_END: &[u8] = b"\x1b]133;B\x07";

/// Looks for the end of a prompt in output that arrives in pieces.
#[derive(Default)]
pub struct PromptEndFinder {
    /// How much of the mark the output read so far ends with.
    matched: usize,
}

impl PromptEndFinder {
    /// The offsets in `bytes` right after each end of a prompt.
    pub fn find(&mut self, bytes: &[u8]) -> Vec<usize> {
        let mut ends = Vec::new();
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == PROMPT_END[self.matched] {
                self.matched += 1;
                if self.matched == PROMPT_END.len() {
                    ends.push(index + 1);
                    self.matched = 0;
                }
            } else {
                // The mark's first byte occurs nowhere else in it, so a mismatch can only be the
                // start of another attempt.
                self.matched = usize::from(*byte == PROMPT_END[0]);
            }
        }
        ends
    }
}

#[cfg(test)]
#[path = "prompt_end_tests.rs"]
mod tests;
