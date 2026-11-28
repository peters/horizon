//! The lines of the standard output of one program: one bounded line, and the
//! head of a line that is above the bound.
//!
//! A line of at most [`MAX_LINE_BYTES`] bytes arrives as the program wrote it. A
//! longer line is read to its newline and held nowhere, but its head is not
//! thrown away: the reader parses the top level `type`, `subtype` and `is_error`
//! of it in a stream, so the codec can end the turn of a `result` line that the
//! reader could not hold, and it skips every other value without holding it.
//!
//! The memory of this module has three bounds: [`MAX_LINE_BYTES`] for the line
//! that it keeps, [`MAX_SHAPE_BYTES`] for the bytes of one line that it reads for
//! the shape of its head, and [`MAX_SHAPE_CHARS`] for one text of that head. Every
//! other byte of an over-long line reaches its newline and is dropped as it
//! arrives.

use std::io::{BufRead, ErrorKind, Read};

use serde::Deserialize;
use serde::de::{Error as DeError, Visitor};

use super::MAX_LINE_BYTES;
use super::codec::LineShape;

/// The bytes of one line that the reader may read for the shape of its head
/// after the line passed [`MAX_LINE_BYTES`]. A longer line is dropped without a
/// shape.
pub(super) const MAX_SHAPE_BYTES: u64 = 64 * 1024 * 1024;

/// The characters of one text of the head of such a line: its `type`, its
/// `subtype` and nothing more, so a hostile text of megabytes becomes no value
/// of this process.
pub(super) const MAX_SHAPE_CHARS: usize = 64;

/// The answer of one bounded read of one line of the program.
pub(super) enum ReadLine {
    /// One whole line, without its newline, within the bound.
    Line,
    /// A line above the bound, read to its newline and held nowhere. The shape
    /// of its head is all that the codec reads of it.
    TooLong(LineShape),
    /// The end of the standard output.
    End,
}

/// Reads one line of at most [`MAX_LINE_BYTES`] bytes. A longer line is read to
/// its newline and answers [`ReadLine::TooLong`] with the shape of its head.
pub(super) fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> std::io::Result<ReadLine> {
    line.clear();
    loop {
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if chunk.is_empty() {
            return Ok(if line.is_empty() { ReadLine::End } else { ReadLine::Line });
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(chunk.len());
        if line.len() + take > MAX_LINE_BYTES {
            // The line is above the bound: the bytes of it that are held are the
            // head that the codec reads, and the rest of it is read to its
            // newline without a byte of it.
            let shape = read_shape(reader, line)?;
            line.clear();
            return Ok(ReadLine::TooLong(shape));
        }
        line.extend_from_slice(&chunk[..take]);
        let consumed = if newline.is_some() { take + 1 } else { take };
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(ReadLine::Line);
        }
    }
}

/// Reads the rest of the line of `head` and answers the shape of the head of
/// that line, or the shape of nothing when the line is not one object of the
/// line format. The rest of the line reaches its newline and is held nowhere.
fn read_shape(reader: &mut impl BufRead, head: &[u8]) -> std::io::Result<LineShape> {
    let left = MAX_SHAPE_BYTES.saturating_sub(head.len() as u64);
    let shape = {
        let mut rest = LineRest { reader, left };
        let mut deserializer = serde_json::Deserializer::from_reader(head.chain(&mut rest));
        ShapeOfLine::deserialize(&mut deserializer)
            .map(LineShape::from)
            .unwrap_or_default()
    };
    drain_line(reader)?;
    Ok(shape)
}

/// Reads the rest of one line to its newline and holds no byte of it.
fn drain_line(reader: &mut impl BufRead) -> std::io::Result<()> {
    loop {
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if chunk.is_empty() {
            return Ok(());
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(chunk.len());
        reader.consume(if newline.is_some() { take + 1 } else { take });
        if newline.is_some() {
            return Ok(());
        }
    }
}

/// The rest of one line of the program as the shape read sees it: the bytes of
/// that line up to its newline, and at most what is left of [`MAX_SHAPE_BYTES`].
/// The newline stays in the reader for [`drain_line`].
struct LineRest<'a, R> {
    reader: &'a mut R,
    left: u64,
}

impl<R: BufRead> Read for LineRest<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 || buffer.is_empty() {
            return Ok(0);
        }
        loop {
            let chunk = match self.reader.fill_buf() {
                Ok(chunk) => chunk,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            let take = buffer
                .len()
                .min(chunk.len())
                .min(usize::try_from(self.left).unwrap_or(usize::MAX));
            let take = chunk[..take].iter().position(|byte| *byte == b'\n').unwrap_or(take);
            if take == 0 {
                return Ok(0);
            }
            buffer[..take].copy_from_slice(&chunk[..take]);
            self.reader.consume(take);
            self.left -= take as u64;
            return Ok(take);
        }
    }
}

/// The head of a line above the bound, as `serde` reads it: the three fields of
/// its top level that say what the loss of the line costs. Every other field is
/// skipped without a value.
#[derive(Debug, Default, Deserialize)]
struct ShapeOfLine {
    #[serde(rename = "type", default)]
    kind: Option<ShortText>,
    #[serde(default)]
    subtype: Option<ShortText>,
    #[serde(default)]
    is_error: Option<bool>,
}

impl From<ShapeOfLine> for LineShape {
    fn from(head: ShapeOfLine) -> Self {
        Self {
            kind: head.kind.map(|text| text.0),
            subtype: head.subtype.map(|text| text.0),
            is_error: head.is_error.unwrap_or(false),
        }
    }
}

/// One text of the head of a line, which gives up above [`MAX_SHAPE_CHARS`]
/// bytes, so a hostile text of megabytes does not become a value of this
/// process.
#[derive(Debug)]
struct ShortText(String);

impl<'de> Deserialize<'de> for ShortText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ShortTextVisitor;

        impl Visitor<'_> for ShortTextVisitor {
            type Value = ShortText;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "a text of at most {MAX_SHAPE_CHARS} bytes")
            }

            fn visit_str<E>(self, text: &str) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                if text.len() > MAX_SHAPE_CHARS {
                    return Err(E::custom("a text of a line is above its bound"));
                }
                Ok(ShortText(text.to_owned()))
            }
        }

        deserializer.deserialize_str(ShortTextVisitor)
    }
}
