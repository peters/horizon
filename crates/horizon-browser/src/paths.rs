use std::path::PathBuf;

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

pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// A folder name a person can read: the identifier keeps the characters a
/// person typed and encodes every other byte, so it fits one path component and
/// never becomes `.` or `..`.
#[must_use]
pub(crate) fn readable_id(local_id: &str) -> String {
    let mut encoded = String::with_capacity(local_id.len().max(1));
    for byte in local_id.bytes() {
        let character = char::from(byte);
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            encoded.push(character);
        } else {
            encoded.push('%');
            encoded.push(char::from(LOWER_HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(LOWER_HEX[usize::from(byte & 0x0f)]));
        }
    }
    match encoded.as_str() {
        "" | "." | ".." => format!("%{}{}", LOWER_HEX[0], LOWER_HEX[0]),
        _ => encoded,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_readable_id_keeps_typed_characters_and_encodes_the_rest() {
        assert_eq!(
            readable_id("3f4c1e2a-0000-4000-8000-000000000000"),
            "3f4c1e2a-0000-4000-8000-000000000000"
        );
        assert_eq!(readable_id("panel_1"), "panel_1");
        assert_eq!(readable_id("a/b"), "a%2fb");
        assert_eq!(readable_id("a b"), "a%20b");
        assert_eq!(readable_id("æ"), "%c3%a6");
    }

    #[test]
    fn a_readable_id_is_never_a_dot_or_empty_component() {
        for name in ["", ".", ".."] {
            let encoded = readable_id(name);
            assert!(!encoded.is_empty(), "{name:?}");
            assert_ne!(encoded, ".", "{name:?}");
            assert_ne!(encoded, "..", "{name:?}");
        }
    }
}
