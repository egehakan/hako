//! Follows a terminal's output just far enough to know whether it stops between two pieces or in
//! the middle of one. A terminal emulator that starts reading at such a point needs the start of
//! the unfinished escape sequence or character first.

const ESC: u8 = 0x1b;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const BEL: u8 = 0x07;

/// A control string, such as one of the app's own shell hooks, can be long; one longer than this
/// is not kept.
const MAX_UNFINISHED: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// The output has not passed a point yet where its state is certain.
    Unknown,
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    /// An OSC string, which BEL or ST ends.
    OscString,
    /// A DCS, SOS, PM or APC string, which only ST ends.
    ControlString,
    /// ESC inside a string, which ends the string if `\` follows.
    StringEscape,
    Utf8 {
        remaining: u8,
    },
}

pub struct Boundary {
    state: State,
    unfinished: Vec<u8>,
    /// The unfinished piece grew past what is kept, so its start is no longer known.
    truncated: bool,
}

impl Default for Boundary {
    fn default() -> Self {
        Self::new()
    }
}

impl Boundary {
    /// For output that is joined at an arbitrary point.
    pub fn new() -> Self {
        Self {
            state: State::Unknown,
            unfinished: Vec::new(),
            truncated: false,
        }
    }

    /// Whether the output has been followed from a point where its state was certain.
    pub fn is_known(&self) -> bool {
        self.state != State::Unknown
    }

    /// Takes the output to be between two pieces, which is where output that has paused is.
    pub fn assume_ground(&mut self) {
        self.enter_ground();
    }

    /// The bytes of the piece the output stopped inside; empty between pieces, and also when
    /// the start of that piece is not known.
    pub fn unfinished(&self) -> &[u8] {
        if self.truncated {
            &[]
        } else {
            &self.unfinished
        }
    }

    pub fn feed(&mut self, mut bytes: &[u8]) {
        while let Some((&byte, rest)) = bytes.split_first() {
            if self.state == State::Ground {
                // Most output is plain text, which changes nothing.
                let plain = bytes
                    .iter()
                    .position(|byte| *byte == ESC || *byte >= 0x80)
                    .unwrap_or(bytes.len());
                if plain > 0 {
                    bytes = &bytes[plain..];
                    continue;
                }
            }
            self.advance(byte);
            bytes = rest;
        }
    }

    fn advance(&mut self, byte: u8) {
        match byte {
            CAN | SUB => return self.enter_ground(),
            ESC => {
                return match self.state {
                    State::OscString | State::ControlString => {
                        self.keep(byte);
                        self.state = State::StringEscape;
                    }
                    State::Unknown
                    | State::Ground
                    | State::Escape
                    | State::EscapeIntermediate
                    | State::Csi
                    | State::StringEscape
                    | State::Utf8 { .. } => self.enter_escape(),
                };
            }
            _ => {}
        }

        match self.state {
            State::Unknown => {}
            State::Ground => match byte {
                0xc2..=0xdf => self.enter_utf8(byte, 1),
                0xe0..=0xef => self.enter_utf8(byte, 2),
                0xf0..=0xf4 => self.enter_utf8(byte, 3),
                _ => {}
            },
            State::Utf8 { remaining } => {
                if !(0x80..=0xbf).contains(&byte) {
                    // Not a continuation: the character is broken off and this byte starts anew.
                    self.enter_ground();
                    self.advance(byte);
                } else if remaining == 1 {
                    self.enter_ground();
                } else {
                    self.keep(byte);
                    self.state = State::Utf8 {
                        remaining: remaining - 1,
                    };
                }
            }
            State::Escape => match byte {
                b'[' => self.continue_as(byte, State::Csi),
                b']' => self.continue_as(byte, State::OscString),
                b'P' | b'X' | b'^' | b'_' => self.continue_as(byte, State::ControlString),
                0x20..=0x2f => self.continue_as(byte, State::EscapeIntermediate),
                0x30..=0x7e => self.enter_ground(),
                // A control character inside a sequence has taken effect already, so it is not
                // part of what has to be repeated.
                _ => {}
            },
            State::EscapeIntermediate => match byte {
                0x20..=0x2f => self.keep(byte),
                0x30..=0x7e => self.enter_ground(),
                _ => {}
            },
            State::Csi => match byte {
                0x20..=0x3f => self.keep(byte),
                0x40..=0x7e => self.enter_ground(),
                _ => {}
            },
            State::OscString => match byte {
                BEL => self.enter_ground(),
                _ => self.keep(byte),
            },
            State::ControlString => self.keep(byte),
            State::StringEscape => match byte {
                b'\\' => self.enter_ground(),
                // Anything else abandons the string, and the ESC starts a sequence of its own.
                _ => {
                    self.enter_escape();
                    self.advance(byte);
                }
            },
        }
    }

    fn enter_ground(&mut self) {
        self.state = State::Ground;
        self.unfinished.clear();
        self.truncated = false;
    }

    fn enter_escape(&mut self) {
        self.enter_ground();
        self.keep(ESC);
        self.state = State::Escape;
    }

    fn enter_utf8(&mut self, byte: u8, remaining: u8) {
        self.keep(byte);
        self.state = State::Utf8 { remaining };
    }

    fn continue_as(&mut self, byte: u8, state: State) {
        self.keep(byte);
        self.state = state;
    }

    fn keep(&mut self, byte: u8) {
        if self.unfinished.len() < MAX_UNFINISHED {
            self.unfinished.push(byte);
        } else {
            self.truncated = true;
        }
    }
}

#[cfg(test)]
#[path = "boundary_tests.rs"]
mod tests;
