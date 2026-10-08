//! Connect GitHub: this machine's own GitHub App, and the token chain each cloud's
//! worker holds and renews. Machine-local; repository YAML cannot select it.
//!
//! A deployment that reaches the worker asks it whether it already holds current
//! access for the cloud's repositories. When it does not, Horizon signs in once for
//! that cloud ([`Mode::Ask`]: one Authorize click; [`Mode::Automatic`]: none) and
//! hands the chain to the worker's root service, which renews it from then on.
//! Signing in is optional for a cloud: a declined, expired or skipped sign-in leaves
//! the cloud without GitHub access and the deployment continues.
use super::{
    Error, Event, Result, command::Runner, git_auth::Target, settings::validate_private_key_file, ssh::Connection,
    state::Deployment,
};
use horizon_cloud::github::{Client, Secret};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod connect;
pub mod requests;
mod signin;
#[cfg(test)]
mod tests;
mod worker;

pub use signin::{Prompt, renew, skip};

/// Why a worker whose GitHub service is not running has no GitHub access.
const SERVICE_DOWN: &str = "The worker's GitHub service is not running, so Git and gh have no GitHub access. \
                            Restart the worker to start it again.";

/// The most grants `horizon-worker-github install` accepts.
const WORKER_GRANTS: usize = 16;

/// How a new cloud gets its GitHub access.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// One Authorize click on GitHub for each cloud. The client secret stays here.
    #[default]
    Ask,
    /// No click: the signed-in browser approves. Each worker keeps the client secret,
    /// readable only by its root service.
    Automatic,
}

/// The GitHub App that Horizon created for this machine's user.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub app_id: u64,
    pub slug: String,
    pub client_id: String,
    /// A private file with the app's client secret.
    pub client_secret_file: PathBuf,
    #[serde(default)]
    pub mode: Mode,
}

impl Settings {
    /// # Errors
    /// A relative secret path or a malformed app identity.
    pub fn validate(&self) -> Result<()> {
        if !self.client_secret_file.is_absolute() {
            return Err(Error::Invalid(
                "The GitHub App secret must use an absolute machine-local path",
            ));
        }
        let slug =
            (1..=100).contains(&self.slug.len()) && self.slug.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        let client =
            (1..=64).contains(&self.client_id.len()) && self.client_id.bytes().all(|b| b.is_ascii_alphanumeric());
        if !slug || !client || self.app_id == 0 {
            return Err(Error::Invalid("Invalid GitHub App settings"));
        }
        Ok(())
    }

    /// The page where the person adds or removes repositories of the app.
    #[must_use]
    pub fn installation_url(&self) -> String {
        format!("https://github.com/apps/{}/installations/new", self.slug)
    }

    fn client_secret(&self) -> Result<Secret> {
        validate_private_key_file(&self.client_secret_file)?;
        let secret = zeroize::Zeroizing::new(std::fs::read_to_string(&self.client_secret_file)?);
        let secret = secret.trim();
        if secret.is_empty() || secret.len() > 256 || !secret.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Error::Invalid("Invalid GitHub App secret file"));
        }
        Ok(Secret::new(secret.to_owned()))
    }
}

/// One worker repository and the GitHub repository it reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    /// Lowercase GitHub `owner/name`.
    pub repository: String,
    pub target: Target,
}

