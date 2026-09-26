//! Attachment authorizes an existing terminal transport, never process creation.
use crate::{
    OperationId,
    bootstrap::{RecoveryRequest, Startup},
    membership::SessionId,
};
use serde::{Deserialize, Serialize};

pub const MAX_BYTES: usize = 16 * 1024;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub startup: Startup,
    pub worker_id: String,
    pub session_id: SessionId,
    pub launch: OperationId,
}

/// A shell-safe argument; do not log or persist this authorization.
/// # Errors
/// Rejects oversized or invalid request serialization.
pub fn encode(request: &RecoveryRequest) -> Result<String, crate::signed::Error> {
    use std::fmt::Write;
    let bytes = serde_json::to_vec(request).map_err(|_| crate::signed::Error::Encoding)?;
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err(crate::signed::Error::Encoding);
    }
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").map_err(|_| crate::signed::Error::Encoding)?;
    }
    Ok(encoded)
}
/// # Errors
/// Accepts only bounded canonical lowercase hex, never shell punctuation.
pub fn decode(encoded: &str) -> Result<RecoveryRequest, crate::signed::Error> {
    let bad = || crate::signed::Error::Encoding;
    if encoded.is_empty() || encoded.len() > MAX_BYTES * 2 || !encoded.len().is_multiple_of(2) {
        return Err(bad());
    }
    let nibble = |c| match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        _ => Err(bad()),
    };
    let bytes = encoded
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| Ok(nibble(c[0])? * 16 + nibble(c[1])?))
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::from_slice(&bytes).map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authorization_argument_is_bounded_canonical_and_not_shell_text() {
        let request = RecoveryRequest {
            message: "signed".into(),
            payload: "payload".into(),
        };
        let encoded = encode(&request).unwrap();
        assert_eq!(decode(&encoded).unwrap().message, request.message);
        for invalid in ["", "0", "AA", "$(id)", "00\n", "0g", "00"] {
            assert!(decode(invalid).is_err());
        }
        assert!(decode(&"00".repeat(MAX_BYTES + 1)).is_err());
    }
}
