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
    /// A chain that renews, acting as `login`, for these lowercase repositories.
    Current { login: String, repositories: Vec<String> },
}

pub(super) fn status(connection: &Connection, runner: &Runner<'_>) -> Result<Status> {
    let output = runner.run(
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
        Grant { repository: String },
        Name(String),
    }
    #[derive(Deserialize)]
    struct Fields {
        state: String,
        #[serde(default)]
        login: Option<String>,
        #[serde(default)]
        repositories: Vec<Held>,
    }
    let Some(fields) = output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Fields>(line.trim()).ok())
    else {
        return Status::Absent;
    };
    let repositories: Vec<String> = fields
        .repositories
        .into_iter()
        .map(|held| match held {
            Held::Grant { repository } | Held::Name(repository) => repository,
        })
        .filter(|name| horizon_cloud::github::valid_repository(name))
        .map(|name| name.to_ascii_lowercase())
        .collect();
    match fields.state.as_str() {
        "unsupported" => Status::Unsupported,
        "ok" if !repositories.is_empty() => Status::Current {
            login: fields.login.unwrap_or_default(),
            repositories,
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
