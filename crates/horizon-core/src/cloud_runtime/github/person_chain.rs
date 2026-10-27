//! The person's own GitHub chain for an agent's cloud: the sign-in that this computer holds
//! for its GitHub App, installed in the worker in place of a token that only this machine
//! could mint. Nobody sits at the card of a cloud that an agent rents, so the worker renews
//! the chain from its refresh token from then on, and the cloud keeps its GitHub access while
//! this computer is off.
//!
//! The one moment a person is needed is the sign-in itself, and it happens once for the whole
//! machine: an agent's create that finds no stored chain runs the same sign-in that the
//! person's own path runs, shows the code through its runner, and continues when the sign-in
//! is approved. A policy that allows continuing without GitHub access proceeds instead of
//! asking.
//!
//! The chain carries the person's account, so a commit and a pull request of the worker are
//! authored as the person: GitHub attributes a commit to the account whose numeric ID the
//! author address begins with, which is the address this module builds.
use super::super::{
    command::Runner,
    settings::{AgentLifecycle, Settings as CloudSettings},
    ssh::Connection,
};
use super::{
    Event, Grant, Prompt, Result, Settings as AppSettings, Signed, github_origin, host, host::PersonChain, worker,
};
use horizon_cloud::github::{Chain, Client, Secret, User};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// A chain of this computer's GitHub sign-in, for the repositories of the agent lifecycle
/// policy, and the person it acts as.
#[derive(Debug)]
pub struct PersonAccess {
    /// The app the chain belongs to, whose client ID the payload names.
    pub(super) app: AppSettings,
    /// The client secret that a chain from a web sign-in needs to renew itself, which the
    /// worker then holds as well as the chain. A device chain renews with the client ID alone.
    pub(super) client_secret: Option<Secret>,
    /// The account the chain acts as, as a commit of the worker is authored.
    pub(super) user: User,
    /// The person chain, with the access token and the refresh token that renews it.
    pub(super) chain: Chain,
    /// Every repository the chain is installed for, lowercase `owner/name`: the repositories
    /// of the agent lifecycle policy.
    pub(super) repositories: Vec<String>,
}

impl PersonAccess {
    /// Whether the chain is installed for the lowercase `repository`.
    fn covers(&self, repository: &str) -> bool {
        self.repositories.iter().any(|name| name == repository)
    }
}

/// Why an agent's cloud gets no GitHub access from this computer's sign-in. Each is a
/// refusal of the whole create, reported before a card exists or a worker is rented, and it
/// names the setting or the sign-in that resolves it.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error(
        "{} has no GitHub remote, so a GitHub chain cannot reach it. Give the repository a GitHub origin, or remove it from the repositories of the agent lifecycle policy",
        path.display()
    )]
    NoRemote { path: PathBuf },
    #[error(
        "agent_lifecycle.repositories lists no repository, so an agent's cloud has nothing to install a GitHub chain for. Add a repository to agent_lifecycle.repositories, or set agent_lifecycle.allow_github_sign_in_skip to true so that an agent's cloud continues without GitHub access"
    )]
    NoPolicyRepository,
    #[error(
        "cloud settings name no GitHub App, so an agent's cloud cannot sign in to GitHub on this computer. Connect GitHub in Horizon's settings, or set agent_lifecycle.allow_github_sign_in_skip to true so that an agent's cloud continues without GitHub access"
    )]
    NoApp,
    #[error(
        "the GitHub sign-in that an agent's cloud needs did not finish: {0} Sign in again from the cloud's card, or set agent_lifecycle.allow_github_sign_in_skip to true so that an agent's cloud continues without GitHub access"
    )]
    SignIn(String),
    #[error(
        "this computer's stored GitHub sign-in could not be used for an agent's cloud: {0}. The stored sign-in is kept, so try again when GitHub answers"
    )]
    Kept(String),
}

/// The GitHub access of an agent's cloud, or `None` when a policy that allows continuing
/// without GitHub access finds no chain. This computer's own sign-in is the only credential
/// an agent's cloud takes: there is no fallback to another one.
///
/// # Errors
/// [`Refusal`], naming the setting or the sign-in that the create must resolve first.
pub fn for_agent_cloud(
    settings: &CloudSettings,
    runner: &Runner<'_>,
) -> std::result::Result<Option<PersonAccess>, Refusal> {
    for_agent_cloud_at(&Client::new(), settings, runner)
}

/// [`for_agent_cloud`] against `client`, so that a test answers as a fake GitHub.
pub(super) fn for_agent_cloud_at(
    client: &Client,
    settings: &CloudSettings,
    runner: &Runner<'_>,
) -> std::result::Result<Option<PersonAccess>, Refusal> {
    // A policy that allows continuing without GitHub access never asks a person for one.
    let skippable = settings
        .agent_lifecycle
        .as_ref()
        .is_some_and(|policy| policy.allow_github_sign_in_skip);
    let repositories = match settings.agent_lifecycle.as_ref() {
        Some(policy) => policy_repositories(policy, runner)?,
        None => Vec::new(),
    };
    if repositories.is_empty() {
        return if skippable {
            Ok(None)
        } else {
            Err(Refusal::NoPolicyRepository)
        };
    }
    // Both are needed to sign in: the app to sign in to, and the directory the chain is kept
    // in. Without them an agent's cloud can only continue without GitHub access.
    let (Some(app), Some(root)) = (settings.github.clone(), settings.root.clone()) else {
        return if skippable { Ok(None) } else { Err(Refusal::NoApp) };
    };
    stored(client, &root, &app, repositories.clone())?.map_or_else(
        || {
            if skippable {
                // A cloud that may continue without GitHub access does not wait for a person.
                return Ok(None);
            }
            ask(client, &root, &app, repositories, runner).map(Some)
        },
        |access| Ok(Some(access)),
    )
}

