//! Length-prefixed frames: four bytes of big-endian length, then one JSON
//! document.
//!
//! The length prefix lets a reader refuse an oversize frame before it reads the
//! body, so no computer can make the server hold a frame of its own size. A reader
//! at the end of a link answers `None`; a link that ends inside a frame is an
//! error.

use std::{
    io::{self, Read, Write},
    net::{Shutdown, TcpStream},
    os::unix::net::UnixStream,
};

use crate::protocol::Frame;

/// Where the frames of one link go.
///
/// A link that the server ends stops writing before it closes, so the computer
/// reads the typed error frame before its side sees the end of the link. A socket
/// that closes while a frame of the computer is still queued resets the link
/// instead, and the computer would lose that error.
pub trait FrameWriter: Write {
    /// Ends the writing side of the link without closing the reading side.
    ///
    /// # Errors
    /// Returns why the writing side cannot be ended.
    fn finish(&mut self) -> io::Result<()>;
}

impl FrameWriter for UnixStream {
    fn finish(&mut self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

impl FrameWriter for TcpStream {
    fn finish(&mut self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

/// A writing side that has no half to end, such as a file of a test.
impl FrameWriter for Vec<u8> {
    fn finish(&mut self) -> io::Result<()> {
        self.flush()
    }
}

/// The most bytes of one frame, and the most bytes of one JSON document that a
/// link carries.
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

/// Why a frame could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The frame is longer than the protocol allows.
    #[error("the frame is longer than the maximum of {MAX_FRAME_BYTES} bytes")]
    TooLarge { size: usize },
    /// The frame is not one document of this protocol.
    #[error("the frame is not one frame of this protocol: {0}")]
    Json(String),
    /// The link ended or failed.
    #[error("the link: {0}")]
    Io(#[from] io::Error),
}

impl FrameError {
    /// The code that a link reports for this error.
    #[must_use]
    pub fn code(&self) -> crate::protocol::LinkErrorCode {
        match self {
            Self::TooLarge { .. } => crate::protocol::LinkErrorCode::FrameTooLarge,
            Self::Json(_) | Self::Io(_) => crate::protocol::LinkErrorCode::BadFrame,
        }
    }
}

/// Reads one frame, or `None` when the link ended before its next frame.
///
/// # Errors
/// Refuses an oversize frame and a frame that is not one document of this
/// protocol, and returns why the link failed.
pub fn read_frame<R: Read>(reader: &mut R) -> Result<Option<Frame>, FrameError> {
    let Some(length) = read_length(reader)? else {
        return Ok(None);
    };
    let length = usize::try_from(u32::from_be_bytes(length)).unwrap_or(MAX_FRAME_BYTES + 1);
    if length == 0 {
        return Err(FrameError::Json("an empty frame".to_owned()));
    }
    if length > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { size: length });
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            FrameError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the link ended inside a frame",
            ))
        } else {
            FrameError::Io(error)
        }
    })?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| FrameError::Json(error.to_string()))
}

/// Writes one frame and flushes it, so the other side can read it now.
///
/// # Errors
/// Refuses a frame above the maximum, and returns why the link failed.
pub fn write_frame<W: Write + ?Sized>(writer: &mut W, frame: &Frame) -> Result<(), FrameError> {
    let body = serde_json::to_vec(frame).map_err(|error| FrameError::Json(error.to_string()))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { size: body.len() });
    }
    let length = u32::try_from(body.len()).map_err(|_| FrameError::TooLarge { size: body.len() })?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

/// Four bytes of length, or `None` when the link ended cleanly before them.
fn read_length<R: Read>(reader: &mut R) -> Result<Option<[u8; 4]>, FrameError> {
    let mut length = [0u8; 4];
    let mut filled = 0;
    while filled < length.len() {
        match reader.read(&mut length[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => {
                return Err(FrameError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the link ended inside a frame length",
                )));
            }
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FrameError::Io(error)),
        }
    }
    Ok(Some(length))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::protocol::{Pull, Wait};

    fn encoded(frame: &Frame) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, frame).expect("the frame is written");
        bytes
    }

    #[test]
    fn a_frame_is_its_length_then_one_document() {
        let frame = Frame::Pull(Pull {
            since: 7,
            tip: "9f2c".to_owned(),
        });
        let bytes = encoded(&frame);
        let length = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(
            usize::try_from(length).expect("one length"),
            bytes.len() - 4,
            "the prefix counts the document alone"
        );
        let body = std::str::from_utf8(&bytes[4..]).expect("the document is text");
        assert_eq!(body, r#"{"pull":{"since":7,"tip":"9f2c"}}"#);
        assert_eq!(
            read_frame(&mut Cursor::new(bytes))
                .expect("the frame reads")
                .expect("one frame"),
            frame
        );
    }

    #[test]
    fn a_link_that_ends_between_frames_is_not_an_error() {
        assert_eq!(read_frame(&mut Cursor::new(Vec::new())).expect("no frame"), None);
        let mut cut = encoded(&Frame::Wait(Wait { since: 1 }));
        cut.truncate(2);
        let error = read_frame(&mut Cursor::new(cut)).expect_err("half a length is an error");
        assert!(matches!(error, FrameError::Io(_)), "{error}");
    }

    #[test]
    fn a_frame_that_ends_inside_its_body_is_an_error() {
        let mut cut = encoded(&Frame::Wait(Wait { since: 1 }));
        cut.truncate(cut.len() - 2);
        let error = read_frame(&mut Cursor::new(cut)).expect_err("a cut body is an error");
        assert!(matches!(error, FrameError::Io(_)), "{error}");
    }

    #[test]
    fn an_oversize_frame_is_refused_before_its_body_is_read() {
        let mut bytes = u32::try_from(MAX_FRAME_BYTES + 1)
            .expect("one length")
            .to_be_bytes()
            .to_vec();
        bytes.extend_from_slice(b"{}");
        let error = read_frame(&mut Cursor::new(bytes)).expect_err("too long");
        assert!(matches!(error, FrameError::TooLarge { size } if size == MAX_FRAME_BYTES + 1));
        assert_eq!(error.code(), crate::protocol::LinkErrorCode::FrameTooLarge);
    }

    #[test]
    fn a_document_that_is_not_a_frame_and_an_empty_frame_are_errors() {
        let mut bytes = 2u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"{}");
        let error = read_frame(&mut Cursor::new(bytes)).expect_err("not one frame");
        assert!(matches!(error, FrameError::Json(_)), "{error}");

        let bytes = 0u32.to_be_bytes().to_vec();
        let error = read_frame(&mut Cursor::new(bytes)).expect_err("an empty frame");
        assert!(matches!(error, FrameError::Json(_)), "{error}");
    }

    #[test]
    fn an_oversize_frame_is_not_written() {
        let huge = Frame::Pull(Pull {
            since: 1,
            tip: "a".repeat(MAX_FRAME_BYTES),
        });
        let mut bytes = Vec::new();
        let error = write_frame(&mut bytes, &huge).expect_err("too long to write");
        assert!(matches!(error, FrameError::TooLarge { .. }), "{error}");
        assert!(bytes.is_empty(), "nothing of an oversize frame reaches the link");
    }
}
