//! The frames between a holder and its client: one kind byte, a 4-byte length in
//! big-endian order, and the payload. The output of the program is sent as bytes,
//! never as JSON, because a program can write a megabyte in a second.
//!
//! A terminal that attaches to a holder reads the same frames: its reader yields
//! the bytes of a `replay` or `output` frame, and its writer wraps input in an
//! `input` frame.

use std::io::{self, Read, Write};

/// Why a frame of a holder link cannot be written or read.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("a frame of {length} bytes is above the bound of {MAX_PAYLOAD} bytes")]
    TooLarge { length: u32 },
    #[error("a frame of the unknown kind {0}")]
    UnknownKind(u8),
    #[error("a frame of the kind {0} has the wrong length")]
    Malformed(u8),
    #[error("the link of a holder: {0}")]
    Io(#[from] io::Error),
}

/// The version of this protocol, which the hello of a holder names.
pub(crate) const VERSION: u8 = 1;

/// The largest payload of one frame, in either direction.
pub(crate) const MAX_PAYLOAD: u32 = 1024 * 1024;

/// The bytes of a kind and a length, before every payload.
pub(crate) const HEADER: usize = 5;

/// The bytes of a size: rows, columns, cell width and cell height.
const SIZE: usize = 8;

pub(crate) const INPUT: u8 = 1;
pub(crate) const SIZE_KIND: u8 = 2;
pub(crate) const CLOSE: u8 = 3;
const HELLO: u8 = 16;
const REPLAY: u8 = 17;
const OUTPUT: u8 = 18;
const EXIT: u8 = 19;
const REFUSAL: u8 = 20;

/// The first answer that a holder gives a client that attached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    /// The process id of the program in the terminal.
    pub pid: u32,
    /// The start time of that process, as Horizon records the child of a panel.
    pub start_time: Option<String>,
    /// The bytes of the replay frame of this link, which follow the hello.
    pub ring_len: u32,
}

/// One frame of a holder and its client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// A client: the bytes to write to the program.
    Input(Vec<u8>),
    /// A client: the size of the terminal.
    Resize {
        rows: u16,
        cols: u16,
        cell_width: u16,
        cell_height: u16,
    },
    /// A client: end the program and the holder.
    Close,
    /// A holder: the answer to a client that attached.
    Hello(Hello),
    /// A holder: the newest output of the program from before the attach.
    Replay(Vec<u8>),
    /// A holder: output of the program.
    Output(Vec<u8>),
    /// A holder: the program ended, with the status that the system reported.
    Exit(Option<i32>),
    /// A holder: another client is attached, so this link closes.
    Refusal,
}

impl Frame {
    /// The kind byte of this frame.
    #[must_use]
    pub fn kind(&self) -> u8 {
        match self {
            Self::Input(_) => INPUT,
            Self::Resize { .. } => SIZE_KIND,
            Self::Close => CLOSE,
            Self::Hello(_) => HELLO,
            Self::Replay(_) => REPLAY,
            Self::Output(_) => OUTPUT,
            Self::Exit(_) => EXIT,
            Self::Refusal => REFUSAL,
        }
    }

    fn parts(&self) -> (u8, Vec<u8>) {
        match self {
            Self::Input(bytes) | Self::Replay(bytes) | Self::Output(bytes) => (self.kind(), bytes.clone()),
            Self::Resize {
                rows,
                cols,
                cell_width,
                cell_height,
            } => (SIZE_KIND, size(*rows, *cols, *cell_width, *cell_height).to_vec()),
            Self::Close | Self::Refusal => (self.kind(), Vec::new()),
            Self::Hello(hello) => (HELLO, hello.encode()),
            Self::Exit(status) => {
                let mut payload = Vec::with_capacity(5);
                payload.push(u8::from(status.is_some()));
                payload.extend_from_slice(&status.unwrap_or_default().to_be_bytes());
                (EXIT, payload)
            }
        }
    }

    fn decode(kind: u8, payload: Vec<u8>) -> Result<Self, FrameError> {
        Ok(match kind {
            INPUT => Self::Input(payload),
            SIZE_KIND => {
                if payload.len() != SIZE {
                    return Err(FrameError::Malformed(kind));
                }
                let field = |at: usize| u16::from_be_bytes([payload[at], payload[at + 1]]);
                Self::Resize {
                    rows: field(0),
                    cols: field(2),
                    cell_width: field(4),
                    cell_height: field(6),
                }
            }
            CLOSE => Self::Close,
            HELLO => Self::Hello(Hello::decode(&payload)?),
            REPLAY => Self::Replay(payload),
            OUTPUT => Self::Output(payload),
            EXIT => {
                if payload.len() != 5 {
                    return Err(FrameError::Malformed(kind));
                }
                let code = i32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
                Self::Exit((payload[0] != 0).then_some(code))
            }
            REFUSAL => Self::Refusal,
            unknown => return Err(FrameError::UnknownKind(unknown)),
        })
    }
}

