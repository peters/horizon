use super::Error;
use std::{fs::File, io::Read};

pub(super) fn bounded(path: impl AsRef<std::path::Path>, max: usize) -> Result<Vec<u8>, Error> {
    let mut data = Vec::new();
    File::open(path)?.take((max + 1) as u64).read_to_end(&mut data)?;
    if data.len() > max {
        return Err(Error::Encoding);
    }
    Ok(data)
}
