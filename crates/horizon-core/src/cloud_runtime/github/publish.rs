//! Publishing worker images to `ghcr.io` as the person.
//!
//! GitHub's package registry accepts no token of the GitHub App behind Connect GitHub.
//! The first time a cloud's image goes to `ghcr.io`, Horizon therefore asks once,
//! through its own OAuth app, for `write:packages` only: a device code on the cloud
//! card, like a cloud's own sign-in. The chain stays on this computer beside Horizon's
//! Docker configuration, renews itself before a push for about six months, and logs
//! that configuration in to `ghcr.io` for the push.
use super::{
    Error, Event, Prompt, Result, Runner, signin,
    stored::{self, Kept, private_directory, read_private, write_private},
};
use base64::Engine as _;
use horizon_cloud::github::{Client, Secret};
use std::path::Path;
use zeroize::{Zeroize as _, Zeroizing};

/// Horizon's own OAuth app "Horizon": public, with device sign-in on. Its tokens carry
/// scopes, which the package registry requires.
const CLIENT_ID: &str = "Ov23liPNCPgWraJDo6XM";
const SCOPE: &str = "write:packages";
const REGISTRY: &str = "ghcr.io";
/// The chain, private, in Horizon's Docker configuration directory.
pub(super) const STORE: &str = "horizon-github-packages.json";
const LOCK: &str = "horizon-github-packages.lock";

const SKIPPED: &str = "This cloud's image goes to ghcr.io, and Horizon was not allowed to publish it. \
                       Retry, then click Open GitHub and Authorize on the card.";
const REFUSED: &str = "GitHub did not let Horizon publish images; the output says why. Retry to ask again.";

/// Where a push logs in as the person: Horizon's own Docker configuration, which keeps
/// the chain, and the cloud whose card shows the sign-in.
#[derive(Clone, Copy)]
pub struct Publisher<'a> {
    pub docker_config: &'a Path,
    pub cloud_id: &'a str,
}

impl Publisher<'_> {
    /// [`login`] for this publisher.
    /// # Errors
    /// As [`login`].
    pub fn login(&self, image: &str, runner: &Runner<'_>) -> Result<()> {
        login(self.docker_config, image, self.cloud_id, runner)
    }
}

/// Whether `image` goes to `ghcr.io`.
#[must_use]
pub fn publishes_to_ghcr(image: &str) -> bool {
    image
        .split_once('/')
        .is_some_and(|(host, _)| host.eq_ignore_ascii_case(REGISTRY))
}

/// Logs `docker_config` in to `ghcr.io` as the person before `image` is pushed there,
/// signing in on `cloud_id`'s card when no stored chain still works. Other registries
/// are left alone.
/// # Errors
/// A skipped or refused sign-in, GitHub out of reach, a cancelled deployment, or a
/// local file that cannot be written.
pub fn login(docker_config: &Path, image: &str, cloud_id: &str, runner: &Runner<'_>) -> Result<()> {
    if !publishes_to_ghcr(image) {
        return Ok(());
    }
    private_directory(docker_config)?;
    // Two clouds that push at once share one sign-in.
    let mut told = false;
    let _lock = stored::lock(&docker_config.join(LOCK), || {
        if !std::mem::replace(&mut told, true) {
            (runner.emit)(Event::Output(
                "GitHub: waiting for another cloud that publishes its image.".into(),
            ));
        }
        Ok(runner.cancel.check()?)
    })?;
    let client = Client::new();
    let (login, token) = match current(docker_config, &client)? {
        Some(found) => found,
        None => sign_in(docker_config, &client, cloud_id, runner)?,
    };
    write_auth(docker_config, &login, &token)
}

/// The stored chain's account and a usable access token, renewed when it is close to its
/// expiry, or `None` when the person must sign in.
fn current(docker_config: &Path, client: &Client) -> Result<Option<(String, Secret)>> {
    stored::current(&docker_config.join(STORE), client, CLIENT_ID, None).map_err(|kept| match kept {
        Kept::Unreachable => {
            Error::Invalid("GitHub could not be reached to publish the image. Check the network and retry.")
        }
        Kept::Unconfirmed => Error::Invalid(
            "GitHub did not confirm Horizon's permission to publish images. Retry; the permission is kept.",
        ),
        Kept::Local(error) => error,
    })
}

