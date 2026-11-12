//! The header and the planes of one picture of a frame source program.
//!
//! A picture is 32 bytes of a header and then the planes, with no padding. The
//! header is read first and every bound of it is checked before one plane byte is
//! read, so a program cannot use a header to make this process allocate or read
//! beyond the contract. The size of a picture is the size of *its own* header: a
//! program that changes its size can send one or two pictures of the old size
//! after the news, and each of them is read with the size that it carries.
use std::io::{self, Read};

use crate::Error;

/// The eight bytes that one picture header starts with.
pub const MAGIC: [u8; 8] = *b"NV12FRM1";

/// The bytes of one picture header.
pub const HEADER_BYTES: usize = 32;

/// The smallest side of a picture.
pub const MIN_SIDE: u32 = 16;

/// The largest side of a picture.
pub const MAX_SIDE: u32 = 8192;

/// Why the bytes of a header are not one picture header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Fault {
    #[error("the header of a picture does not start with NV12FRM1")]
    Magic,
    #[error("the size {width}x{height} of a picture is not even and {MIN_SIDE} to {MAX_SIDE}")]
    Size { width: u32, height: u32 },
    #[error("a picture ended after {read} of {expected} bytes")]
    Short { read: usize, expected: usize },
}

/// The header of one picture: which picture it is, when it was captured and how
/// large it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub sequence: u64,
    /// The time of the capture, in microseconds since 1970, as the program gave it.
    pub captured_at_micros: u64,
    pub width: u32,
    pub height: u32,
}

impl Header {
    /// The header of `bytes`, or the bound that they break.
    ///
    /// # Errors
    /// Reports a magic of another format and a side that is odd, below [`MIN_SIDE`]
    /// or above [`MAX_SIDE`].
    pub fn parse(bytes: &[u8; HEADER_BYTES]) -> Result<Self, Fault> {
        if bytes[..MAGIC.len()] != MAGIC {
            return Err(Fault::Magic);
        }
        let number = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap_or_default());
        let side = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default());
        let header = Self {
            sequence: number(8),
            captured_at_micros: number(16),
            width: side(24),
            height: side(28),
        };
        if !header.is_valid() {
            return Err(Fault::Size {
                width: header.width,
                height: header.height,
            });
        }
        Ok(header)
    }

    /// Whether both sides are even and inside the bounds of the contract.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.width >= MIN_SIDE
            && self.width <= MAX_SIDE
            && self.height >= MIN_SIDE
            && self.height <= MAX_SIDE
            && self.width.is_multiple_of(2)
            && self.height.is_multiple_of(2)
    }

    /// The bytes of the Y plane.
    #[must_use]
    pub const fn y_bytes(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// The bytes of the interleaved UV plane: one pair for each 2x2 block.
    #[must_use]
    pub const fn uv_bytes(&self) -> usize {
        self.y_bytes() / 2
    }

    /// The size of the picture in pixels.
    #[must_use]
    pub const fn size(&self) -> [usize; 2] {
        [self.width as usize, self.height as usize]
    }
}

/// One picture as it came: its header and the two planes, Y first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub header: Header,
    /// `header.y_bytes()` of Y, then `header.uv_bytes()` of interleaved UV.
    pub planes: Vec<u8>,
}

impl Picture {
    /// The Y plane, or no byte when the planes are not the size of the header.
    #[must_use]
    pub fn y(&self) -> &[u8] {
        self.planes.get(..self.header.y_bytes()).unwrap_or_default()
    }

    /// The interleaved UV plane, or no byte when the planes are not the size of
    /// the header.
    #[must_use]
    pub fn uv(&self) -> &[u8] {
        self.planes.get(self.header.y_bytes()..).unwrap_or_default()
    }
}

/// The next picture of `reader`, or `None` at the end of the socket.
///
/// # Errors
/// Reports a header that breaks a bound, and a picture that ended before its last
/// plane byte. A base that answers a signal is tried again, so a short read is the
/// end of the socket and nothing else.
pub fn read(reader: &mut impl Read) -> Result<Option<Picture>, Error> {
    let mut bytes = [0_u8; HEADER_BYTES];
    match fill(reader, &mut bytes)? {
        0 => return Ok(None),
        read if read < HEADER_BYTES => {
            return Err(Error::Picture(Fault::Short {
                read,
                expected: HEADER_BYTES,
            }));
        }
        _ => {}
    }
    let header = Header::parse(&bytes)?;
    let expected = header.y_bytes() + header.uv_bytes();
    let mut planes = vec![0_u8; expected];
    let read = fill(reader, &mut planes)?;
    if read < expected {
        return Err(Error::Picture(Fault::Short { read, expected }));
    }
    Ok(Some(Picture { header, planes }))
}

/// Fill `buffer` from `reader`, answering how many bytes it read before the end.
fn fill(reader: &mut impl Read, buffer: &mut [u8]) -> Result<usize, Error> {
    let mut read = 0;
    while read < buffer.len() {
        match reader.read(&mut buffer[read..]) {
            Ok(0) => break,
            Ok(bytes) => read += bytes,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(Error::from(error)),
        }
    }
    Ok(read)
}

