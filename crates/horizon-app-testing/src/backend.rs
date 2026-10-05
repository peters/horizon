//! Bounded foreground-backend readiness protocol; child output is never a public diagnostic.
use crate::{Error, Result};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    native_backend_ready: u32,
    port: u16,
}

/// # Errors
/// Accepts one versioned JSON line, without hosts, credentials, extra fields or zero ports.
pub fn ready_port(bytes: &[u8]) -> Result<u16> {
    if bytes.len() > 256 {
        return Err(Error::BackendReadyInvalid);
    }
    let ready: Ready = serde_json::from_slice(bytes).map_err(|_| Error::BackendReadyInvalid)?;
    if ready.native_backend_ready != 1 || ready.port == 0 {
        return Err(Error::BackendReadyInvalid);
    }
    Ok(ready.port)
}
