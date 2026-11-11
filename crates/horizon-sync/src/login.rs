//! The codes and tokens of a join, and the hashes that the server keeps.
//!
//! A code is 12 characters of Crockford base32, shown in three groups of four. It
//! is short on purpose: a person reads it from a window and types it on another
//! computer. The server keeps the SHA-256 of the code, so a copy of `server.sqlite`
//! holds no code that still works.
//!
//! A token is 32 bytes from the OS random source, written as the base64url text of
//! 43 characters. It is the whole login of one computer, and the server keeps the
//! SHA-256 of it, so the database holds no token that a link could present.
//!
//! Both hashes are lower-case hexadecimal text, which is one value of a column and
//! one line of a log.

use std::fmt::Write as _;

use chacha20poly1305::aead::Generate;
use horizon_core::server_link::{CODE_CHARS, normalize_code};
use sha2::{Digest as _, Sha256};

use crate::error::StoreError;

/// The alphabet of a code, which is the alphabet of Crockford base32.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The bytes of a token.
const TOKEN_BYTES: usize = 32;

/// The domain of the hash of a code, so this hash is no hash of another field.
const CODE_DOMAIN: &[u8] = b"horizon-sync-invite-v1\n";

/// The domain of the hash of a token.
const TOKEN_DOMAIN: &[u8] = b"horizon-sync-token-v1\n";

/// One new code of an invitation, in three groups of four.
///
/// # Errors
/// Returns why the OS random source did not answer.
pub fn new_code() -> Result<String, StoreError> {
    let mut bytes = [0u8; CODE_CHARS];
    fill(&mut bytes)?;
    // 256 divides by 32, so no character of the alphabet is more likely than
    // another one.
    let text: String = bytes
        .iter()
        .map(|byte| char::from(CODE_ALPHABET[usize::from(*byte) % CODE_ALPHABET.len()]))
        .collect();
    normalize_code(&text).map_err(|error| random_error(&error))
}

/// The hash of `code`, so the server can look one up without holding the code.
///
/// # Errors
/// Refuses a text that is not one code, so a caller never hashes a long text.
pub fn hash_code(code: &str) -> Result<String, StoreError> {
    let code = normalize_code(code).map_err(|error| random_error(&error))?;
    Ok(hex(&digest(CODE_DOMAIN, code.as_bytes())))
}

/// One new token of a computer, as the base64url text of 32 bytes.
///
/// # Errors
/// Returns why the OS random source did not answer.
pub fn new_token() -> Result<String, StoreError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    fill(&mut bytes)?;
    Ok(base64url(&bytes))
}

/// The hash of `token`, which is what the server stores.
#[must_use]
pub fn hash_token(token: &str) -> String {
    hex(&digest(TOKEN_DOMAIN, token.as_bytes()))
}

/// Fills `bytes` from the OS random source. A 32-byte draw is the unit that the
/// cipher crate implements; a shorter tail takes the front of one more draw.
fn fill(bytes: &mut [u8]) -> Result<(), StoreError> {
    for chunk in bytes.chunks_mut(32) {
        let random: [u8; 32] =
            <[u8; 32]>::try_generate().map_err(|error| StoreError::File(format!("the random source: {error}")))?;
        chunk.copy_from_slice(&random[..chunk.len()]);
    }
    Ok(())
}

fn digest(domain: &[u8], value: &[u8]) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(value);
    hash.finalize().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// The base64url text of `bytes`, without padding.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = chunk.get(1).map_or(0, |byte| u32::from(*byte));
        let third = chunk.get(2).map_or(0, |byte| u32::from(*byte));
        let joined = (first << 16) | (second << 8) | third;
        text.push(char::from(ALPHABET[((joined >> 18) & 0x3f) as usize]));
        text.push(char::from(ALPHABET[((joined >> 12) & 0x3f) as usize]));
        if chunk.len() > 1 {
            text.push(char::from(ALPHABET[((joined >> 6) & 0x3f) as usize]));
        }
        if chunk.len() > 2 {
            text.push(char::from(ALPHABET[(joined & 0x3f) as usize]));
        }
    }
    text
}

fn random_error(error: &impl std::fmt::Display) -> StoreError {
    StoreError::File(format!("the code of an invitation: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_twelve_characters_in_three_groups() {
        let code = new_code().expect("a code");
        assert_eq!(code.len(), 14, "twelve characters and two dashes");
        assert!(horizon_core::check_code(&code), "{code}");
        let mut other = new_code().expect("a code");
        for _ in 0..8 {
            if other != code {
                break;
            }
            other = new_code().expect("a code");
        }
        assert_ne!(other, code, "the random source gives two different codes");
    }

    #[test]
    fn a_code_and_a_token_are_hashed_with_their_own_domain() {
        let code = hash_code("abcd-efgh-jkmn").expect("a code");
        assert_eq!(code.len(), 64);
        assert_eq!(
            code,
            hash_code("ABCD-EFGH-JKMN").expect("the same code"),
            "one code has one hash, however a person writes it"
        );
        assert_ne!(code, hash_code("ABCD-EFGH-JKMP").expect("another code"));
        assert!(hash_code("not a code").is_err(), "only a code is hashed");
        assert_ne!(
            code,
            hash_token("ABCD-EFGH-JKMN"),
            "a token and a code of the same text are two hashes"
        );
    }

    #[test]
    fn a_token_is_the_base64url_text_of_thirty_two_bytes() {
        let token = new_token().expect("a token");
        assert_eq!(token.len(), 43);
        assert!(horizon_core::secret_store::check_token(&token), "{token}");
        assert_ne!(new_token().expect("a token"), token);
        assert_eq!(hash_token(&token).len(), 64);
        // The text of the base64url alphabet, without a padding character.
        for byte in token.bytes() {
            assert!(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'), "{token}");
        }
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
    }
}
