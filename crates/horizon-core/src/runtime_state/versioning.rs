use std::borrow::Borrow;

use serde::{Deserialize, Deserializer, Serializer, de, ser};
use thiserror::Error;

use super::RUNTIME_STATE_VERSION;

pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let version = u32::deserialize(deserializer)?;
    validate(version).map_err(de::Error::custom)?;
    Ok(version)
}

pub(super) fn serialize<S>(version: impl Borrow<u32>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let version = *version.borrow();
    validate(version).map_err(ser::Error::custom)?;
    serializer.serialize_u32(version)
}

fn validate(version: u32) -> Result<(), UnsupportedVersion> {
    if version > RUNTIME_STATE_VERSION {
        return Err(UnsupportedVersion {
            found: version,
            supported: RUNTIME_STATE_VERSION,
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
#[error("unsupported runtime state version {found}; maximum supported version is {supported}")]
struct UnsupportedVersion {
    found: u32,
    supported: u32,
}
