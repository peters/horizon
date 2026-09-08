use super::{ArtifactDigest, SeedError, source_error};
use sha2::{Digest, Sha256};
use std::io::{self, Write};

pub(super) struct Summary {
    pub(super) bytes: u64,
    pub(super) sha256: ArtifactDigest,
}

pub(super) fn copy(
    read: &mut impl FnMut(&mut [u8]) -> io::Result<usize>,
    output: &mut impl Write,
    limit: u64,
    objects: usize,
) -> Result<Summary, SeedError> {
    let mut header = [0; 12];
    let mut position = 0;
    while position < header.len() {
        let count = read(&mut header[position..]).map_err(|error| source_error(error.kind()))?;
        if count == 0 {
            return Err(SeedError::Object);
        }
        position += count;
    }
    let count = u32::from_be_bytes(header[8..].try_into().map_err(|_| SeedError::Object)?);
    if header[..8] != *b"PACK\0\0\0\x02" || count == 0 || u64::from(count) > objects as u64 || limit < 32 {
        return Err(SeedError::Object);
    }
    let mut hash = Sha256::new();
    hash.update(header);
    output.write_all(&header).map_err(|_| SeedError::Storage)?;
    let mut total = header.len() as u64;
    let mut buffer = [0; 16 * 1024];
    loop {
        let length = usize::try_from(limit.saturating_sub(total).saturating_add(1).min(buffer.len() as u64))
            .map_err(|_| SeedError::Limit)?;
        let count = read(&mut buffer[..length]).map_err(|error| source_error(error.kind()))?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .filter(|bytes| *bytes <= limit)
            .ok_or(SeedError::Limit)?;
        output.write_all(&buffer[..count]).map_err(|_| SeedError::Storage)?;
        hash.update(&buffer[..count]);
    }
    if total < 32 {
        return Err(SeedError::Object);
    }
    Ok(Summary {
        bytes: total,
        sha256: ArtifactDigest::from_sha256_bytes(hash.finalize().into()),
    })
}
