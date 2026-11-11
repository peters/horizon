//! The token of this computer on its server: one row of the secrets file.
//!
//! A computer that joins a server with the code of an invitation gets one token
//! back, and every later link of that computer presents it. The token is a bearer
//! value of that one device, so it lives where the other secrets of this computer
//! live, in `secrets.sqlite`, under the id [`SERVER_TOKEN_ID`].
//!
//! The id is deliberately outside the ids of the protocol. `SecretId::parse`
//! refuses it, so no server can send a token in a `secret` row, and the link sends
//! only the ids that `SecretId` names. The token therefore never reaches a queue
//! file, a mirror file or a line of the log.

use zeroize::Zeroizing;

use super::SecretStore;
use crate::remote_browser_credential::RemoteCredentialError;

/// The id of the token row of the server link in the secrets file.
pub const SERVER_TOKEN_ID: &str = "server/token";

/// The most characters of a token: this Horizon writes 43, the base64url text of
/// 32 bytes.
pub const MAX_TOKEN_CHARS: usize = 64;

/// Whether `text` is one token that this Horizon reads: 1 to [`MAX_TOKEN_CHARS`]
/// characters of the base64url alphabet.
#[must_use]
pub fn check_token(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_TOKEN_CHARS
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// The token that `store` holds, or `None` when this computer is not logged in.
///
/// This reads the secrets file, so it runs on a worker thread and never on the UI
/// thread.
///
/// # Errors
/// Returns the path and the reason when the row cannot be read.
pub fn read_token(store: &SecretStore) -> Result<Option<Zeroizing<String>>, RemoteCredentialError> {
    let Some(bytes) = store.read(SERVER_TOKEN_ID)? else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| file_error(store, "the token row is not text"))?
        .trim();
    if !check_token(text) {
        return Err(file_error(store, "the token row is not one token"));
    }
    Ok(Some(Zeroizing::new(text.to_owned())))
}

/// Writes `token` as the token of this computer.
///
/// # Errors
/// Refuses a text that is not one token, and returns why the row cannot be
/// written.
pub fn write_token(store: &SecretStore, token: &str) -> Result<(), RemoteCredentialError> {
    if !check_token(token) {
        return Err(file_error(store, "the token is not one token of this Horizon"));
    }
    store.write(SERVER_TOKEN_ID, token.as_bytes())
}

/// Forgets the token of this computer. A token that is not there is already
/// forgotten.
///
/// # Errors
/// Returns why the row cannot be removed.
pub fn remove_token(store: &SecretStore) -> Result<(), RemoteCredentialError> {
    store.remove(SERVER_TOKEN_ID)
}

fn file_error(store: &SecretStore, reason: &str) -> RemoteCredentialError {
    RemoteCredentialError::File {
        path: store.path().to_path_buf(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretId;

    #[test]
    fn the_token_is_one_row_of_the_secrets_file_and_no_secret_id() {
        let store = SecretStore::in_memory().expect("a store in memory");
        assert!(read_token(&store).expect("no token").is_none());

        let token = "a".repeat(43);
        write_token(&store, &token).expect("the token");
        assert_eq!(
            read_token(&store).expect("the token").expect("one token").as_str(),
            token
        );
        assert_eq!(store.ids().expect("the ids"), vec![SERVER_TOKEN_ID.to_string()]);

        // The ids of the protocol never include the row of the token, so no
        // server sends one and no link sends one.
        assert!(SecretId::parse(SERVER_TOKEN_ID).is_err());
        assert!(
            SecretId::parse("server/token")
                .unwrap_err()
                .to_string()
                .contains("server/token")
        );

        remove_token(&store).expect("the removal");
        assert!(read_token(&store).expect("no token").is_none());
        remove_token(&store).expect("a second removal changes nothing");
    }

    #[test]
    fn a_text_that_is_not_one_token_is_refused() {
        for good in ["A".repeat(43), "a-b_c".to_owned(), "Z".repeat(MAX_TOKEN_CHARS)] {
            assert!(check_token(&good), "{good:?} is one token");
        }
        for bad in [
            String::new(),
            "Z".repeat(MAX_TOKEN_CHARS + 1),
            "a b".to_owned(),
            "token/with/slash".to_owned(),
            "tok\n".to_owned(),
            "tök".to_owned(),
        ] {
            assert!(!check_token(&bad), "{bad:?} is no token");
        }
        let store = SecretStore::in_memory().expect("a store in memory");
        assert!(write_token(&store, "a bad token").is_err());
        assert!(
            read_token(&store).expect("no token").is_none(),
            "a refused token writes nothing"
        );
    }
}
