//! The tip of the log at an index: a short name for "the server log at this
//! index", so a computer that is ahead of a restored server learns to reset.
//!
//! The function is the same on both sides:
//!
//! ```text
//! tip = lower hex of the first 16 bytes of
//!       SHA-256("horizon-sync-tip-v1\n" + store id + "\n" + decimal index)
//! ```
//!
//! The store id is a random identifier that the server writes once, at the
//! creation of its file. Two different server files therefore have two different
//! tips for the same index, and a copy of the log at an older index has a
//! different tip than the index that a computer holds. A tip is not a secret and
//! it proves nothing: it only says whether the two sides talk about the same log
//! row.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};

/// The version and the field separator that the tip hashes, so a later format
/// cannot collide with this one.
const DOMAIN: &[u8] = b"horizon-sync-tip-v1\n";

/// The number of digest bytes that name a tip, as 32 hexadecimal characters.
const TIP_BYTES: usize = 16;

/// The tip of `store_id` at `index`.
#[must_use]
pub fn tip(store_id: &str, index: u64) -> String {
    let mut hash = Sha256::new();
    hash.update(DOMAIN);
    hash.update(store_id.as_bytes());
    hash.update(b"\n");
    hash.update(index.to_string().as_bytes());
    let digest = hash.finalize();
    let mut out = String::with_capacity(TIP_BYTES * 2);
    for byte in &digest[..TIP_BYTES] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tip_names_one_store_and_one_index_and_nothing_else() {
        assert_eq!(tip("store-a", 0), tip("store-a", 0), "the same pair is the same tip");
        assert_eq!(tip("store-a", 0).len(), 32, "16 bytes as lower hex");
        assert!(
            tip("store-a", 0)
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "lower hex only"
        );
        let mut tips = std::collections::BTreeSet::new();
        for store in ["store-a", "store-b"] {
            for index in [0, 1, 2, 7, 118] {
                assert!(tips.insert(tip(store, index)), "{store} at {index} names a new tip");
            }
        }
    }

    #[test]
    fn a_tip_does_not_move_when_an_index_is_written_differently() {
        assert_ne!(
            tip("store-a", 1),
            tip("store-a", 10),
            "the decimal index is one field, so 1 and 10 do not share a tip"
        );
        assert_ne!(tip("ab", 1), tip("a", 11), "the store id is one field too");
    }
}