/// The GitHub repositories of a deployment: its primary when the checkout's origin is on
/// GitHub, and each same-worker sibling, which `.horizon/cloud.yml` names by `owner/name`.
/// # Errors
/// A sibling declaration that is not a GitHub repository name.
pub fn grants(state: &Deployment, runner: &Runner<'_>) -> Result<Vec<Grant>> {
    let mut grants = Vec::new();
    if let Ok(primary) = super::companions::inventory::identity(&state.repository, runner) {
        grants.push(Grant {
            repository: primary.to_ascii_lowercase(),
            target: Target::Primary,
        });
    }
    for sibling in state.siblings.iter().flat_map(|set| &set.members) {
        if !horizon_cloud::github::valid_repository(&sibling.repository) {
            return Err(Error::Invalid("A sibling repository is not a GitHub owner/name"));
        }
        let target = Target::try_from(format!("sibling:{}", sibling.alias)).map_err(Error::Invalid)?;
        grants.push(Grant {
            repository: sibling.repository.to_ascii_lowercase(),
            target,
        });
    }
    let mut seen = std::collections::HashSet::new();
    if grants.iter().any(|grant| !seen.insert(grant.repository.clone())) {
        return Err(Error::Invalid(
            "One GitHub repository is checked out twice on this worker",
        ));
    }
    // Refused before any sign-in: the worker takes at most this many grants.
    if grants.len() > WORKER_GRANTS {
        return Err(Error::Invalid(
            "A worker takes GitHub access for at most 16 repositories",
        ));
    }
    Ok(grants)
}

/// Gives the worker current GitHub access for the cloud's repositories, signing in
/// for this cloud when the worker holds none. Every GitHub-side refusal is reported
/// and leaves the cloud without GitHub access; only a worker that cannot store the
/// access fails the deployment. Returns whether the worker serves access from the app,
/// which then replaces any credential binding from cloud settings.
/// # Errors
/// The worker refused the access it was given.
pub fn configure(
    settings: Option<&Settings>,
    state: &Deployment,
    connection: &Connection,
    runner: &Runner<'_>,
    legacy: bool,
) -> Result<bool> {
    let say = |text: &str| (runner.emit)(Event::Output(format!("GitHub: {text}")));
    // A cloud without GitHub access from the app may still have a credential binding
    // from cloud settings; the outcome says so instead of claiming no access.
    let end = |reason: String| {
        let reason = if legacy {
            format!("{reason} Git uses the credential binding from cloud settings.")
        } else {
            reason
        };
        say(&reason);
        (runner.emit)(Event::GitHub(Prompt::Ended(reason)));
    };
    let connected = |login, repositories, requests| {
        (runner.emit)(Event::GitHub(Prompt::Connected {
            login,
            repositories,
            requests,
            renewable: settings.is_some(),
        }));
    };
    // Consumed once, whatever the worker holds, so a later reconnect never signs in anew.
    let renew = signin::renewing(&state.cloud_id);
    let grants = match settings {
        Some(_) => grants(state, runner)?,
        None => Vec::new(),
    };
    if settings.is_some() && grants.is_empty() {
        end("This cloud has no repository on GitHub.".into());
        return Ok(false);
    }
    // Asked also without settings: a worker keeps its access after Disconnect, and the
    // card still shows it and its agents' requests.
    let held = worker::status(connection, runner)?;
    let Some(settings) = settings else {
        if let worker::Status::Current {
            login,
            repositories,
            requests,
            ..
        } = held
        {
            say("the worker holds access from an earlier connection.");
            connected(login, repositories, requests);
            return Ok(true);
        }
        return Ok(false);
    };
    let kept = match held {
        worker::Status::Unsupported => {
            say("this worker image cannot hold GitHub access. Rebuild the image to add it.");
            end("This worker image cannot hold GitHub access.".into());
            return Ok(false);
        }
        worker::Status::Unavailable => {
            end(SERVICE_DOWN.into());
            return Ok(false);
        }
        // A repository the worker does not reach stays out until the person chooses
        // Connect GitHub again, so a reconnect never asks for a sign-in by itself.
        worker::Status::Current {
            login,
            repositories,
            checkouts,
            requests,
        } if !renew && !narrower(&checkouts, &grants, &say) => {
            say("the worker holds current access.");
            for grant in grants.iter().filter(|grant| !covers(&repositories, grant)) {
                say(&format!(
                    "{} is not in this cloud's access. Use Connect GitHub again on the cloud card to add it.",
                    grant.repository
                ));
            }
            connected(login, repositories, requests);
            return Ok(true);
        }
        // Connect GitHub again: sign in anew, and keep the current access if that fails.
        worker::Status::Current {
            login,
            repositories,
            requests,
            ..
        } => Some((login, repositories, requests)),
        worker::Status::Absent => None,
    };
    Ok(match sign_in(settings, state, grants, connection, runner)? {
        Ok(()) => true,
        Err(reason) => {
            if let Some((login, repositories, requests)) = kept {
                say(&format!("{reason} The cloud keeps its current GitHub access."));
                connected(login, repositories, requests);
                true
            } else {
                end(reason);
                false
            }
        }
    })
}

