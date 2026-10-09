//! Publishing worker images to `ghcr.io` as the person.
//!
//! GitHub's package registry accepts no token of the GitHub App behind Connect GitHub.
//! The first time a cloud's image goes to `ghcr.io`, Horizon therefore asks once,
//! through its own OAuth app, for `write:packages` only: a device code on the cloud
//! card, like a cloud's own sign-in. The chain stays on this computer beside Horizon's
//! Docker configuration, renews itself before a push for about six months, and logs
//! that configuration in to `ghcr.io` for the push.
use super::{Error, Event, Prompt, Result, Runner, signin};
use base64::Engine as _;
use horizon_cloud::github::{Chain, Client, Error as GitHubError, Secret};
use serde::Deserialize;
use std::{
    fs::{File, OpenOptions},
    io::{Read as _, Write as _},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

/// Horizon's own OAuth app "Horizon": public, with device sign-in on. Its tokens carry
/// scopes, which the package registry requires.
const CLIENT_ID: &str = "Ov23liPNCPgWraJDo6XM";
const SCOPE: &str = "write:packages";
const REGISTRY: &str = "ghcr.io";
/// The chain, private, in Horizon's Docker configuration directory.
const STORE: &str = "horizon-github-packages.json";
const LOCK: &str = "horizon-github-packages.lock";
/// An access token with less time left is renewed first, so an upload never outlives it.
const MARGIN: Duration = Duration::from_mins(60);
/// The largest Docker configuration file Horizon reads and rewrites.
const MAX_CONFIG: u64 = 1 << 20;

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
    let _lock = lock(docker_config, runner)?;
    let client = Client::new();
    let (login, token) = match current(docker_config, &client)? {
        Some(found) => found,
        None => sign_in(docker_config, &client, cloud_id, runner)?,
    };
    write_auth(docker_config, &login, &token)
}

/// The stored chain's account and a usable access token, renewing the chain when it
/// is close to its expiry, or `None` when the person must sign in.
fn current(docker_config: &Path, client: &Client) -> Result<Option<(String, Secret)>> {
    let Some(stored) = load(docker_config) else {
        return Ok(None);
    };
    let now = SystemTime::now();
    let token = if stored.access_expires_at > now + MARGIN {
        stored.access_token
    } else if stored.refresh_expires_at > now {
        match client.refresh(CLIENT_ID, None, &stored.refresh_token) {
            // The old chain stopped working with this answer: store the new one first.
            Ok(chain) => {
                save(docker_config, &stored.login, &chain)?;
                chain.access_token
            }
            Err(GitHubError::Transport) => return Err(unreachable()),
            Err(_) => return forget(docker_config),
        }
    } else {
        return forget(docker_config);
    };
    // The person may have revoked Horizon on GitHub since; ask GitHub who the token is.
    match client.user(&token) {
        Ok(user) => Ok(Some((user.login, token))),
        Err(GitHubError::Transport) => Err(unreachable()),
        Err(_) => forget(docker_config),
    }
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
    save(docker_config, &user.login, &chain)?;
    say(format!("Horizon may now publish images as {}.", user.login));
    (runner.emit)(Event::GitHub(Prompt::Published { allowed: true }));
    Ok((user.login, chain.access_token))
}

fn unreachable() -> Error {
    Error::Invalid("GitHub could not be reached to publish the image. Check the network and retry.")
}

/// Drops a chain that no longer works, so the next push asks again.
fn forget(docker_config: &Path) -> Result<Option<(String, Secret)>> {
    match std::fs::remove_file(docker_config.join(STORE)) {
        Ok(()) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

struct Stored {
    login: String,
    access_token: Secret,
    access_expires_at: SystemTime,
    refresh_token: Secret,
    refresh_expires_at: SystemTime,
}

fn load(docker_config: &Path) -> Option<Stored> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Fields {
        login: String,
        access_token: Secret,
        access_expires_at: u64,
        refresh_token: Secret,
        refresh_expires_at: u64,
    }
    let bytes = Zeroizing::new(read_private(&docker_config.join(STORE)).ok()??);
    let fields: Fields = serde_json::from_slice(&bytes).ok()?;
    let at = |seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds));
    (valid_login(&fields.login) && valid_token(&fields.access_token) && valid_token(&fields.refresh_token))
        .then_some(())?;
    Some(Stored {
        login: fields.login,
        access_token: fields.access_token,
        access_expires_at: at(fields.access_expires_at)?,
        refresh_token: fields.refresh_token,
        refresh_expires_at: at(fields.refresh_expires_at)?,
    })
}

fn save(docker_config: &Path, login: &str, chain: &Chain) -> Result<()> {
    let seconds = |at: SystemTime| at.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let value = Zeroizing::new(
        serde_json::json!({
            "login": login,
            "access_token": chain.access_token.expose(),
            "access_expires_at": seconds(chain.access_expires_at),
            "refresh_token": chain.refresh_token.expose(),
            "refresh_expires_at": seconds(chain.refresh_expires_at),
        })
        .to_string(),
    );
    write_private(docker_config, STORE, value.as_bytes())
}

/// Sets `ghcr.io` in `docker_config/config.json` to `login` and `token`, keeping every
/// other setting. A credential helper for `ghcr.io` would answer first, so it goes.
fn write_auth(docker_config: &Path, login: &str, token: &Secret) -> Result<()> {
    let path = docker_config.join("config.json");
    let existing = Zeroizing::new(read_private(&path)?.unwrap_or_default());
    let mut config: serde_json::Value = if existing.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&existing)
            .map_err(|_| Error::Invalid("Horizon's Docker configuration is not valid JSON"))?
    };
    let object = config
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
    let auth = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(format!("{login}:{}", token.expose())));
    let auths = object
        .entry("auths")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or(Error::Invalid("Horizon's Docker configuration is not valid JSON"))?;
    auths.insert(REGISTRY.into(), serde_json::json!({ "auth": auth.as_str() }));
    let bytes = Zeroizing::new(serde_json::to_vec_pretty(&config).map_err(|_| Error::Json)?);
    write_private(docker_config, "config.json", &bytes)
}

fn valid_login(login: &str) -> bool {
    (1..=39).contains(&login.len()) && login.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn valid_token(token: &Secret) -> bool {
    let token = token.expose();
    (1..=2048).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_graphic())
}

/// A private file's bytes, or `None` when it does not exist. A file that others could
/// read or replace is refused.
fn read_private(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid("Horizon's Docker configuration must be private"));
        }
    }
    if !metadata.is_file() || metadata.len() > MAX_CONFIG {
        return Err(Error::Invalid("Horizon's Docker configuration is not a private file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// Replaces `directory/name` with `bytes` in one rename, readable only by this user.
fn write_private(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = tempfile::Builder::new()
        .prefix(&format!(".{name}-"))
        .tempfile_in(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file().set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(directory.join(name)).map_err(|error| error.error)?;
    Ok(())
}

/// Creates `directory` readable only by this user; an existing link is refused.
fn private_directory(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    if std::fs::symlink_metadata(directory)?.file_type().is_symlink() {
        return Err(Error::Invalid("Horizon's Docker configuration must not be a link"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Holds the publishing lock of `docker_config` until dropped, waiting for another
/// cloud's push while the deployment is not cancelled.
fn lock(docker_config: &Path, runner: &Runner<'_>) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(docker_config.join(LOCK))?;
    let mut told = false;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                if !told {
                    (runner.emit)(Event::Output(
                        "GitHub: waiting for another cloud that publishes its image.".into(),
                    ));
                    told = true;
                }
                runner.cancel.check()?;
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests;
