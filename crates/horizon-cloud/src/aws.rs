//! AWS request signing with Signature Version 4, without the AWS SDK.
//!
//! [`sign`] builds the `Authorization` header for the EC2 Query API from the
//! HMAC-SHA256 and SHA-256 primitives that `ring` already provides. It signs a
//! request; it never sends one, never reads a credential from disk and never
//! reads the clock.
use zeroize::Zeroizing;

mod sign;
pub use sign::{Request, Signed, sign};

/// An AWS access key pair. The secret is wiped when this drops and is never
/// printed.
pub struct Credential {
    access_key_id: String,
    secret_access_key: Zeroizing<String>,
}

impl Credential {
    /// # Errors
    /// Refuses an access key ID that is not alphanumeric, and a secret that is
    /// empty or not printable ASCII.
    pub fn new(access_key_id: String, secret_access_key: String) -> Result<Self> {
        let id = !access_key_id.is_empty() && access_key_id.bytes().all(|byte| byte.is_ascii_alphanumeric());
        let secret =
            !secret_access_key.is_empty() && secret_access_key.bytes().all(|byte| (b'!'..=b'~').contains(&byte));
        if !id || !secret {
            return Err(Error::Credential);
        }
        Ok(Self {
            access_key_id,
            secret_access_key: Zeroizing::new(secret_access_key),
        })
    }

    pub(crate) fn access_key_id(&self) -> &str {
        &self.access_key_id
    }

    pub(crate) fn secret_access_key(&self) -> &str {
        &self.secret_access_key
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Credential([REDACTED])")
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Use an alphanumeric AWS access key ID with a printable ASCII secret access key.")]
    Credential,
    #[error("Sign an absolute URL that has a host, with every percent escape written as two hex digits.")]
    Url,
    #[error("Sign for an AWS region and service name, such as eu-north-1 and ec2.")]
    Scope,
    #[error(
        "Sign a request with an uppercase HTTP method and headers that are well formed, not repeated and not set by SigV4."
    )]
    Request,
}
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::{Credential, Error};

    const ACCESS_KEY: &str = "AKIDEXAMPLE";
    const SECRET_KEY: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    #[test]
    fn a_credential_needs_an_alphanumeric_id_and_a_printable_secret() {
        assert_eq!(
            Credential::new(String::new(), SECRET_KEY.into()).err(),
            Some(Error::Credential)
        );
        assert_eq!(
            Credential::new(ACCESS_KEY.into(), String::new()).err(),
            Some(Error::Credential)
        );
        assert_eq!(
            Credential::new("AKID EXAMPLE".into(), SECRET_KEY.into()).err(),
            Some(Error::Credential)
        );
        assert_eq!(
            Credential::new(ACCESS_KEY.into(), "line\nbreak".into()).err(),
            Some(Error::Credential)
        );
        assert!(Credential::new(ACCESS_KEY.into(), SECRET_KEY.into()).is_ok());
    }

    #[test]
    fn no_print_of_a_credential_or_its_error_reveals_the_secret() {
        let prints = [
            format!("{:?}", Credential::new(ACCESS_KEY.into(), SECRET_KEY.into()).unwrap()),
            format!("{:?}", Error::Credential),
            Error::Credential.to_string(),
            format!(
                "{:?}",
                Credential::new(ACCESS_KEY.into(), "unprintable\nsecret".into()).unwrap_err()
            ),
        ];
        for print in prints {
            assert!(!print.contains(SECRET_KEY), "{print}");
            assert!(!print.contains("unprintable"), "{print}");
            assert!(!print.contains("wJalrXUtnFEMI"), "{print}");
        }
    }
}
