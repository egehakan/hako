//! The binary frame that carries terminal output:
//!
//! ```text
//! byte 0        0x01
//! byte 1        n = length of the session id in bytes (1 to 255)
//! bytes 2..2+n  session id, UTF-8
//! next 8 bytes  seq: unsigned 64-bit big-endian offset of the first payload byte
//! rest          raw PTY bytes
//! ```

use thiserror::Error;

pub const OUTPUT_FRAME_TYPE: u8 = 0x01;

const SEQ_BYTES: usize = 8;

#[derive(Debug, Error, PartialEq, Eq)]
#[error("a session id must be 1 to 255 bytes, not {0}")]
pub struct SessionIdLengthError(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputFrame<'a> {
    pub session_id: &'a str,
    /// Offset of the first payload byte in the session's output stream.
    pub seq: u64,
    pub payload: &'a [u8],
}

impl<'a> OutputFrame<'a> {
    pub fn encode(&self) -> Result<Vec<u8>, SessionIdLengthError> {
        let id = self.session_id.as_bytes();
        let id_length = u8::try_from(id.len())
            .ok()
            .filter(|length| *length > 0)
            .ok_or(SessionIdLengthError(id.len()))?;
        let mut frame = Vec::with_capacity(2 + id.len() + SEQ_BYTES + self.payload.len());
        frame.push(OUTPUT_FRAME_TYPE);
        frame.push(id_length);
        frame.extend_from_slice(id);
        frame.extend_from_slice(&self.seq.to_be_bytes());
        frame.extend_from_slice(self.payload);
        Ok(frame)
    }

    /// Reads a frame, or returns `None` when `frame` is not a well-formed output frame.
    pub fn decode(frame: &'a [u8]) -> Option<Self> {
        let (&[frame_type, id_length], rest) = frame.split_first_chunk::<2>()?;
        if frame_type != OUTPUT_FRAME_TYPE || id_length == 0 {
            return None;
        }
        let (id, rest) = rest.split_at_checked(usize::from(id_length))?;
        let (seq, payload) = rest.split_first_chunk::<SEQ_BYTES>()?;
        Some(Self {
            session_id: std::str::from_utf8(id).ok()?,
            seq: u64::from_be_bytes(*seq),
            payload,
        })
    }
}

#[cfg(test)]
#[path = "output_frame_tests.rs"]
mod tests;
