//! The worker's root GitHub service: its status, and the chain Horizon gives it.
//! The payload travels on SSH stdin from a private file and never enters argv or output.
use super::{Error, Grant, Result, Runner, Settings};
use crate::cloud_runtime::ssh::Connection;
use horizon_cloud::github::{Chain, Secret, User};
use serde::{Deserialize, Serialize, Serializer};
use std::{io::Write as _, time::Duration};

const STATUS: &str = "if command -v horizon-worker-github >/dev/null 2>&1; then horizon-worker-github status; \
     else printf '{\"state\":\"unsupported\"}'; fi";

/// What the worker holds for the cloud.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Status {
    /// The image has no GitHub service.
    Unsupported,
    /// No chain, or one GitHub no longer accepts.
    Absent,
    /// A stored chain whose service is not running, so Git and `gh` get no token.
    Unavailable,
    /// A chain that renews, acting as `login`, for these lowercase repositories.
    /// `requests` is whether the service takes agents' access requests.
    /// `checkouts` are the repositories granted for this cloud's checkouts; the others in
    /// `repositories` were allowed for the whole cloud on an agent's request.
    Current {
        login: String,
        repositories: Vec<String>,
        checkouts: Vec<String>,
        requests: bool,
    },
}

pub(super) fn status(connection: &Connection, runner: &Runner<'_>) -> Result<Status> {
    // The status is parsed, not shown: its JSON line is not deployment output.
    let quiet = Runner {
        cancel: runner.cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let output = quiet.run(
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
    }
    let Some(fields) = output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Fields>(line.trim()).ok())
    else {
        return Status::Absent;
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
        // A service that is not running serves no chain, stored or new.
        _ if fields.serving == Some(false) => Status::Unavailable,
        "ok" if !repositories.is_empty() => Status::Current {
            login: fields.login.unwrap_or_default(),
            repositories,
            checkouts,
            requests: fields.pending_requests.is_some(),
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

fn unix(at: std::time::SystemTime) -> u64 {
    at.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Writes the payload for `horizon-worker-github install` to a private temporary
/// file. The file is removed when the returned handle drops.
pub(super) fn payload(
    settings: &Settings,
    client_secret: Option<&Secret>,
    user: &User,
    grants: &[Grant],
    chain: &Chain,
) -> Result<tempfile::NamedTempFile> {
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
    let mut file = tempfile::NamedTempFile::new()?;
    serde_json::to_writer(&mut file, &payload).map_err(|_| Error::Json)?;
    file.flush()?;
    Ok(file)
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
    let file = payload(settings, client_secret, user, grants, chain)?;
    runner.private_payload(&mut connection.command("horizon-worker-github install"), file.path())
}
