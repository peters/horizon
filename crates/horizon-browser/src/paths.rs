use std::path::{Path, PathBuf};

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

#[must_use]
pub(crate) fn default_root() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).map_or_else(
        || PathBuf::from(".horizon-browser"),
        |home| home.join(".horizon-browser"),
    )
}

#[must_use]
pub(crate) fn expand_tilde(input: &str) -> PathBuf {
    if input == "~" {
        home_dir()
    } else if let Some(rest) = input.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(input)
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

#[must_use]
pub(crate) fn safe_local_id(local_id: &str) -> String {
    let mut encoded = String::with_capacity(1 + local_id.len() * 2);
    encoded.push('%');
    for byte in local_id.bytes() {
        encoded.push(char::from(LOWER_HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(LOWER_HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[must_use]
pub(crate) fn browser_profile_dir(root: &Path, local_id: &str) -> PathBuf {
    root.join("profiles").join(safe_local_id(local_id))
}
