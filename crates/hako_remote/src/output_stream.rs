//! One terminal session's output on its way to the relay. Output is collected between sends, so
//! that a burst leaves as a few large frames, and every byte is counted, so that a receiver can
//! tell from a frame's offset when some never reached it.

/// The largest payload sent in one output frame.
pub const MAX_FRAME_PAYLOAD: usize = 64 * 1024;

/// How much unsent output a session holds before it gives the output up.
pub const MAX_HELD_OUTPUT: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputChunk {
    /// Offset of the first payload byte in the session's output.
    pub seq: u64,
    pub payload: Vec<u8>,
}

pub struct OutputStream {
    /// Offset of the first held byte.
    seq: u64,
    held: Vec<u8>,
    limit: usize,
    /// Output has been given up since the last chunk was taken, which the receiver has to be
    /// shown even if nothing follows.
    gap_to_announce: bool,
}

impl OutputStream {
    /// A stream whose next byte has the offset `seq` and which holds at most `limit` bytes.
    pub fn new(seq: u64, limit: usize) -> Self {
        Self {
            seq,
            held: Vec::new(),
            limit,
            gap_to_announce: false,
        }
    }

    /// Offset of the byte that will follow everything taken in so far.
    pub fn end(&self) -> u64 {
        self.seq + self.held.len() as u64
    }

    pub fn has_output(&self) -> bool {
        !self.held.is_empty() || self.gap_to_announce
    }

    /// Takes in output. Output that would not fit is given up together with what is held, which
    /// leaves a gap in the offsets.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.held.len() + bytes.len() > self.limit {
            self.give_up(bytes.len() as u64);
        } else {
            self.held.extend_from_slice(bytes);
        }
    }

    /// Records that output was lost before it got here, without knowing how much.
    pub fn mark_lost(&mut self) {
        // One byte is enough to move the next offset away from the one the receiver expects.
        self.give_up(1);
    }

    fn give_up(&mut self, uncounted: u64) {
        self.seq = self.end() + uncounted;
        self.held.clear();
        self.gap_to_announce = true;
    }

    /// Removes what is held, split into chunks of at most `max_payload` bytes. After a gap the
    /// first chunk may be empty: its offset alone tells the receiver that output is missing.
    pub fn take_chunks(&mut self, max_payload: usize) -> Vec<OutputChunk> {
        let mut chunks = Vec::new();
        if self.held.is_empty() && self.gap_to_announce {
            chunks.push(OutputChunk {
                seq: self.seq,
                payload: Vec::new(),
            });
        }
        let mut seq = self.seq;
        for payload in self.held.chunks(max_payload.max(1)) {
            chunks.push(OutputChunk {
                seq,
                payload: payload.to_vec(),
            });
            seq += payload.len() as u64;
        }
        self.seq = seq;
        self.held.clear();
        self.gap_to_announce = false;
        chunks
    }
}

#[cfg(test)]
#[path = "output_stream_tests.rs"]
mod tests;
