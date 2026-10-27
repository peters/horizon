//! The GitHub access of an agent's cloud: a GitHub App installation token that this machine
//! mints for exactly the repositories of the agent lifecycle policy. It replaces the person's
//! sign-in, because nobody sits at the card of a cloud that an agent rents.
//!
//! The token is minted twice by design. The create path mints once to refuse a policy that no
//! token reaches, before a card exists or a worker is rented. The deployment mints again, so
//! the worker receives a token that lasts up to an hour from the install rather than from the
//! answer, which a long image build may already have spent. The app's key never leaves this
//! computer, and no holder of the token can renew it: only the key mints the next one.
use super::super::{
    command::Runner,
    settings::{AgentLifecycle, Settings},
    ssh::Connection,
};
use super::{Event, Grant, Prompt, Result, Serves, Settings as AppSettings, Signed, github_origin, serving, worker};
use horizon_cloud::github::{Bot, Client, Secret};
use std::{collections::BTreeSet, path::PathBuf, time::SystemTime};

/// A token of the app's installation, for exactly the repositories of the agent policy, and
/// what a commit of that token acts as.
#[derive(Debug)]
pub struct Installation {
    /// The app's client ID, which the worker's payload names.
    pub(super) client_id: String,
    /// The token value. It is wiped on drop and prints as `<redacted>`.
    pub(super) token: Secret,
    /// When GitHub ends the token, at most one hour after the mint.
    pub(super) expires_at: SystemTime,
    /// The bot that this token acts as, which a commit names.
    pub(super) bot: Bot,
    /// Every repository the token reaches, lowercase `owner/name`.
    pub(super) repositories: Vec<String>,
}

impl Installation {
    /// Whether the token reaches the lowercase `repository`.
    fn covers(&self, repository: &str) -> bool {
        self.repositories.iter().any(|name| name == repository)
    }
}

/// Why an agent's cloud gets no installation token. Each is a refusal of the whole create,
/// reported before a card exists or a worker is rented.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error(
        "{} has no GitHub remote, so the GitHub App cannot reach it. Give the repository a GitHub origin, or remove it from the repositories of the agent lifecycle policy",
        path.display()
    )]
    NoRemote { path: PathBuf },
    #[error(
        "the agent policy holds {repository} on another account than {account}, and one installation token reaches one account. List the repositories of one account"
    )]
    SeparateAccounts { account: String, repository: String },
    #[error(
        "the GitHub App is not installed on {account}, so no token reaches the agent policy ({repositories}). Install the app on {account} at {url}"
    )]
    NotInstalled {
        account: String,
        repositories: String,
        url: String,
    },
    #[error(
        "the GitHub App does not reach every repository of the agent policy ({repositories}), and Horizon cannot tell which one GitHub refused. Add them to the app's installation at {url}"
    )]
    NotCovered { repositories: String, url: String },
    #[error("this machine cannot use the GitHub App's private key: {0}")]
    Key(String),
    #[error(transparent)]
    GitHub(#[from] horizon_cloud::github::Error),
}

/// The app's installation token for the repositories of the agent lifecycle policy. `None` when
/// cloud settings name no private key, or the policy lists no repository, and the create then
/// asks a person to sign in as before.
///
/// # Errors
/// [`Refusal`], naming the repository that no token reaches.
pub fn for_agent_cloud(settings: &Settings, runner: &Runner<'_>) -> std::result::Result<Option<Installation>, Refusal> {
    let Some(app) = settings.github.as_ref() else {
        return Ok(None);
    };
    if app.private_key_file.is_none() {
        return Ok(None);
    }
    let Some(policy) = settings.agent_lifecycle.as_ref() else {
        return Ok(None);
    };
    let repositories = policy_repositories(policy, runner)?;
    if repositories.is_empty() {
        return Ok(None);
    }
    let key = app.private_key().map_err(|error| Refusal::Key(error.to_string()))?;
    // The token is requested for the policy's repositories and nothing else, so it reaches no
    // repository that the policy does not name.
    let minted = Client::new()
        .installation_token(app.app_id, &key, &repositories)
        .map_err(|error| refused(error, &repositories, app))?;
    Ok(Some(Installation {
        client_id: app.client_id.clone(),
        token: minted.token,
        expires_at: minted.expires_at,
        bot: minted.bot,
        repositories,
    }))
}

