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

pub use signin::{Prompt, skip};

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
    grants.dedup_by(|a, b| a.repository == b.repository);
    Ok(grants)
}

/// Gives the worker current GitHub access for the cloud's repositories, signing in
/// for this cloud when the worker holds none. Every GitHub-side refusal is reported
/// and leaves the cloud without GitHub access; only a worker that cannot store the
/// access fails the deployment.
/// # Errors
/// The worker refused the access it was given.
pub fn configure(
    settings: Option<&Settings>,
    state: &Deployment,
    connection: &Connection,
    runner: &Runner<'_>,
) -> Result<()> {
    let Some(settings) = settings else {
        return Ok(());
    };
    let say = |text: &str| (runner.emit)(Event::Output(format!("GitHub: {text}")));
    let grants = grants(state, runner)?;
    if grants.is_empty() {
        say("this cloud has no repository on GitHub.");
        return Ok(());
    }
    match worker::status(connection, runner)? {
        worker::Status::Unsupported => {
            say("this worker image cannot hold GitHub access. Rebuild the image to add it.");
            (runner.emit)(Event::GitHub(Prompt::Ended(
                "This worker image cannot hold GitHub access.".into(),
            )));
            return Ok(());
        }
        // A repository the worker does not reach stays out until the person connects
        // this cloud again, so a reconnect never asks for a sign-in by itself.
        worker::Status::Current { login, repositories } => {
            say("the worker holds current access.");
            for grant in grants.iter().filter(|grant| !covers(&repositories, grant)) {
                say(&format!(
                    "{} is not in this cloud's access. Connect GitHub again to add it.",
                    grant.repository
                ));
            }
            (runner.emit)(Event::GitHub(Prompt::Connected { login, repositories }));
            return Ok(());
        }
        worker::Status::Absent => {}
    }
    let client = Client::new();
    let chain = match signin::chain(settings, &state.cloud_id, &client, runner) {
        Ok(chain) => chain,
        Err(signin::Ended::Error(error)) => return Err(error),
        Err(signin::Ended::Reason(reason)) => {
            say(&reason);
            (runner.emit)(Event::GitHub(Prompt::Ended(reason)));
            return Ok(());
        }
    };
    let (user, installed) = match client
        .user(&chain.access_token)
        .and_then(|user| Ok((user, client.installed_repositories(&chain.access_token)?)))
    {
        Ok(found) => found,
        Err(error) => {
            say(&error.to_string());
            (runner.emit)(Event::GitHub(Prompt::Ended(error.to_string())));
            return Ok(());
        }
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
        (runner.emit)(Event::GitHub(Prompt::Ended(
            "The GitHub App is not installed on this cloud's repositories.".into(),
        )));
        return Ok(());
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
    (runner.emit)(Event::GitHub(Prompt::Connected {
        login: user.login,
        repositories: reachable.into_iter().map(|grant| grant.repository).collect(),
    }));
    Ok(())
}

/// Whether the worker's access reaches the grant's repository.
fn covers(held: &[String], grant: &Grant) -> bool {
    held.iter()
        .any(|repository| repository.eq_ignore_ascii_case(&grant.repository))
}
