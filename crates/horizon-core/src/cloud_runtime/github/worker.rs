//! The worker's root GitHub service: its status, and the chain Horizon gives it.
//! The payload travels on SSH stdin from memory and never enters argv, output or a named file.
use super::{Error, Grant, Result, Runner, Settings, installation::Installation};
use crate::cloud_runtime::ssh::Connection;
use horizon_cloud::github::{Chain, Secret, User};
use serde::{Deserialize, Serialize, Serializer};
use std::time::Duration;

const STATUS: &str = "if command -v horizon-worker-github >/dev/null 2>&1; then horizon-worker-github status; \
     else printf '{\"state\":\"unsupported\"}'; fi";

/// The chain kind of payload version 2, which carries no refresh token.
const INSTALLATION_KIND: &str = "installation";

/// What the worker holds for the cloud.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Status {
    /// The image has no GitHub service.
    Unsupported,
    /// No chain, or one GitHub no longer accepts.
    Absent,
    /// A stored chain whose service is not running, so Git and `gh` get no token.
    Unavailable,
    /// An installation token that ended. The worker holds no refresh token for it, so
    /// nothing there renews it: only this machine installs a new one.
    Expired,
    /// A chain that renews, acting as `login`, for these lowercase repositories.
    /// `requests` is whether the service takes agents' access requests.
    /// `checkouts` are the repositories granted for this cloud's checkouts; the others in
    /// `repositories` were allowed for the whole cloud on an agent's request.
    /// `renewal` is whether the worker renews the chain itself.
    Current {
        login: String,
        repositories: Vec<String>,
        checkouts: Vec<String>,
        requests: bool,
        renewal: Renewal,
    },
}

/// Who gives the worker its next token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Renewal {
    /// The worker renews its own chain from its refresh token.
    Worker,
    /// An installation token has no refresh token, so only this machine installs the next one.
    Host,
}

pub(super) fn status(connection: &Connection, runner: &Runner<'_>) -> Result<Status> {
    // The status JSON is parsed, never shown as deployment output.
    let output = runner.run_parsed(
        "GitHub access status",
        &mut connection.command(STATUS),
        Duration::from_secs(20),
    )?;
    Ok(parse_status(&output))
}

pub(super) fn parse_status(output: &str) -> Status {
    /// A held repository: an object with its `owner/name`, or the name alone.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Held {
        Grant {
            repository: String,
            #[serde(default)]
            target: Option<String>,
        },
        Name(String),
    }
    #[derive(Deserialize)]
    struct Fields {
        state: String,
        #[serde(default)]
        login: Option<String>,
        #[serde(default)]
        repositories: Vec<Held>,
        #[serde(default)]
        serving: Option<bool>,
        #[serde(default)]
        pending_requests: Option<u64>,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        renewal: Option<String>,
    }
    let Some(fields) = output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Fields>(line.trim()).ok())
    else {
        return Status::Absent;
    };
    // An installation token has no refresh token. The worker names both the chain kind and
    // who renews it, and an image from before them names neither, which is a person chain.
    let renewal = if fields.renewal.as_deref() == Some("host") || fields.kind.as_deref() == Some(INSTALLATION_KIND) {
        Renewal::Host
    } else {
        Renewal::Worker
    };
    let held: Vec<(String, bool)> = fields
        .repositories
        .into_iter()
        .map(|held| match held {
            Held::Grant { repository, target } => (repository, target.is_some()),
            Held::Name(repository) => (repository, true),
        })
        .filter(|(name, _)| horizon_cloud::github::valid_repository(name))
        .map(|(name, checkout)| (name.to_ascii_lowercase(), checkout))
        .collect();
    let repositories: Vec<String> = held.iter().map(|(name, _)| name.clone()).collect();
    let checkouts: Vec<String> = held
        .iter()
        .filter(|(_, checkout)| *checkout)
        .map(|(name, _)| name.clone())
        .collect();
    match fields.state.as_str() {
        "unsupported" => Status::Unsupported,
        // Named before a stopped service: a restart serves an ended installation token no
        // more than the stopped service did, and only this machine installs a new one.
        "installation_token_expired" => Status::Expired,
        // A service that is not running serves no chain, stored or new.
        _ if fields.serving == Some(false) => Status::Unavailable,
        "ok" if !repositories.is_empty() => Status::Current {
            login: fields.login.unwrap_or_default(),
            repositories,
            checkouts,
            requests: fields.pending_requests.is_some(),
            renewal,
        },
        _ => Status::Absent,
    }
}

/// A secret written as its value into the private payload file only.
struct Exposed<'a>(&'a Secret);

impl Serialize for Exposed<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.expose())
    }
}

#[derive(Serialize)]
struct Payload<'a> {
    version: u32,
    client_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_secret: Option<Exposed<'a>>,
    login: &'a str,
    author_name: &'a str,
    author_email: &'a str,
    grants: Vec<PayloadGrant<'a>>,
    chain: PayloadChain<'a>,
}