/// The header of one picture, for a test and for a stand-in that writes pictures.
#[must_use]
pub fn header_bytes(header: &Header) -> [u8; HEADER_BYTES] {
    let mut bytes = [0_u8; HEADER_BYTES];
    bytes[..MAGIC.len()].copy_from_slice(&MAGIC);
    bytes[8..16].copy_from_slice(&header.sequence.to_le_bytes());
    bytes[16..24].copy_from_slice(&header.captured_at_micros.to_le_bytes());
    bytes[24..28].copy_from_slice(&header.width.to_le_bytes());
    bytes[28..32].copy_from_slice(&header.height.to_le_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn header(width: u32, height: u32) -> Header {
        Header {
            sequence: 7,
            captured_at_micros: 1,
            width,
            height,
        }
    }

    fn picture_bytes(header: &Header, y: u8, uv: u8) -> Vec<u8> {
        let mut bytes = header_bytes(header).to_vec();
        bytes.extend(std::iter::repeat_n(y, header.y_bytes()));
        bytes.extend(std::iter::repeat_n(uv, header.uv_bytes()));
        bytes
    }

    #[test]
    fn a_header_round_trips_and_every_bound_is_checked() {
        let parsed = Header::parse(&header_bytes(&header(1920, 1080))).expect("a picture header");
        assert_eq!(parsed, header(1920, 1080));
        assert_eq!((parsed.y_bytes(), parsed.uv_bytes()), (1920 * 1080, 1920 * 1080 / 2));

        let mut bytes = header_bytes(&header(640, 480));
        bytes[0] = b'X';
        assert_eq!(Header::parse(&bytes), Err(Fault::Magic));
        for (width, height) in [(15, 16), (16, 15), (17, 16), (16, 17), (8194, 16), (16, 8194), (0, 0)] {
            assert_eq!(
                Header::parse(&header_bytes(&header(width, height))),
                Err(Fault::Size { width, height }),
                "{width}x{height}"
            );
        }
        for (width, height) in [(16, 16), (8192, 8192), (1920, 1080)] {
            assert!(
                Header::parse(&header_bytes(&header(width, height))).is_ok(),
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn a_short_read_is_the_end_of_one_picture_and_not_a_picture() {
        let header = header(16, 16);
        let whole = picture_bytes(&header, 1, 2);
        let mut stream = Cursor::new(whole.clone());
        let picture = read(&mut stream).expect("one picture").expect("a picture");
        assert_eq!(picture.header, header);
        assert_eq!(picture.planes.len(), header.y_bytes() + header.uv_bytes());
        assert!(read(&mut stream).expect("the end of the socket").is_none());

        for cut in [1, 20, HEADER_BYTES, HEADER_BYTES + 1, whole.len() - 1] {
            let mut stream = Cursor::new(whole[..cut].to_vec());
            let error = read(&mut stream).expect_err("a picture that ended early");
            assert!(matches!(error, Error::Picture(Fault::Short { .. })), "{cut}: {error:?}");
        }
        let mut empty = Cursor::new(Vec::new());
        assert!(read(&mut empty).expect("no picture").is_none());
    }

    #[test]
    fn each_picture_is_read_with_the_size_of_its_own_header() {
        let small = header(32, 16);
        let large = header(64, 32);
        let mut stream = Vec::new();
        stream.extend(picture_bytes(&small, 10, 20));
        stream.extend(picture_bytes(&large, 30, 40));
        stream.extend(picture_bytes(&small, 50, 60));
        let mut stream = Cursor::new(stream);
        // The size changes between two pictures, and a picture of the old size
        // arrives after the news: each is read with its own header.
        for (header, y, uv) in [(small, 10, 20), (large, 30, 40), (small, 50, 60)] {
            let picture = read(&mut stream).expect("a picture").expect("a picture");
            assert_eq!(picture.header, header);
            assert_eq!((picture.y()[0], picture.uv()[0]), (y, uv));
        }
        assert!(read(&mut stream).expect("the end").is_none());
    }

    #[test]
    fn a_reader_that_answers_an_interrupt_still_fills_its_buffer() {
        struct Interrupted {
            bytes: Vec<u8>,
            at: usize,
            interrupts: usize,
        }
        impl Read for Interrupted {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if self.interrupts > 0 {
                    self.interrupts -= 1;
                    return Err(io::Error::from(io::ErrorKind::Interrupted));
                }
                let take = buffer.len().min(3).min(self.bytes.len() - self.at);
                buffer[..take].copy_from_slice(&self.bytes[self.at..self.at + take]);
                self.at += take;
                Ok(take)
            }
        }
        let mut stream = Interrupted {
            bytes: picture_bytes(&header(16, 16), 4, 5),
            at: 0,
            interrupts: 4,
        };
        let picture = read(&mut stream).expect("a picture").expect("a picture");
        assert_eq!(picture.planes.len(), 16 * 16 + 16 * 16 / 2);
    }
}
