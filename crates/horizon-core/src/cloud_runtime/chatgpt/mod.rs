//! Sign in with `ChatGPT`: this machine's connection to the user's `ChatGPT` account.
//!
//! The flow registers a user-defined agent for this host, identified by a stable
//! `ext_agent_host_id`. The user signs in in the system browser and may grant
//! `chatgpt.tokens.use.direct`, which lets eligible requests use their `ChatGPT` plan.
//! Credential records live in private files under the cloud root; they never enter
//! the settings file or deployment state.
mod flow;
mod id_token;
mod response;
mod store;

use super::Cancellation;
use std::{path::Path, sync::mpsc::Receiver};
pub use store::Connection;

/// `OpenID` discovery for the `ChatGPT` sign-in service; both the ID-token JWKS and the
/// revocation endpoint come from it.
const CONFIG_URL: &str = "https://auth.openai.com/.well-known/openid-configuration";

/// Errors the Sign in with `ChatGPT` flow reports.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("ChatGPT sign-in I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Malformed response from the ChatGPT sign-in service")]
    Malformed,
    #[error("The sign-in was declined or cancelled")]
    Declined,
    #[error("ChatGPT sign-in failed: {0}")]
    Provider(String),
    #[error("The account ID token did not validate")]
    IdToken,
    #[error("No ChatGPT connection is saved")]
    Missing,
}
pub type Result<T> = std::result::Result<T, Error>;

/// The host ID this installation sends as `ext_agent_host_id`, created on first use.
/// # Errors
/// The host ID file could not be read or created.
pub fn host_id(root: &Path) -> Result<String> {
    store::host_id(root)
}

/// Every saved `ChatGPT` account registration, newest first.
/// # Errors
/// The connection files could not be read.
pub fn connections(root: &Path) -> Result<Vec<Connection>> {
    store::connections(root)
}

/// The token-free connection the settings card shows: the active registration, else the newest saved one.
/// # Errors
/// The connection files could not be read.
pub fn status(root: &Path) -> Result<Option<Connection>> {
    store::default_registration(root).map(|record| record.map(Connection::from))
}

/// Starts sign-in in the system browser and opens its loopback callback. The receiver
/// gets the saved connection when the flow finishes.
/// # Errors
/// The loopback listener could not be opened, or the browser could not be opened.
pub fn start(
    root: &Path,
    open: fn(&str) -> std::io::Result<()>,
    cancel: Cancellation,
) -> Result<Receiver<Result<Connection>>> {
    flow::start(root, open, cancel)
}

/// Marks the first-sign-in plan-usage confirmation as shown for one registration.
/// # Errors
/// The registration could not be read or written.
pub fn confirm_usage(root: &Path, client_id: &str) -> Result<()> {
    store::confirm_usage(root, client_id)
}

/// Revokes the renewable session, then clears its tokens locally.
/// Returns whether the remote revocation was confirmed.
/// # Errors
/// The registration could not be read or written.
pub fn sign_out(root: &Path, client_id: &str) -> Result<Option<bool>> {
    flow::sign_out(root, client_id)
}

/// Obtains a replacement token set for one registration, storing the rotating refresh
/// token with the new access token.
/// # Errors
/// The registration has no refresh token, or the token endpoint refused the refresh.
pub fn refresh(root: &Path, client_id: &str) -> Result<()> {
    flow::refresh(root, client_id)
}
