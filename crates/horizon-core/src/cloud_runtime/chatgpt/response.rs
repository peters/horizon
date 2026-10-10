//! Bounded parsing with format-only diagnostics; credential values never enter logs.
use super::{Error, Result};
use std::io::Read as _;

pub(super) fn read<T: serde::de::DeserializeOwned>(mut body: ureq::Body, stage: &'static str) -> Result<T> {
    let mut bytes = zeroize::Zeroizing::new(Vec::with_capacity(64 * 1024 + 1));
    body.as_reader()
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Malformed)?;
    if bytes.len() > 64 * 1024 {
        return Err(Error::Malformed);
    }
    parse(&bytes, stage)
}

pub(super) fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8], stage: &'static str) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|error| {
        tracing::warn!(stage, category = ?error.classify(), line = error.line(), column = error.column(), "sign-in response format was not recognized");
        Error::Malformed
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_response_budget_accepts_valid_json_at_the_limit() {
        let mut bytes = vec![b' '; 64 * 1024];
        bytes[..2].copy_from_slice(b"{}");
        let body = ureq::Body::builder().data(bytes);
        assert_eq!(
            read::<serde_json::Value>(body, "test response").unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn the_response_budget_rejects_valid_json_over_the_limit() {
        let mut bytes = vec![b' '; 64 * 1024 + 1];
        bytes[..2].copy_from_slice(b"{}");
        let body = ureq::Body::builder().reader(std::io::Cursor::new(bytes));
        assert!(matches!(
            read::<serde_json::Value>(body, "test response"),
            Err(Error::Malformed)
        ));
    }
}