/// The chain this computer stores for the app of `settings`, for `repositories`, or `None`
/// when it holds none or GitHub ended it.
fn stored(
    client: &Client,
    root: &Path,
    app: &AppSettings,
    repositories: Vec<String>,
) -> std::result::Result<Option<PersonAccess>, Refusal> {
    let found = host::person_chain_at(root, app, client).map_err(|error| Refusal::Kept(error.to_string()))?;
    let (user, chain, client_secret) = match found {
        PersonChain::Chain {
            user,
            chain,
            client_secret,
        } => (user, chain, client_secret),
        PersonChain::SignedOut | PersonChain::Rejected => return Ok(None),
    };
    Ok(Some(PersonAccess {
        app: app.clone(),
        client_secret,
        user: author(&user),
        chain,
        repositories,
    }))
}

/// The one sign-in an agent's create may need: the same one the person's own path runs, shown
/// through `runner`, whose chain is then stored for every later cloud.
fn ask(
    client: &Client,
    root: &Path,
    app: &AppSettings,
    repositories: Vec<String>,
    runner: &Runner<'_>,
) -> std::result::Result<PersonAccess, Refusal> {
    let show = |prompt: Prompt| (runner.emit)(Event::GitHub(prompt));
    match host::sign_in_at(client, root, app, runner.cancel, &show) {
        Ok(Ok(_)) => {}
        Ok(Err(reason)) => return Err(Refusal::SignIn(reason)),
        Err(error) => return Err(Refusal::Kept(error.to_string())),
    }
    stored(client, root, app, repositories)?
        .ok_or_else(|| Refusal::SignIn("GitHub approved the sign-in, and this computer stored no chain for it.".into()))
}

/// The GitHub `owner/name` of every repository of the policy, lowercase and deduplicated. A
/// path whose origin Horizon cannot read has no GitHub remote that a chain could reach, so it
/// is the same refusal as an origin elsewhere.
fn policy_repositories(policy: &AgentLifecycle, runner: &Runner<'_>) -> std::result::Result<Vec<String>, Refusal> {
    let mut names = BTreeSet::new();
    for path in &policy.repositories {
        let Ok(Some(name)) = github_origin(path, runner) else {
            return Err(Refusal::NoRemote { path: path.clone() });
        };
        names.insert(name.to_ascii_lowercase());
    }
    Ok(names.into_iter().collect())
}

/// The identity a commit of the worker is authored as, from the account that the chain acts
/// as: its login, its name when GitHub answers one, and the noreply address of the account.
/// GitHub attributes such a commit to that account, and the address shows no address that the
/// person keeps private.
pub(super) fn author(user: &User) -> User {
    User {
        id: user.id,
        login: user.login.clone(),
        name: if user.name.trim().is_empty() {
            user.login.clone()
        } else {
            user.name.clone()
        },
        // Always the noreply address, which GitHub attributes to the account even when the
        // person keeps their email private, and which never exposes it in a commit.
        email: format!("{}+{}@users.noreply.github.com", user.id, user.login),
    }
}

/// Gives the worker the person chain, for the grants the policy's repositories reach, and
/// reports what the worker serves.
pub(super) fn install(
    access: &PersonAccess,
    grants: Vec<Grant>,
    connection: &Connection,
    runner: &Runner<'_>,
) -> Result<Signed> {
    let say = |text: &str| (runner.emit)(Event::Output(format!("GitHub: {text}")));
    let (reachable, missing): (Vec<Grant>, Vec<Grant>) =
        grants.into_iter().partition(|grant| access.covers(&grant.repository));
    for grant in &missing {
        say(&format!(
            "{} is not one of the repositories of the agent policy, so this computer's chain is not installed for it.",
            grant.repository
        ));
    }
    if reachable.is_empty() {
        return Ok(Signed::Refused(
            "This computer's GitHub chain reaches none of this cloud's repositories.".into(),
        ));
    }
    worker::install(
        connection,
        runner,
        &access.app,
        access.client_secret.as_ref(),
        &access.user,
        &reachable,
        &access.chain,
    )?;
    say(&format!(
        "the worker holds this computer's GitHub sign-in as {} for {} repositories, so its commits and pull requests are authored as that person. The worker renews the chain itself for as long as GitHub honours its refresh token.",
        access.user.login,
        reachable.len()
    ));
    // Connected only when the service serves the chain it was given; it also reports whether
    // it takes agents' access requests.
    let requests = match worker::status(connection, runner)? {
        worker::Status::Current { requests, .. } => requests,
        worker::Status::Unavailable => {
            return Ok(Signed::Unserved(
                "The worker's GitHub service is not running, so Git and gh have no GitHub access. \
                 The cloud's card asks an agent to start the sessions again."
                    .to_owned(),
            ));
        }
        worker::Status::Absent | worker::Status::Unsupported => {
            return Ok(Signed::Unserved(
                "The worker did not keep the GitHub access it was given.".to_owned(),
            ));
        }
    };
    (runner.emit)(Event::GitHub(Prompt::Connected {
        login: access.user.login.clone(),
        repositories: reachable.into_iter().map(|grant| grant.repository).collect(),
        requests,
        // This machine signs the cloud in again from its card, which is how a cloud whose
        // chain ended gets access a second time.
        renewable: true,
    }));
    Ok(Signed::In {
        complete: missing.is_empty(),
    })
}
