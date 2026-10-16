use super::super::{Error, Result};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const MAX_SECRET_BYTES: usize = 4096;
const MAX_CLIENT_ID_BYTES: usize = 256;
const MAX_STORED_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApiCredentialKind {
    PersonalToken,
    #[serde(rename = "oauth")]
    OAuth,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", content = "kind", rename_all = "snake_case")]
pub enum ApiCredentialStatus {
    Missing,
    Stored(ApiCredentialKind),
}

/// A write-only credential draft. Saved secret values are never returned to callers.
pub struct ApiCredential(Secret);

enum Secret {
    PersonalToken {
        token: Zeroizing<String>,
    },
    OAuth {
        client_id: Zeroizing<String>,
        client_secret: Zeroizing<String>,
    },
}

// A plain struct deserializes fields directly; an internally tagged enum buffers
// secret strings before field adapters can put them in zeroizing storage.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    #[serde(deserialize_with = "secret_string")]
    kind: Zeroizing<String>,
    #[serde(default, deserialize_with = "optional_secret")]
    token: Option<Zeroizing<String>>,
    #[serde(default, deserialize_with = "optional_secret")]
    client_id: Option<Zeroizing<String>>,
    #[serde(default, deserialize_with = "optional_secret")]
    client_secret: Option<Zeroizing<String>>,
}

fn optional_secret<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Zeroizing<String>>, D::Error> {
    secret_string(deserializer).map(Some)
}

fn secret_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Zeroizing<String>, D::Error> {
    String::deserialize(deserializer).map(Zeroizing::new)
}

fn opaque(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && value.bytes().all(|b| b.is_ascii_graphic())
}

fn valid_token(token: &str) -> bool {
    token
        .strip_prefix("tskey-api-")
        .is_some_and(|suffix| !suffix.is_empty())
        && opaque(token, MAX_SECRET_BYTES)
}

impl ApiCredential {
    /// # Errors
    /// Requires a bounded personal Tailscale API token, not an enrollment auth key.
    pub fn personal_token(token: &str) -> Result<Self> {
        let token = token.trim();
        if !valid_token(token) {
            return Err(Error::InvalidApiCredential);
        }
        Ok(Self(Secret::PersonalToken {
            token: Zeroizing::new(token.into()),
        }))
    }

    /// # Errors
    /// Requires a bounded client ID and client secret without whitespace or controls.
    pub fn oauth(client_id: &str, client_secret: &str) -> Result<Self> {
        let client_id = client_id.trim();
        let client_secret = client_secret.trim();
        if !opaque(client_id, MAX_CLIENT_ID_BYTES) || !opaque(client_secret, MAX_SECRET_BYTES) {
            return Err(Error::InvalidApiCredential);
        }
        Ok(Self(Secret::OAuth {
            client_id: Zeroizing::new(client_id.into()),
            client_secret: Zeroizing::new(client_secret.into()),
        }))
    }

    pub(super) fn kind(&self) -> ApiCredentialKind {
        match &self.0 {
            Secret::PersonalToken { .. } => ApiCredentialKind::PersonalToken,
            Secret::OAuth { .. } => ApiCredentialKind::OAuth,
        }
    }

    fn validate(&self) -> Result<()> {
        let valid = match &self.0 {
            Secret::PersonalToken { token } => valid_token(token),
            Secret::OAuth {
                client_id,
                client_secret,
            } => opaque(client_id, MAX_CLIENT_ID_BYTES) && opaque(client_secret, MAX_SECRET_BYTES),
        };
        if valid {
            Ok(())
        } else {
            Err(Error::InvalidApiCredential)
        }
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_STORED_BYTES {
            return Err(Error::InvalidApiCredential);
        }
        let stored: Stored = serde_json::from_slice(bytes).map_err(|_| Error::InvalidApiCredential)?;
        let secret = match (
            stored.kind.as_str(),
            stored.token,
            stored.client_id,
            stored.client_secret,
        ) {
            ("personal_token", Some(token), None, None) => Secret::PersonalToken { token },
            ("oauth", None, Some(client_id), Some(client_secret)) => Secret::OAuth {
                client_id,
                client_secret,
            },
            _ => return Err(Error::InvalidApiCredential),
        };
        let credential = Self(secret);
        credential.validate()?;
        Ok(credential)
    }

    pub(super) fn encode(&self) -> Result<Zeroizing<Vec<u8>>> {
        #[derive(Serialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Payload<'a> {
            PersonalToken {
                token: &'a str,
            },
            #[serde(rename = "oauth")]
            OAuth {
                client_id: &'a str,
                client_secret: &'a str,
            },
        }
        let payload = match &self.0 {
            Secret::PersonalToken { token } => Payload::PersonalToken { token },
            Secret::OAuth {
                client_id,
                client_secret,
            } => Payload::OAuth {
                client_id,
                client_secret,
            },
        };
        serde_json::to_vec(&payload)
            .map(Zeroizing::new)
            .map_err(|_| Error::InvalidApiCredential)
    }
}

pub(super) enum HttpMaterial<'a> {
    Personal(&'a str),
    OAuth { client_id: &'a str, client_secret: &'a str },
}
impl ApiCredential {
    pub(super) fn http_material(&self) -> HttpMaterial<'_> {
        match &self.0 {
            Secret::PersonalToken { token } => HttpMaterial::Personal(token),
            Secret::OAuth {
                client_id,
                client_secret,
            } => HttpMaterial::OAuth {
                client_id,
                client_secret,
            },
        }
    }
}
