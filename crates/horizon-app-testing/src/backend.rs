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
    let bytes = bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(bytes);
    if bytes.contains(&b'\n') || bytes.contains(&b'\r') {
        return Err(Error::BackendReadyInvalid);
    }
    let ready: Ready = serde_json::from_slice(bytes).map_err(|_| Error::BackendReadyInvalid)?;
    if ready.native_backend_ready != 1 || ready.port == 0 {
        return Err(Error::BackendReadyInvalid);
    }
    Ok(ready.port)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_accepts_only_one_optional_line_ending() {
        for suffix in ["", "\n", "\r\n"] {
            assert_eq!(
                ready_port(format!("{{\"native_backend_ready\":1,\"port\":8080}}{suffix}").as_bytes()),
                Ok(8080)
            );
        }
        for invalid in [
            b"{\n\"native_backend_ready\":1,\"port\":8080}".as_slice(),
            b"{\"native_backend_ready\":1,\"port\":8080}\n\n",
            b"{\"native_backend_ready\":1,\"port\":8080}\r",
        ] {
            assert_eq!(ready_port(invalid), Err(Error::BackendReadyInvalid));
        }
    }
}
