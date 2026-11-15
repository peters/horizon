//! The byte scanner that finds the program status reports of one panel's output.
//!
//! A program reports its state to its terminal as one OSC sequence,
//! `ESC ] 7501 ; pairs ST` (Program Status Protocol, revision 0.4; `ST` is
//! `ESC \` or `BEL`). The VT parser of this workspace drops a private OSC number,
//! so the bytes are read on their way to the parser, in the reader of the PTY.
//!
//! The scanner is a small byte machine. It copies no output, it allocates
//! nothing, and it holds no byte of a stream that is not a status sequence; the
//! only buffer is the bound of one payload. A sequence may end in a later read
//! than the one that started it, so the state of the machine is the whole of what
//! a read leaves behind.

use crate::inbox::StatusTap;

/// The byte that starts every escape sequence.
const ESC: u8 = 0x1b;
/// The byte that can end an OSC sequence instead of `ST`.
const BEL: u8 = 0x07;
/// The OSC introducer.
const OSC: u8 = b']';
/// The OSC number of the protocol and the separator that follows it.
const NUMBER: &[u8] = b"7501;";

/// The longest payload that this scanner keeps, in bytes.
///
/// The protocol caps one report at 4096 bytes and lets a terminal choose lower
/// limits, and a report that breaks a limit is discarded whole. A payload beyond
/// this bound is read to its end and dropped, so the stream around it is
/// unaffected.
pub const MAX_PAYLOAD_BYTES: usize = 256;

/// One complete payload of a status sequence: the bytes after `7501;`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Payload {
    bytes: [u8; MAX_PAYLOAD_BYTES],
    len: u16,
}

impl Payload {
    /// The payload as the program wrote it.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn push(&mut self, byte: u8) -> bool {
        let len = usize::from(self.len);
        if len >= MAX_PAYLOAD_BYTES {
            return false;
        }
        self.bytes[len] = byte;
        self.len = self.len.saturating_add(1);
        true
    }
}

impl Default for Payload {
    fn default() -> Self {
        Self {
            bytes: [0; MAX_PAYLOAD_BYTES],
            len: 0,
        }
    }
}

#[cfg(test)]
impl Payload {
    /// A payload of `bytes`, for a test of the hand-over of a report.
    pub(crate) fn for_tests(bytes: &[u8]) -> Self {
        assert!(
            bytes.len() <= MAX_PAYLOAD_BYTES,
            "the payload of the test fits the bound"
        );
        let mut payload = Self::default();
        for &byte in bytes {
            assert!(payload.push(byte), "the payload of the test fits the bound");
        }
        payload
    }
}

/// Where the machine is in the stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Between sequences: every byte is shown and none is looked at.
    #[default]
    Ground,
    /// An escape byte arrived.
    Escape,
    /// The introducer arrived; this many bytes of the number have matched.
    Number { matched: u8 },
    /// The number matched and the payload accumulates.
    Payload,
    /// An escape byte arrived inside a payload.
    PayloadEscape,
    /// Another OSC sequence runs; its bytes are skipped to its end.
    Other,
    /// An escape byte arrived inside another OSC sequence.
    OtherEscape,
}

/// The scanner of one terminal's output.
pub struct Scanner {
    tap: StatusTap,
    state: State,
    payload: Payload,
    /// The payload broke [`MAX_PAYLOAD_BYTES`], so its end drops it.
    overflow: bool,
}

impl Scanner {
    /// A scanner whose complete payloads go to `tap`.
    #[must_use]
    pub fn new(tap: StatusTap) -> Self {
        Self {
            tap,
            state: State::Ground,
            payload: Payload::default(),
            overflow: false,
        }
    }

    /// Looks at the bytes of one read. The caller shows them unchanged.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            match self.state {
                State::Ground => {
                    // The common case of a stream of output: find the next escape
                    // byte in one vectorised pass and look at nothing in between.
                    // The machine has no other introducer: an OSC sequence of this
                    // scanner begins with `ESC ]`, so one byte is the whole search.
                    let Some(offset) = memchr::memchr(ESC, &bytes[index..]) else {
                        return;
                    };
                    index += offset;
                    self.state = State::Escape;
                }
                State::Escape => {
                    self.state = if byte == OSC {
                        State::Number { matched: 0 }
                    } else if byte == ESC {
                        State::Escape
                    } else {
                        State::Ground
                    };
                }
                State::Number { matched } => {
                    let expected = NUMBER.get(usize::from(matched)).copied();
                    self.state = match expected {
                        Some(expected) if expected == byte => {
                            if usize::from(matched) + 1 == NUMBER.len() {
                                self.payload.clear();
                                self.overflow = false;
                                State::Payload
                            } else {
                                State::Number {
                                    matched: matched.saturating_add(1),
                                }
                            }
                        }
                        _ if byte == ESC => State::Escape,
                        _ => State::Other,
                    };
                }
                State::Payload => {
                    if byte == BEL {
                        self.finish();
                    } else if byte == ESC {
                        self.state = State::PayloadEscape;
                    } else {
                        self.overflow |= !self.payload.push(byte);
                    }
                }
                State::PayloadEscape => {
                    if byte == b'\\' {
                        self.finish();
                    } else if byte != ESC {
                        // An escape byte that ends no sequence aborts this one: its
                        // remaining bytes are skipped to the end of the sequence.
                        self.state = State::Other;
                    }
                }
                State::Other => {
                    self.state = if byte == BEL {
                        State::Ground
                    } else if byte == ESC {
                        State::OtherEscape
                    } else {
                        State::Other
                    };
                }
                State::OtherEscape => {
                    self.state = if byte == b'\\' {
                        State::Ground
                    } else if byte == ESC {
                        State::Escape
                    } else {
                        State::Other
                    };
                }
            }
            index += 1;
        }
    }

    /// Hands the payload of a completed sequence on, unless it broke the bound.
    fn finish(&mut self) {
        self.state = State::Ground;
        if self.overflow {
            self.overflow = false;
            self.payload.clear();
            return;
        }
        self.tap.report(self.payload);
        self.payload.clear();
    }
}

#[cfg(test)]
mod tests;
