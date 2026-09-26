//! Cursor traversal must finish before absence or uniqueness can be established.
use super::RunPod;
use crate::{Cancellation, CloudError};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pagination {
    next_cursor: Option<String>,
    has_next_page: bool,
}
impl RunPod {
    pub(super) fn pages(&self, path: &str, key: &str, cancel: &Cancellation) -> Result<Vec<Value>, CloudError> {
        let mut values = Vec::new();
        let mut cursors = HashSet::new();
        let mut next = path.to_owned();
        // A bounded incomplete traversal is an error, never an empty account.
        for _ in 0..1000 {
            let mut page = self.request("GET", &next, None, cancel)?;
            let rows = page
                .get_mut(key)
                .and_then(Value::as_array_mut)
                .ok_or(CloudError::InvalidResponse)?;
            values.append(rows);
            let pagination: Pagination =
                serde_json::from_value(page.get("pagination").cloned().ok_or(CloudError::InvalidResponse)?)
                    .map_err(|_| CloudError::InvalidResponse)?;
            if !pagination.has_next_page {
                if pagination.next_cursor.is_some() {
                    return Err(CloudError::InvalidResponse);
                }
                return Ok(values);
            }
            let cursor = pagination
                .next_cursor
                .filter(|v| !v.is_empty() && v.len() <= 4096)
                .ok_or(CloudError::InvalidResponse)?;
            if !cursors.insert(cursor.clone()) {
                return Err(CloudError::InvalidResponse);
            }
            let separator = if path.contains('?') { '&' } else { '?' };
            next = format!("{path}{separator}cursor={}", encode(&cursor));
        }
        Err(CloudError::InvalidResponse)
    }
}
fn encode(value: &str) -> String {
    use std::fmt::Write;
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}
