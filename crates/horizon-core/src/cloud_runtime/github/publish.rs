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

pub mod publication;

/// Horizon's own OAuth app "Horizon": public, with device sign-in on. Its tokens carry
/// scopes, which the package registry requires.
pub(super) const CLIENT_ID: &str = "Ov23liPNCPgWraJDo6XM";

/// The GitHub page where the person sees Horizon's OAuth app and grants it to an
/// organization that limits OAuth apps.
pub const APP_PAGE: &str = "https://github.com/settings/connections/applications/Ov23liPNCPgWraJDo6XM";
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
    pub prepared: Option<&'a publication::Prepared>,
}

impl Publisher<'_> {
    /// [`login`] for this publisher.
    /// # Errors
    /// As [`login`].
    pub fn login(&self, image: &str, runner: &Runner<'_>) -> Result<()> {
        match self.prepared {
            Some(prepared) => prepared.renew(self.docker_config, self.cloud_id, runner),
            None => login(self.docker_config, image, self.cloud_id, runner),
        }
    }
}

/// Whether `image` goes to `ghcr.io`.
#[must_use]
pub fn publishes_to_ghcr(image: &str) -> bool {
    image
        .split_once('/')
        .is_some_and(|(host, _)| host.eq_ignore_ascii_case(REGISTRY))
}

/// The GitHub account Horizon publishes `ghcr.io` images as, read from the chain kept
/// in `docker_config` without asking GitHub, or `None` when the card asks first. A
/// chain revoked on GitHub still names its account until the next push finds out.
#[must_use]
pub fn publisher(docker_config: &Path) -> Option<String> {
    stored::load(&docker_config.join(STORE))
        .filter(|chain| chain.refresh_expires_at > std::time::SystemTime::now())
        .map(|chain| chain.login)
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

/// A throwaway Docker configuration for building on this computer: only Horizon's own
/// `ghcr.io` login, renewed without asking when it is close to expiry, so private
/// `ghcr.io` bases pull without the machine's credential helpers. Without a stored login
/// it holds none, and public images pull anonymously. No registry login or credential
/// helper of the machine's carries over. Its Docker context and CLI plugins do, so
/// `buildx` and the daemon are the ones Docker normally uses; a remote context brings
/// its own connection settings and TLS files with it, as Docker reads them anyway.
/// Returns the directory, deleted on drop, and whether it carries the login.
/// # Errors
/// The directory or its file cannot be written.
pub fn build_config(docker_config: &Path) -> Result<(tempfile::TempDir, bool)> {
    let machine = std::env::var_os("DOCKER_CONFIG")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".docker")));
    build_config_with(docker_config, machine.as_deref())
}

/// A private Docker configuration retaining only the machine's daemon context and plugins.
/// It never reads a stored publication chain or copies registry logins and helpers.
/// # Errors
/// The directory or its file cannot be written.
pub fn empty_build_config() -> Result<tempfile::TempDir> {
    let empty = private_tempdir("horizon-empty-docker-")?;
    build_config(empty.path()).map(|(config, _)| config)
}

/// The settings of the machine's Docker configuration that hold no credentials; proxy
/// URLs can, so `proxies` stays out. The context it names resolves only where its
/// folders are linked in, so only there.
#[cfg(unix)]
const MACHINE_SETTINGS: &[&str] = &["currentContext", "cliPluginsExtraDirs"];
#[cfg(not(unix))]
const MACHINE_SETTINGS: &[&str] = &["cliPluginsExtraDirs"];

fn build_config_with(docker_config: &Path, machine: Option<&Path>) -> Result<(tempfile::TempDir, bool)> {
    let directory = private_tempdir("horizon-build-docker-")?;
    let login = if docker_config.join(STORE).is_file() {
        // The lock is held while the login renews; a renewal that fails says so instead of
        // turning into an anonymous pull that a private base then refuses.
        let client = Client::new();
        let _lock = stored::lock(&docker_config.join(LOCK), || Ok(()))?;
        current(docker_config, &client)?
    } else {
        None
    };
    let signed_in = login.is_some();
    let mut config = match login {
        Some((login, token)) => {
            let plain = Zeroizing::new(format!("{login}:{}", token.expose()));
            let auth = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(plain.as_bytes()));
            Config(serde_json::json!({ "auths": { REGISTRY: { "auth": auth.as_str() } } }))
        }
        // Not empty: with no entry at all, Docker on Linux may pick a credential helper it
        // finds, such as pass or secretservice, and send a login saved there.
        None => Config(serde_json::json!({ "auths": { "horizon.invalid": {} } })),
    };
    if let Some(machine) = machine {
        let machine = std::path::absolute(machine)?;
        // Contexts (with any TLS files a remote context keeps) and plugins: what Docker
        // needs to reach its daemon. Never registry logins or credential helpers.
        let settings = std::fs::read(machine.join("config.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        if let (Some(settings), Some(target)) = (settings, config.0.as_object_mut()) {
            for &key in MACHINE_SETTINGS {
                if let Some(value) = settings.get(key) {
                    target.insert(key.to_owned(), value.clone());
                }
            }
        }
        #[cfg(unix)]
        for folder in ["contexts", "cli-plugins"] {
            if machine.join(folder).is_dir() {
                let _ = std::os::unix::fs::symlink(machine.join(folder), directory.path().join(folder));
            }
        }
    }
    let bytes = stored::serialized(|writer| serde_json::to_writer(writer, &config.0))?;
    write_private(directory.path(), "config.json", &bytes)?;
    Ok((directory, signed_in))
}

