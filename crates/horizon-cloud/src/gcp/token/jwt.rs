//! The signed assertion: a fixed `RS256` header, the claims Google expects, and the
//! RSA signature over both.

use super::{Error, Result, ServiceAccount};
use base64::Engine as _;
use serde::Serialize;

/// The header of every assertion. It is fixed, so no key identifier and no other
/// field can creep in.
pub(super) const HEADER: &str = r#"{"alg":"RS256","typ":"JWT"}"#;

/// The only scope Horizon needs: Cloud Platform, which covers Compute Engine.
pub(super) const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// The lifetime of an assertion, in seconds. Google refuses more than an hour.
pub(super) const LIFETIME: i64 = 3600;

/// The claims of an assertion, in the order they are sent.
#[derive(Serialize)]
pub(super) struct Claims<'a> {
    pub(super) iss: &'a str,
    pub(super) scope: &'a str,
    pub(super) aud: &'a str,
    pub(super) iat: i64,
    pub(super) exp: i64,
}

impl<'a> Claims<'a> {
    pub(super) fn new(account: &'a ServiceAccount, issued: i64) -> Self {
        Self {
            iss: account.client_email(),
            scope: SCOPE,
            aud: account.token_uri(),
            iat: issued,
            exp: issued + LIFETIME,
        }
    }
}

/// The assertion: `base64url(header) + "." + base64url(claims)` signed with `RS256`.
pub(super) fn assertion(account: &ServiceAccount, issued: i64) -> Result<String> {
    let unsigned = unsigned(account, issued)?;
    let signature = account.private_key().sign(&unsigned)?;
    Ok(format!("{unsigned}.{signature}"))
}

/// The signed part of the assertion, before its signature.
pub(super) fn unsigned(account: &ServiceAccount, issued: i64) -> Result<String> {
    let claims = serde_json::to_string(&Claims::new(account, issued)).map_err(|_| Error::InvalidResponse)?;
    Ok(format!("{}.{}", encode(HEADER), encode(&claims)))
}

fn encode(text: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text)
}