/// The GitHub `owner/name` of every repository of the policy, lowercase and deduplicated.
/// A path whose origin Horizon cannot read has no GitHub remote that a token could reach, so it
/// is the same refusal as an origin elsewhere.
fn policy_repositories(policy: &AgentLifecycle, runner: &Runner<'_>) -> std::result::Result<Vec<String>, Refusal> {
    let mut names = BTreeSet::new();
    for path in &policy.repositories {
        let Ok(Some(name)) = github_origin(path, runner) else {
            return Err(Refusal::NoRemote { path: path.clone() });
        };
        names.insert(name.to_ascii_lowercase());
    }
    let repositories: Vec<String> = names.into_iter().collect();
    // One installation token belongs to one account, and GitHub installs an app on each account
    // separately. The first repository that names another account is the one to move or remove.
    let Some((account, _)) = repositories.first().and_then(|name| name.split_once('/')) else {
        return Ok(repositories);
    };
    if let Some(repository) = repositories.iter().find(|name| {
        name.split_once('/')
            .is_none_or(|(owner, _)| !owner.eq_ignore_ascii_case(account))
    }) {
        return Err(Refusal::SeparateAccounts {
            account: account.to_ascii_lowercase(),
            repository: repository.clone(),
        });
    }
    Ok(repositories)
}

/// GitHub's answer to a mint that gave no token, as the refusal the create reports. The two
/// answers that name a repository group say which repositories the app must reach.
fn refused(error: horizon_cloud::github::Error, repositories: &[String], app: &AppSettings) -> Refusal {
    let names = repositories.join(", ");
    match error {
        horizon_cloud::github::Error::NotInstalled => Refusal::NotInstalled {
            account: account_of(repositories),
            repositories: names,
            url: app.installation_url(),
        },
        horizon_cloud::github::Error::NotCovered => Refusal::NotCovered {
            repositories: names,
            url: app.installation_url(),
        },
        error => Refusal::GitHub(error),
    }
}

/// The account that the policy's repositories belong to. The mint is asked for one account only,
/// so the first repository names it.
fn account_of(repositories: &[String]) -> String {
    repositories
        .first()
        .and_then(|name| name.split_once('/'))
        .map_or_else(String::new, |(account, _)| account.to_ascii_lowercase())
}

/// Gives the worker the installation token, for the grants the token reaches, and reports what
/// the worker serves. A grant outside the policy's repositories keeps no access, as a repository
/// the app does not reach keeps none after a sign-in.
pub(super) fn install(
    installation: &Installation,
    grants: Vec<Grant>,
    connection: &Connection,
    runner: &Runner<'_>,
) -> Result<Signed> {
    let say = |text: &str| (runner.emit)(Event::Output(format!("GitHub: {text}")));
    let (reachable, missing): (Vec<Grant>, Vec<Grant>) = grants
        .into_iter()
        .partition(|grant| installation.covers(&grant.repository));
    for grant in &missing {
        say(&format!(
            "{} is not one of the repositories of the agent policy, so the app's token does not reach it.",
            grant.repository
        ));
    }
    if reachable.is_empty() {
        return Ok(Signed::Refused(
            "The GitHub App's token reaches none of this cloud's repositories.".into(),
        ));
    }
    worker::install_installation(connection, runner, installation, &reachable)?;
    say(&format!(
        "the worker holds a GitHub App installation token as {} for {} repositories. It ends at most an hour after the mint, and only this machine installs a new one.",
        installation.bot.login,
        reachable.len()
    ));
    let requests = match serving(connection, runner)? {
        Serves::Requests(requests) => requests,
        Serves::Nothing(reason) => return Ok(Signed::Unserved(reason.to_owned())),
    };
    (runner.emit)(Event::GitHub(Prompt::Connected {
        login: installation.bot.login.clone(),
        repositories: reachable.into_iter().map(|grant| grant.repository).collect(),
        requests,
        // This machine signs the cloud in again from its card, which is how a cloud whose
        // installation token ended gets access a second time. The app's next token comes from
        // the next create of an agent's cloud.
        renewable: true,
    }));
    Ok(Signed::In {
        complete: missing.is_empty(),
    })
}
