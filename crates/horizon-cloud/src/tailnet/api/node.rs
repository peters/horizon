//! Desktop-only exact-device HTTP client; the core caller supplies locked capture authority.
mod http;
use super::{ApiCredential, Credentials, Native, Store, credential::HttpMaterial};
use http::{HttpTransport, Request, Transport};
use serde::Deserialize;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetentionReason {
    MissingCredential,
    UnavailableCredential,
    Authorization,
    NotFoundUnverified,
    Transport,
    UnexpectedResponse,
}
impl RetentionReason {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::MissingCredential => "the saved tailnet has no desktop API credential",
            Self::UnavailableCredential => "the saved desktop API credential is unavailable",
            Self::Authorization => "API access was denied; OAuth needs devices:core and tag:horizon-worker",
            Self::NotFoundUnverified => "the API did not find this node; its removal is unconfirmed",
            Self::Transport => "the API is unavailable, rate-limited or its request timed out",
            Self::UnexpectedResponse => "the API response could not confirm removal",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeDeletion {
    ConfirmedDeleted,
    Retained(RetentionReason),
}

/// Exact-device transport for trusted desktop controllers, not an owner/agent grant.
/// Never invoke on the UI thread; credentials remain inside the client.
pub struct NodeApi {
    store: Store,
    transport: HttpTransport,
}
impl NodeApi {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            store,
            transport: HttpTransport::new(),
        }
    }
    /// Delete only an ID from the caller's locked, validated capture record.
    /// The caller must already have confirmed its compute and storage are absent.
    /// No names or inventory inference occurs. A 404 remains unconfirmed.
    #[must_use]
    pub fn delete_recorded(&self, tailnet: &str, device: &str, budget: Duration) -> NodeDeletion {
        delete(&self.store, tailnet, device, budget, &Native, &self.transport)
    }
}

fn credential(store: &Store, id: &str, keys: &impl Credentials) -> Result<ApiCredential, RetentionReason> {
    super::resolved(store, id, keys)
        .map_err(|error| match error {
            super::Error::Missing => RetentionReason::MissingCredential,
            _ => RetentionReason::UnavailableCredential,
        })?
        .ok_or(RetentionReason::MissingCredential)
}

fn delete(
    store: &Store,
    tailnet: &str,
    device: &str,
    budget: Duration,
    keys: &impl Credentials,
    transport: &impl Transport,
) -> NodeDeletion {
    let result = (|| {
        http::device_url(device)?;
        let deadline = Instant::now() + budget.min(OPERATION_TIMEOUT);
        let _lock = store.lock().map_err(|_| RetentionReason::UnavailableCredential)?;
        let credential = credential(store, tailnet, keys)?;
        let bearer = match credential.http_material() {
            HttpMaterial::Personal(token) => Zeroizing::new(token.to_owned()),
            HttpMaterial::OAuth {
                client_id,
                client_secret,
            } => oauth(transport, client_id, client_secret, deadline)?,
        };
        if Instant::now() >= deadline {
            return Err(RetentionReason::Transport);
        }
        let response = transport.send(
            Request::Delete {
                device_id: device,
                bearer: &bearer,
            },
            deadline,
        )?;
        match response.status {
            200 | 204 => Ok(()),
            401 | 403 => Err(RetentionReason::Authorization),
            404 => Err(RetentionReason::NotFoundUnverified),
            408 | 429 | 503 => Err(RetentionReason::Transport),
            _ => Err(RetentionReason::UnexpectedResponse),
        }
    })();
    result.map_or_else(NodeDeletion::Retained, |()| NodeDeletion::ConfirmedDeleted)
}

#[derive(Deserialize)]
struct OAuthResponse {
    #[serde(deserialize_with = "secret")]
    access_token: Zeroizing<String>,
    token_type: String,
    expires_in: u64,
}
fn secret<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Zeroizing<String>, D::Error> {
    String::deserialize(decoder).map(Zeroizing::new)
}
fn oauth(
    transport: &impl Transport,
    client_id: &str,
    client_secret: &str,
    deadline: Instant,
) -> Result<Zeroizing<String>, RetentionReason> {
    let response = transport.send(
        Request::OAuth {
            client_id,
            client_secret,
        },
        deadline,
    )?;
    match response.status {
        200 => {}
        401 | 403 => return Err(RetentionReason::Authorization),
        408 | 429 | 503 => return Err(RetentionReason::Transport),
        _ => return Err(RetentionReason::UnexpectedResponse),
    }
    let token: OAuthResponse =
        serde_json::from_slice(&response.body).map_err(|_| RetentionReason::UnexpectedResponse)?;
    if !token.token_type.eq_ignore_ascii_case("bearer")
        || token.expires_in == 0
        || token.access_token.is_empty()
        || token.access_token.len() > 4096
        || !token.access_token.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(RetentionReason::UnexpectedResponse);
    }
    Ok(token.access_token)
}

#[cfg(test)]
mod tests;