impl Hello {
    fn encode(&self) -> Vec<u8> {
        let start_time = self.start_time.as_deref().unwrap_or_default();
        let mut payload = Vec::with_capacity(11 + start_time.len());
        payload.push(VERSION);
        payload.extend_from_slice(&self.pid.to_be_bytes());
        let named = u16::try_from(start_time.len()).unwrap_or(u16::MAX);
        payload.extend_from_slice(&named.to_be_bytes());
        payload.extend_from_slice(&start_time.as_bytes()[..usize::from(named)]);
        payload.extend_from_slice(&self.ring_len.to_be_bytes());
        payload
    }

    fn decode(payload: &[u8]) -> Result<Self, FrameError> {
        if payload.len() < 11 || payload[0] != VERSION {
            return Err(FrameError::Malformed(HELLO));
        }
        let pid = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
        let named = usize::from(u16::from_be_bytes([payload[5], payload[6]]));
        let Some(body) = payload.get(7..) else {
            return Err(FrameError::Malformed(HELLO));
        };
        if body.len() != named + 4 {
            return Err(FrameError::Malformed(HELLO));
        }
        let (name, length) = body.split_at(named);
        let ring_len = u32::from_be_bytes([length[0], length[1], length[2], length[3]]);
        Ok(Self {
            pid,
            start_time: (!name.is_empty()).then(|| String::from_utf8_lossy(name).into_owned()),
            ring_len,
        })
    }
}

/// The payload of a size: rows, columns, cell width and cell height.
pub(crate) fn size(rows: u16, cols: u16, cell_width: u16, cell_height: u16) -> [u8; SIZE] {
    let mut payload = [0u8; SIZE];
    for (at, value) in [rows, cols, cell_width, cell_height].into_iter().enumerate() {
        payload[2 * at..2 * at + 2].copy_from_slice(&value.to_be_bytes());
    }
    payload
}

/// The bytes of the kind and the length of one frame with a payload of `length`.
pub(crate) fn header(kind: u8, length: usize) -> [u8; HEADER] {
    let mut header = [0u8; HEADER];
    header[0] = kind;
    header[1..].copy_from_slice(&u32::try_from(length).unwrap_or(MAX_PAYLOAD).to_be_bytes());
    header
}

/// The bytes of one frame with a payload of `payload`.
pub(crate) fn frame_bytes(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER + payload.len());
    bytes.extend_from_slice(&header(kind, payload.len()));
    bytes.extend_from_slice(payload);
    bytes
}

/// Writes `frame` as one frame.
///
/// # Errors
///
/// Returns why the frame cannot be written, or that its payload is too large.
pub fn write(writer: &mut impl Write, frame: &Frame) -> Result<(), FrameError> {
    let (kind, payload) = frame.parts();
    if payload.len() > MAX_PAYLOAD as usize {
        return Err(FrameError::TooLarge {
            length: u32::try_from(payload.len()).unwrap_or(MAX_PAYLOAD),
        });
    }
    writer.write_all(&header(kind, payload.len()))?;
    writer.write_all(&payload)?;
    Ok(())
}

/// Reads one frame from a reader that blocks until the frame arrives.
///
/// # Errors
///
/// Returns why the frame cannot be read, or that it is no frame of this protocol.
pub fn read_one(reader: &mut impl Read) -> Result<Frame, FrameError> {
    let mut header = [0u8; HEADER];
    reader.read_exact(&mut header)?;
    let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]);
    if length > MAX_PAYLOAD {
        return Err(FrameError::TooLarge { length });
    }
    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload)?;
    Frame::decode(header[0], payload)
}

/// The frames that one stream delivered, as far as a whole frame arrived.
#[derive(Default)]
pub struct Frames {
    bytes: Vec<u8>,
}

impl Frames {
    /// Adds the bytes that arrived and answers every frame that is now whole.
    ///
    /// # Errors
    ///
    /// Returns why the bytes are no frames of this protocol.
    pub fn push(&mut self, more: &[u8]) -> Result<Vec<Frame>, FrameError> {
        self.bytes.extend_from_slice(more);
        let mut frames = Vec::new();
        loop {
            if self.bytes.len() < HEADER {
                break;
            }
            let length = u32::from_be_bytes([self.bytes[1], self.bytes[2], self.bytes[3], self.bytes[4]]);
            if length > MAX_PAYLOAD {
                return Err(FrameError::TooLarge { length });
            }
            let whole = HEADER + length as usize;
            if self.bytes.len() < whole {
                break;
            }
            frames.push(Frame::decode(self.bytes[0], self.bytes[HEADER..whole].to_vec())?);
            self.bytes.drain(..whole);
        }
        Ok(frames)
    }
}
