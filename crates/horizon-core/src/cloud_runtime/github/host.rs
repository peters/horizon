//! This computer's own sign-in for the connected GitHub App. **New cloud** lists the
//! repositories the app reaches with it and clones a private one with it, so no personal
//! access token is needed. It is one more chain of the same app, signed in the way the
//! app's setting says (one Authorize click, or none in Automatic), renewed before each use
//! and kept private beside the app's secret.
use super::{
    Error, Event, Mode, Prompt, Result, Runner, Settings, signin,
    stored::{self, Kept},
};
use crate::cloud_runtime::Cancellation;
use horizon_cloud::github::{Client, Secret};
use std::path::{Path, PathBuf};

/// The name a Skip of this computer's sign-in uses, as a cloud's sign-in uses its cloud.
const SIGN_IN: &str = "horizon-this-computer";

/// Where this computer's chain for the app of `settings` is kept. A chain of another app is
/// never used for this one.
fn path(root: &Path, settings: &Settings) -> PathBuf {
    root.join("credentials")
        .join(format!("github-host-{}.json", settings.app_id))
}

/// The account and a usable access token of this computer's sign-in, renewed when close
/// to its expiry, or `None` when it must sign in first.
/// # Errors
/// GitHub out of reach or not confirming the sign-in, which is then kept, or a local file
/// that cannot be read or written.
pub fn current(root: &Path, settings: &Settings) -> Result<Option<(String, Secret)>> {
    // Only a chain from a web sign-in renews with the secret; a device chain needs none.
    let secret = match settings.mode {
        Mode::Automatic => settings.client_secret().ok(),
        Mode::Ask => None,
    };
    stored::current(
        &path(root, settings),
        &Client::new(),
        &settings.client_id,
        secret.as_ref(),
    )
    .map_err(|kept| match kept {
        Kept::Unreachable => Error::Invalid("GitHub could not be reached. Check the network and try again."),
        Kept::Unconfirmed => Error::Invalid("GitHub did not confirm this computer's sign-in. Try again."),
        Kept::Local(error) => error,
    })
}

/// Signs this computer in for the app of `settings`. `show` gets the code to approve, or
/// the page to open in Automatic; a [`skip`] or `cancel` ends it. Returns the account and
/// its token, or why GitHub ended the sign-in.
/// # Errors
/// A local file that cannot be written, or a cancelled sign-in.
pub fn sign_in(
    root: &Path,
    settings: &Settings,
    cancel: &Cancellation,
    show: &dyn Fn(Prompt),
) -> Result<std::result::Result<(String, Secret), String>> {
    let emit = |event| {
        if let Event::GitHub(prompt) = event {
            show(prompt);
        }
    };
    let runner = Runner {
        cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    let client = Client::new();
    let chain = match signin::chain(settings, SIGN_IN, &client, &runner) {
        Ok(chain) => chain,
        Err(signin::Ended::Error(error)) => return Err(error),
        Err(signin::Ended::Skipped) => return Ok(Err("Skipped.".into())),
        Err(signin::Ended::Reason(reason)) => return Ok(Err(reason)),
    };
    let user = match client.user(&chain.access_token) {
        Ok(user) => user,
        Err(error) => return Ok(Err(error.to_string())),
    };
    stored::save(
        &path(root, settings),
        &user.login,
        &chain,
        settings.mode == Mode::Automatic,
    )?;
    Ok(Ok((user.login, chain.access_token)))
}

/// Ends this computer's sign-in that waits for the person.
pub fn skip() {
    signin::skip(SIGN_IN);
}

/// Every `owner/name` the app is installed on and this computer's account reaches, in
/// lowercase and sorted.
/// # Errors
/// GitHub out of reach or refusing the token.
pub fn repositories(token: &Secret) -> std::result::Result<Vec<String>, String> {
    let mut found = Client::new()
        .installed_repositories(token)
        .map_err(|error| error.to_string())?;
    found.sort();
    found.dedup();
    Ok(found)
}

/// Forgets this computer's sign-in for the app of `settings`, as on Disconnect.
/// # Errors
/// A file that cannot be removed.
pub fn forget(root: &Path, settings: &Settings) -> Result<()> {
    stored::forget(&path(root, settings)).map(drop)
}

#[cfg(test)]
mod tests;