#[derive(Serialize)]
struct PayloadGrant<'a> {
    repository: &'a str,
    target: String,
    access: &'static str,
}

#[derive(Serialize)]
struct PayloadChain<'a> {
    access_token: Exposed<'a>,
    access_expires_at: u64,
    refresh_token: Exposed<'a>,
    refresh_expires_at: u64,
}

/// The payload of an installation chain: the same fields as version 1, and a chain with no
/// refresh token. An image from before version 2 refuses the version instead of guessing.
#[derive(Serialize)]
struct InstallationPayload<'a> {
    version: u32,
    client_id: &'a str,
    login: &'a str,
    author_name: &'a str,
    author_email: &'a str,
    grants: Vec<PayloadGrant<'a>>,
    chain: InstallationChain<'a>,
}

#[derive(Serialize)]
struct InstallationChain<'a> {
    access_token: Exposed<'a>,
    access_expires_at: u64,
    kind: &'static str,
}

fn unix(at: std::time::SystemTime) -> u64 {
    at.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The payload for `horizon-worker-github install`, cleared from memory when it drops.
pub(super) fn payload(
    settings: &Settings,
    client_secret: Option<&Secret>,
    user: &User,
    grants: &[Grant],
    chain: &Chain,
) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    let payload = Payload {
        version: 1,
        client_id: &settings.client_id,
        client_secret: client_secret.map(Exposed),
        login: &user.login,
        author_name: &user.name,
        author_email: &user.email,
        grants: grants
            .iter()
            .map(|grant| PayloadGrant {
                repository: &grant.repository,
                target: grant.target.clone().into(),
                access: "push",
            })
            .collect(),
        chain: PayloadChain {
            access_token: Exposed(&chain.access_token),
            access_expires_at: unix(chain.access_expires_at),
            refresh_token: Exposed(&chain.refresh_token),
            refresh_expires_at: unix(chain.refresh_expires_at),
        },
    };
    let mut bytes = zeroize::Zeroizing::new(Vec::with_capacity(256 * 1024));
    serde_json::to_writer(Bounded(&mut bytes), &payload).map_err(|_| Error::Json)?;
    Ok(bytes)
}

/// The payload for `horizon-worker-github install` of an installation token, cleared from
/// memory when it drops. The chain is the marker and the token alone: the worker has no
/// refresh token, and the app's key stays on this computer.
pub(super) fn installation_payload(
    installation: &Installation,
    grants: &[Grant],
) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    let payload = InstallationPayload {
        version: 2,
        client_id: &installation.client_id,
        login: &installation.bot.login,
        author_name: &installation.bot.name,
        author_email: &installation.bot.email,
        grants: grants
            .iter()
            .map(|grant| PayloadGrant {
                repository: &grant.repository,
                target: grant.target.clone().into(),
                access: "push",
            })
            .collect(),
        chain: InstallationChain {
            access_token: Exposed(&installation.token),
            access_expires_at: unix(installation.expires_at),
            kind: INSTALLATION_KIND,
        },
    };
    let mut bytes = zeroize::Zeroizing::new(Vec::with_capacity(256 * 1024));
    serde_json::to_writer(Bounded(&mut bytes), &payload).map_err(|_| Error::Json)?;
    Ok(bytes)
}

/// Writes only within the buffer's capacity, the upload bound, so the buffer never grows
/// and leaves no copy of the tokens behind in freed memory.
struct Bounded<'a>(&'a mut Vec<u8>);

impl std::io::Write for Bounded<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.0.capacity() - self.0.len() {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Removes the worker's chain and the static Git binding.
pub(super) fn clear(connection: &Connection, runner: &Runner<'_>) -> Result<()> {
    runner
        .run(
            "GitHub access removal",
            &mut connection.command("horizon-worker-github clear"),
            Duration::from_secs(20),
        )
        .map(drop)
}

pub(super) fn install(
    connection: &Connection,
    runner: &Runner<'_>,
    settings: &Settings,
    client_secret: Option<&Secret>,
    user: &User,
    grants: &[Grant],
    chain: &Chain,
) -> Result<()> {
    let payload = payload(settings, client_secret, user, grants, chain)?;
    runner.private_bytes(&mut connection.command("horizon-worker-github install"), &payload)
}

/// Gives the worker the installation token of `grants`. The token has no refresh token, so the
/// worker serves it until it ends and never renews it.
pub(super) fn install_installation(
    connection: &Connection,
    runner: &Runner<'_>,
    installation: &Installation,
    grants: &[Grant],
) -> Result<()> {
    let payload = installation_payload(installation, grants)?;
    runner.private_bytes(&mut connection.command("horizon-worker-github install"), &payload)
}