fn sign_in(docker_config: &Path, client: &Client, cloud_id: &str, runner: &Runner<'_>) -> Result<(String, Secret)> {
    let say = |text: String| (runner.emit)(Event::Output(format!("GitHub: {text}")));
    let ended = || (runner.emit)(Event::GitHub(Prompt::Published { allowed: false }));
    // A Skip from an earlier attempt does not end this one.
    signin::skipped(cloud_id);
    say("this cloud's image goes to ghcr.io; asking GitHub to let Horizon publish images for you.".into());
    let prompt = |user_code, verification_uri, expires_at| Prompt::Publish {
        user_code,
        verification_uri,
        expires_at,
    };
    let chain = match signin::device_chain(client, CLIENT_ID, Some(SCOPE), cloud_id, runner, prompt) {
        Ok(chain) => chain,
        Err(signin::Ended::Error(error)) => {
            ended();
            return Err(error);
        }
        Err(signin::Ended::Skipped) => {
            ended();
            return Err(Error::Invalid(SKIPPED));
        }
        Err(signin::Ended::Reason(reason)) => {
            say(reason);
            ended();
            return Err(Error::Invalid(REFUSED));
        }
    };
    let user = match client.user(&chain.access_token) {
        Ok(user) => user,
        Err(error) => {
            say(error.to_string());
            ended();
            return Err(Error::Invalid(REFUSED));
        }
    };
    stored::save(&docker_config.join(STORE), &user.login, &chain, false)?;
    say(format!("Horizon may now publish images as {}.", user.login));
    (runner.emit)(Event::GitHub(Prompt::Published { allowed: true }));
    Ok((user.login, chain.access_token))
}

/// Sets `ghcr.io` in `docker_config/config.json` to `login` and `token`, keeping every
/// other setting. A credential helper for `ghcr.io` would answer first, so it goes.
fn write_auth(docker_config: &Path, login: &str, token: &Secret) -> Result<()> {
    let path = docker_config.join("config.json");
    let existing = read_private(&path)?.unwrap_or_default();
    let mut config = Config(if existing.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&existing)
            .map_err(|_| Error::Invalid("Horizon's Docker configuration is not valid JSON"))?
    });
    let object = config
        .0
        .as_object_mut()
        .ok_or(Error::Invalid("Horizon's Docker configuration is not valid JSON"))?;
    if object.contains_key("credsStore") {
        return Err(Error::Invalid(
            "Horizon's Docker configuration uses a credential store, so Horizon cannot log it in to ghcr.io",
        ));
    }
    if let Some(helpers) = object.get_mut("credHelpers").and_then(serde_json::Value::as_object_mut) {
        helpers.remove(REGISTRY);
    }
    let plain = Zeroizing::new(format!("{login}:{}", token.expose()));
    let auth = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(plain.as_bytes()));
    let auths = object
        .entry("auths")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or(Error::Invalid("Horizon's Docker configuration is not valid JSON"))?;
    // The login it replaces is wiped too, not only what stays in the configuration.
    if let Some(mut replaced) = auths.insert(REGISTRY.into(), serde_json::json!({ "auth": auth.as_str() })) {
        wipe(&mut replaced);
    }
    let bytes = stored::serialized(|writer| serde_json::to_writer_pretty(writer, &config.0))?;
    write_private(docker_config, "config.json", &bytes)
}

/// A parsed Docker configuration. It holds the login of every registry, this one's and
/// others', so each of its strings is wiped when it is dropped.
struct Config(serde_json::Value);

impl Drop for Config {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// Wipes every string in `value`.
fn wipe(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => text.zeroize(),
        serde_json::Value::Array(items) => items.iter_mut().for_each(wipe),
        serde_json::Value::Object(map) => map.values_mut().for_each(wipe),
        _ => {}
    }
}

#[cfg(test)]
mod tests;
