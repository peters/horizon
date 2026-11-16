//! The reader that shows every byte of a program's output and looks at some of them.

use std::io::{self, Read};

use crate::inbox::StatusTap;
use crate::scanner::Scanner;

/// A reader over another reader that hands every byte on unchanged.
///
/// The scanner looks at the bytes on their way past, so one pass over the output
/// serves both the VT parser and the status reports, and the screen of the panel
/// shows exactly what the program wrote.
pub struct StatusReader<R> {
    source: R,
    scanner: Scanner,
}

impl<R> StatusReader<R> {
    /// Reads `source` through a scanner whose payloads go to `tap`.
    pub fn new(source: R, tap: StatusTap) -> Self {
        Self {
            source,
            scanner: Scanner::new(tap),
        }
    }
}

impl<R> StatusReader<R> {
    /// The reader under the scanner.
    pub fn source(&self) -> &R {
        &self.source
    }

    /// The reader under the scanner, for the wrapper that asks for its end.
    pub fn source_mut(&mut self) -> &mut R {
        &mut self.source
    }
}

impl<R: Read> Read for StatusReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.source.read(buffer)?;
        self.scanner.feed(&buffer[..read]);
        Ok(read)
    }
}