/// Signs in for this cloud and gives the worker the chain, for the repositories where
/// the app is installed. A GitHub-side refusal is returned as its reason, not an error.
fn sign_in(
    settings: &Settings,
    state: &Deployment,
    grants: Vec<Grant>,
    connection: &Connection,
    runner: &Runner<'_>,
) -> Result<std::result::Result<(), String>> {
    let say = |text: &str| (runner.emit)(Event::Output(format!("GitHub: {text}")));
    let client = Client::new();
    let chain = match signin::chain(settings, &state.cloud_id, &client, runner) {
        Ok(chain) => chain,
        Err(signin::Ended::Error(error)) => return Err(error),
        Err(signin::Ended::Reason(reason)) => return Ok(Err(reason)),
    };
    let (user, installed) = match client
        .user(&chain.access_token)
        .and_then(|user| Ok((user, client.installed_repositories(&chain.access_token)?)))
    {
        Ok(found) => found,
        Err(error) => return Ok(Err(error.to_string())),
    };
    let (reachable, missing): (Vec<_>, Vec<_>) = grants
        .into_iter()
        .partition(|grant| installed.contains(&grant.repository));
    for grant in &missing {
        say(&format!(
            "the app is not installed on {}. Add it at {}",
            grant.repository,
            settings.installation_url()
        ));
    }
    if reachable.is_empty() {
        return Ok(Err(
            "The GitHub App is not installed on this cloud's repositories.".into()
        ));
    }
    let secret = match settings.mode {
        Mode::Ask => None,
        Mode::Automatic => Some(settings.client_secret()?),
    };
    worker::install(connection, runner, settings, secret.as_ref(), &user, &reachable, &chain)?;
    say(&format!(
        "signed in as {} for {} repositories.",
        user.login,
        reachable.len()
    ));
    // Connected only when the service serves the chain it now holds; it also reports
    // whether it takes agents' requests.
    let requests = match worker::status(connection, runner)? {
        worker::Status::Current { requests, .. } => requests,
        worker::Status::Unavailable => return Ok(Err(SERVICE_DOWN.into())),
        worker::Status::Absent | worker::Status::Unsupported => {
            return Ok(Err("The worker did not keep the GitHub access it was given.".into()));
        }
    };
    (runner.emit)(Event::GitHub(Prompt::Connected {
        login: user.login,
        repositories: reachable.into_iter().map(|grant| grant.repository).collect(),
        requests,
        renewable: true,
    }));
    Ok(Ok(()))
}

/// Whether the worker holds access for checkouts this cloud no longer has, such as a
/// removed sibling. Such access is narrowed by signing in again, which `say` announces.
fn narrower(checkouts: &[String], grants: &[Grant], say: &dyn Fn(&str)) -> bool {
    let stale: Vec<&String> = checkouts
        .iter()
        .filter(|held| !grants.iter().any(|grant| grant.repository.eq_ignore_ascii_case(held)))
        .collect();
    for repository in &stale {
        say(&format!(
            "{repository} is no longer checked out on this cloud; signing in again to narrow its access."
        ));
    }
    !stale.is_empty()
}

/// Whether the worker's access reaches the grant's repository.
fn covers(held: &[String], grant: &Grant) -> bool {
    held.iter()
        .any(|repository| repository.eq_ignore_ascii_case(&grant.repository))
}
