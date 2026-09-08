use super::{PrivateCheckoutError as Error, files, linux::check_cancel};
use crate::repository_overlay::MAX_CONTENT_BYTES;
use flate2::{Decompress, FlushDecompress, Status};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Write},
};

/// Only standard loose blobs emitted by the fresh seed writer are supported.
pub(super) fn copy(
    input: &File,
    bytes: u64,
    output: &mut impl Write,
    cancelled: &impl Fn() -> bool,
) -> Result<(), Error> {
    let metadata = input.metadata().map_err(|_| Error::Storage)?;
    files::regular(&metadata)?;
    if bytes > MAX_CONTENT_BYTES || metadata.len() > bytes + bytes / 100 + 65_536 {
        return Err(Error::Object);
    }
    let mut reader = BufReader::with_capacity(16 * 1024, input.take(metadata.len()));
    let header = format!("blob {bytes}\0");
    let mut header_read = 0;
    let mut remaining = bytes;
    let mut decoder = Decompress::new(true);
    let mut buffer = [0u8; 16 * 1024];
    loop {
        check_cancel(cancelled)?;
        let before_in = decoder.total_in();
        let before_out = decoder.total_out();
        let status = decoder
            .decompress(
                reader.fill_buf().map_err(|_| Error::Storage)?,
                &mut buffer,
                FlushDecompress::None,
            )
            .map_err(|_| Error::Object)?;
        let consumed = usize::try_from(decoder.total_in() - before_in).map_err(|_| Error::Object)?;
        let produced = usize::try_from(decoder.total_out() - before_out).map_err(|_| Error::Object)?;
        reader.consume(consumed);
        let prefix = produced.min(header.len() - header_read);
        if buffer[..prefix] != header.as_bytes()[header_read..header_read + prefix] {
            return Err(Error::Object);
        }
        header_read += prefix;
        let payload = &buffer[prefix..produced];
        remaining = remaining.checked_sub(payload.len() as u64).ok_or(Error::Object)?;
        output.write_all(payload).map_err(|_| Error::Storage)?;
        if status == Status::StreamEnd {
            if remaining != 0 || header_read != header.len() || decoder.total_in() != metadata.len() {
                return Err(Error::Object);
            }
            break;
        }
        if consumed == 0 && produced == 0 {
            return Err(Error::Object);
        }
    }
    check_cancel(cancelled)?;
    if !files::same(&metadata, &input.metadata().map_err(|_| Error::Storage)?) {
        return Err(Error::UnsafeNode);
    }
    Ok(())
}