fn private_tempdir(prefix: &str) -> Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    Ok(builder.tempdir()?)
}

/// The organization that keeps Horizon's own login from pulling a `ghcr.io` image named
/// in `text`, a build's output, because it limits OAuth apps, as GitHub's packages API
/// says. `None` without a stored login, without such an image, when GitHub cannot be
/// asked, or once `cancelled` says so; it is asked before each request.
#[must_use]
pub fn restricting_organization(docker_config: &Path, text: &str, cancelled: &dyn Fn() -> bool) -> Option<String> {
    let packages = ghcr_packages(text);
    if packages.is_empty() || !docker_config.join(STORE).is_file() || cancelled() {
        return None;
    }
    let client = Client::new();
    // Another cloud may hold the login while it publishes or signs in: a stop ends the
    // wait, and nothing is renewed after it.
    let (_, token) = stored::lock(&docker_config.join(LOCK), || {
        if cancelled() {
            Err(std::io::Error::other("the proof stopped").into())
        } else {
            Ok(())
        }
    })
    .ok()
    .filter(|_| !cancelled())
    .and_then(|_lock| current(docker_config, &client).ok().flatten())?;
    // The first answer that is neither yes nor no (an outage, a rate limit) ends the
    // probing: asking the next image would only wait out another timeout.
    for (owner, name) in packages {
        if cancelled() {
            return None;
        }
        match client.oauth_restricted(&token, &owner, &name) {
            Ok(true) => return Some(owner),
            Ok(false) => {}
            Err(_) => return None,
        }
    }
    None
}

/// The most images one diagnosis asks GitHub about.
const MAX_PACKAGES: usize = 8;

/// The owner and name of each distinct `ghcr.io` image named in `text`, without its tag
/// or digest; at most [`MAX_PACKAGES`], the first ones named.
fn ghcr_packages(text: &str) -> Vec<(String, String)> {
    let mut packages = Vec::new();
    for (index, _) in text.match_indices("ghcr.io/") {
        if packages.len() == MAX_PACKAGES {
            break;
        }
        // A registry API address such as https://ghcr.io/v2/OWNER/NAME/manifests/TAG
        // names no image in that form; the image reference beside it does.
        if text[..index].ends_with("://") {
            continue;
        }
        let reference: String = text[index + "ghcr.io/".len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || "._-/:@".contains(*c))
            .collect();
        let path = reference.split('@').next().unwrap_or_default();
        // A tag follows the first colon after the last slash.
        let last = path.rfind('/').map_or(0, |slash| slash + 1);
        let path = path[last..].find(':').map_or(path, |colon| &path[..last + colon]);
        // A sentence may end right after the image: its full stop is no part of the name.
        let path = path.trim_end_matches(['/', '.', '_', '-']);
        let Some((owner, name)) = path.split_once('/') else {
            continue;
        };
        let package = (owner.to_ascii_lowercase(), name.to_ascii_lowercase());
        if !name.is_empty() && !packages.contains(&package) {
            packages.push(package);
        }
    }
    packages
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
    sign_in_user(docker_config, client, cloud_id, runner).map(|(user, token)| (user.login, token))
}

fn sign_in_user(
    docker_config: &Path,
    client: &Client,
    cloud_id: &str,
    runner: &Runner<'_>,
) -> Result<(horizon_cloud::github::User, Secret)> {
    sign_in_as(docker_config, client, cloud_id, None, runner)
}

fn sign_in_as(
    docker_config: &Path,
    client: &Client,
    cloud_id: &str,
    expected: Option<&publication::Account>,
    runner: &Runner<'_>,
) -> Result<(horizon_cloud::github::User, Secret)> {
    let result = sign_in_as_inner(docker_config, client, cloud_id, expected, runner);
    (runner.emit)(Event::GitHub(Prompt::Published {
        allowed: result.is_ok(),
    }));
    result
}

fn sign_in_as_inner(
    docker_config: &Path,
    client: &Client,
    cloud_id: &str,
    expected: Option<&publication::Account>,
    runner: &Runner<'_>,
) -> Result<(horizon_cloud::github::User, Secret)> {
    runner.cancel.check()?;
    let say = |text: String| (runner.emit)(Event::Output(format!("GitHub: {text}")));
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
            return Err(error);
        }
        Err(signin::Ended::Skipped) => {
            return Err(Error::Invalid(SKIPPED));
        }
        Err(signin::Ended::Reason(reason)) => {
            say(reason);
            return Err(Error::Invalid(REFUSED));
        }
    };
    runner.cancel.check()?;
    let user = match client.user(&chain.access_token) {
        Ok(user) => user,
        Err(error) => {
            say(error.to_string());
            return Err(Error::Invalid(REFUSED));
        }
    };
    runner.cancel.check()?;
    if let Some(expected) = expected {
        expected.check(&publication::Account {
            id: user.id,
            login: user.login.clone(),
        })?;
    }
    stored::save(&docker_config.join(STORE), &user.login, &chain, false)?;
    say(format!("Horizon may now publish images as {}.", user.login));
    Ok((user, chain.access_token))
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
