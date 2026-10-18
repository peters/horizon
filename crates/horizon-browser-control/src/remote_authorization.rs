//! One provider authorization header from bound credential values.
//!
//! Each host keeps its own credential stores. It checks every bound value with
//! [`header_safe_value`] and gives the checked values to [`authorization_header`],
//! so every host forms the same header and refuses the same values. Every
//! intermediate buffer is a `zeroize` type that is wiped when dropped, on
//! success and on every error path alike. Nothing here logs or serializes a value.

use base64::Engine as _;
use horizon_browser::remote::{CredentialReference, RemoteAuthentication};
use zeroize::Zeroizing;

/// Largest accepted secret. Provider keys are far smaller; the bound stops a
/// pasted file from becoming a header.
pub const MAX_SECRET_BYTES: usize = 16 * 1024;

/// Why a bound value cannot become part of an authorization header. Never
/// carries the value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialValueError {
    #[error("credential value is empty or larger than {MAX_SECRET_BYTES} bytes")]
    InvalidValue,
    #[error("credential value is not printable ASCII without control characters")]
    NotHeaderSafe,
    #[error("a Basic authentication username must not contain a colon")]
    InvalidUsername,
    #[error("a Bearer token may only contain letters, digits, - . _ ~ + / and trailing =")]
    InvalidBearerToken,
}

/// # Errors
/// [`CredentialValueError::InvalidValue`] for an empty value or one larger
/// than [`MAX_SECRET_BYTES`].
pub fn validate_secret(bytes: &[u8]) -> Result<(), CredentialValueError> {
    if bytes.is_empty() || bytes.len() > MAX_SECRET_BYTES {
        return Err(CredentialValueError::InvalidValue);
    }
    Ok(())
}

/// Copy one bound value into a zeroizing buffer when it can be part of a header.
///
/// # Errors
/// The value is empty, too large, or not printable ASCII.
pub fn header_safe_value(bytes: &[u8]) -> Result<Zeroizing<String>, CredentialValueError> {
    validate_secret(bytes)?;
    if !bytes.iter().all(|byte| byte.is_ascii_graphic() || *byte == b' ') {
        return Err(CredentialValueError::NotHeaderSafe);
    }
    Ok(Zeroizing::new(String::from_utf8_lossy(bytes).into_owned()))
}

/// Build the authorization header value, or `None` for `kind: none`.
///
/// `fetch` returns the checked value of one reference. `invalid` turns a value
/// that does not fit its scheme into the caller's error for that reference.
///
/// # Errors
/// The first error of `fetch`, or of `invalid` for a value that does not fit.
pub fn authorization_header<E>(
    authentication: &RemoteAuthentication,
    mut fetch: impl FnMut(&CredentialReference) -> Result<Zeroizing<String>, E>,
    invalid: impl Fn(&CredentialReference, CredentialValueError) -> E,
) -> Result<Option<Zeroizing<String>>, E> {
    match authentication {
        RemoteAuthentication::None {} => Ok(None),
        RemoteAuthentication::Basic {
            username_ref,
            password_ref,
        } => {
            let username = fetch(username_ref)?;
            if username.contains(':') {
                return Err(invalid(username_ref, CredentialValueError::InvalidUsername));
            }
            let password = fetch(password_ref)?;
            let joined = Zeroizing::new(format!("{}:{}", username.as_str(), password.as_str()));
            let encoded = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(joined.as_bytes()));
            Ok(Some(Zeroizing::new(format!("Basic {}", encoded.as_str()))))
        }
        RemoteAuthentication::Bearer { token_ref } => {
            let token = fetch(token_ref)?;
            if !is_token68(&token) {
                return Err(invalid(token_ref, CredentialValueError::InvalidBearerToken));
            }
            Ok(Some(Zeroizing::new(format!("Bearer {}", token.as_str()))))
        }
    }
}

/// RFC 7235 `token68`: the only shape a Bearer credential may take.
fn is_token68(token: &str) -> bool {
    let trimmed = token.trim_end_matches('=');
    !trimmed.is_empty()
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'+' | b'/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic() -> RemoteAuthentication {
        RemoteAuthentication::Basic {
            username_ref: CredentialReference::from("user"),
            password_ref: CredentialReference::from("key"),
        }
    }

    fn values<'a>(
        user: &'a str,
        key: &'a str,
    ) -> impl FnMut(&CredentialReference) -> Result<Zeroizing<String>, String> + 'a {
        move |reference| {
            let value = if reference.as_str() == "user" { user } else { key };
            header_safe_value(value.as_bytes()).map_err(|error| error.to_string())
        }
    }

    fn named(reference: &CredentialReference, error: CredentialValueError) -> String {
        format!("{}: {error}", reference.as_str())
    }

    #[test]
    fn basic_and_bearer_headers_match_their_schemes() {
        let header = authorization_header(&basic(), values("alice", "s3cret-key"), named);
        assert_eq!(
            header.map(|header| header.map(|value| value.as_str().to_owned())),
            Ok(Some("Basic YWxpY2U6czNjcmV0LWtleQ==".to_owned()))
        );
        let bearer = RemoteAuthentication::Bearer {
            token_ref: CredentialReference::from("key"),
        };
        let header = authorization_header(&bearer, values("", "abc.DEF-1~+/=="), named);
        assert_eq!(
            header.map(|header| header.map(|value| value.as_str().to_owned())),
            Ok(Some("Bearer abc.DEF-1~+/==".to_owned()))
        );
        assert_eq!(
            authorization_header(&RemoteAuthentication::None {}, values("", ""), named),
            Ok(None)
        );
    }

    #[test]
    fn values_that_do_not_fit_name_only_the_reference() {
        assert_eq!(
            authorization_header(&basic(), values("al:ice", "key"), named),
            Err("user: a Basic authentication username must not contain a colon".to_owned())
        );
        let bearer = RemoteAuthentication::Bearer {
            token_ref: CredentialReference::from("key"),
        };
        let refused = authorization_header(&bearer, values("", "not a token"), named).unwrap_err();
        assert!(refused.starts_with("key: "), "{refused}");
        assert!(!refused.contains("not a token"), "{refused}");
        assert_eq!(header_safe_value(b"").unwrap_err(), CredentialValueError::InvalidValue);
        assert_eq!(
            header_safe_value(&vec![b'a'; MAX_SECRET_BYTES + 1]).unwrap_err(),
            CredentialValueError::InvalidValue
        );
        assert_eq!(
            header_safe_value(b"line\n").unwrap_err(),
            CredentialValueError::NotHeaderSafe
        );
    }
}
