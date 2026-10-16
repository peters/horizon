//! The cloud's package read access. GitHub's package API accepts no token of the GitHub App
//! behind Connect GitHub, so agents could not check that a private package version exists or
//! how old it is. During a deployment, right after the cloud's GitHub access, Horizon therefore
//! asks the person once for that cloud: a device code for Horizon's own OAuth app, with the
//! scope `read:packages` only, approved with the person's own GitHub account. The chain goes to
//! the worker's root GitHub service only (`horizon-worker-github packages`), which adds its
//! token to package reads and renews it with the client ID alone. No agent gets it, and Horizon
//! keeps no copy. A skipped or refused sign-in leaves the cloud without package reads and the
//! deployment continues.
use super::{Error, Event, Prompt, Result, Runner, Settings, publish, signin};
use crate::cloud_runtime::{ssh::Connection, state::Deployment};
use horizon_cloud::github::{Chain, Client};
use serde::Serialize;
use std::{
    collections::{BTreeSet, HashSet},
    io::Write as _,
    sync::{LazyLock, Mutex},
    time::Duration,
};

/// The only scope the chain may carry.
pub const SCOPE: &str = "read:packages";
/// Where the person sees, and can revoke, what Horizon's OAuth app may do.
pub use publish::APP_PAGE;
const STATUS: &str = "horizon-worker-github status 2>/dev/null || true";
const INSTALL: &str = "horizon-worker-github packages";
/// The most repositories the worker's root service takes with a chain.
const MAX_REPOSITORIES: usize = 50;
/// What a skip or a failed sign-in means for the cloud, also on the Upkeep board.
const WITHOUT: &str = "This cloud has no package read access, so checks of private package versions need a person.";

/// Clouds whose person skipped the package sign-in in this run of Horizon. A reconnect does
/// not ask again; Connect GitHub again on the card does.
static SKIPPED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// What the worker holds, from its GitHub service's `status`.
#[derive(Debug, PartialEq, Eq)]
enum Held {
    /// An image whose service has no package read access.
    Unsupported,
    /// No chain, or one that GitHub ended.
    Missing,
    /// A chain the worker renews.
    Current,
}

/// The worker's package read access, from its GitHub service's `status`.
fn held(output: &str) -> Held {
    let Some(value) = output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
    else {
        return Held::Unsupported;
    };
    match value.get("packages") {
        None => Held::Unsupported,
        Some(serde_json::Value::Null) => Held::Missing,
        Some(packages) if packages.get("state").and_then(serde_json::Value::as_str) == Some("ok") => Held::Current,
        Some(_) => Held::Missing,
    }
}

/// The cloud's own repositories, which alone open packages on the worker: its checkouts on
/// GitHub (the primary and the same-worker siblings) and, for an Upkeep, its list. Never a
/// repository that an agent asked for. At most [`MAX_REPOSITORIES`], in lowercase.
fn own_repositories(state: &Deployment, runner: &Runner<'_>) -> Result<Vec<String>> {
    let mut names: Vec<String> = super::grants(state, runner)?
        .into_iter()
        .map(|grant| grant.repository)
        .collect();
    if let Ok(config) = crate::upkeep::worker::load(&state.cloud_id) {
        names.extend(config.repositories.iter().map(|name| name.to_ascii_lowercase()));
    }
    let mut seen = HashSet::new();
    names.retain(|name| horizon_cloud::github::valid_repository(name) && seen.insert(name.clone()));
    if names.len() > MAX_REPOSITORIES {
        return Err(Error::Invalid(
            "A cloud gives package read access for at most 50 repositories",
        ));
    }
    Ok(names)
}

/// Gives the worker package read access when it holds none: one device sign-in for the cloud,
/// unless the person skipped it in this run of Horizon and `renew` is false. Never fails the
/// deployment, except for a cancellation; every other outcome is said in its output.
/// # Errors
/// The deployment was cancelled.
pub fn configure(
    settings: Option<&Settings>,
    state: &Deployment,
    connection: &Connection,
    runner: &Runner<'_>,
    renew: bool,
) -> Result<()> {
    if settings.is_none() {
        return Ok(());
    }
    let say = |text: &str| (runner.emit)(Event::Output(format!("GitHub packages: {text}")));
    let status = runner.run_parsed(
        "Package read access status",
        &mut connection.command(STATUS),
        Duration::from_secs(20),
    );
    runner.cancel.check()?;
    let Ok(output) = status else {
        say("the worker's package read access could not be read, so Horizon did not ask for it.");
        return Ok(());
    };
    if held(&output) != Held::Missing {
        return Ok(());
    }
    let repositories = match own_repositories(state, runner) {
        Ok(repositories) if !repositories.is_empty() => repositories,
        Ok(_) => return Ok(()),
        Err(error) => {
            runner.cancel.check()?;
            say(&error.to_string());
            say(WITHOUT);
            return Ok(());
        }
    };
    let cloud_id = &state.cloud_id;
    if renew {
        SKIPPED.lock().map(|mut skipped| skipped.remove(cloud_id)).ok();
    } else if SKIPPED.lock().is_ok_and(|skipped| skipped.contains(cloud_id)) {
        say(WITHOUT);
        return Ok(());
    }
    let client = Client::new();
    let outcome = sign_in(&client, cloud_id, runner);
    (runner.emit)(Event::GitHub(Prompt::PackagesEnded {
        allowed: outcome.is_ok(),
    }));
    let chain = match outcome {
        Ok(chain) => chain,
        Err(Ended::Cancelled(error)) => return Err(error),
        Err(Ended::Skipped) => {
            if let Ok(mut skipped) = SKIPPED.lock() {
                skipped.insert(cloud_id.clone());
            }
            say(WITHOUT);
            return Ok(());
        }
        Err(Ended::Reason(reason)) => {
            say(&reason);
            say(WITHOUT);
            return Ok(());
        }
    };
    let owners: BTreeSet<&str> = repositories
        .iter()
        .filter_map(|name| name.split_once('/').map(|(owner, _)| owner))
        .collect();
    let mut closed = Vec::new();
    for owner in owners {
        match client.packages_restricted(&chain.access_token, owner) {
            Ok(false) => {}
            Ok(true) => {
                closed.push(owner);
                say(&format!(
                    "the organization {owner} limits OAuth apps, so agents cannot read its packages until an owner \
                     of {owner} grants Horizon at \
                     https://github.com/organizations/{owner}/settings/oauth_application_policy (others can ask for \
                     it at {APP_PAGE})."
                ));
            }
            Err(error) => say(&format!("could not check whether {owner} limits OAuth apps: {error}")),
        }
    }
    let installed = install(&chain, &repositories, connection, runner);
    runner.cancel.check()?;
    if installed.is_ok() && closed.is_empty() {
        say("the worker can now read the packages of this cloud's owners for its agents.");
    } else if installed.is_ok() {
        say(&format!(
            "the worker can now read the packages of this cloud's owners for its agents, except {}.",
            closed.join(", ")
        ));
    } else {
        say("the worker could not store the package read access.");
        say(WITHOUT);
    }
    Ok(())
}

/// Why no chain came back.
enum Ended {
    Skipped,
    Reason(String),
    Cancelled(Error),
}

/// One device sign-in for Horizon's OAuth app with [`SCOPE`] only, checked with GitHub.
fn sign_in(client: &Client, cloud_id: &str, runner: &Runner<'_>) -> std::result::Result<Chain, Ended> {
    runner.cancel.check().map_err(|error| Ended::Cancelled(error.into()))?;
    // A Skip from an earlier attempt does not end this one.
    signin::skipped(cloud_id);
    (runner.emit)(Event::Output(
        "GitHub packages: asking you to let this cloud's agents read packages; the worker's system service \
         keeps the access, agents never see it."
            .into(),
    ));
    let prompt = |user_code, verification_uri, expires_at| Prompt::Packages {
        user_code,
        verification_uri,
        expires_at,
    };
    let chain = match signin::device_chain(client, publish::CLIENT_ID, Some(SCOPE), cloud_id, runner, prompt) {
        Ok(chain) => chain,
        Err(signin::Ended::Skipped) => return Err(Ended::Skipped),
        Err(signin::Ended::Reason(reason)) => return Err(Ended::Reason(reason)),
        Err(signin::Ended::Error(error)) => return Err(Ended::Cancelled(error)),
    };
    runner.cancel.check().map_err(|error| Ended::Cancelled(error.into()))?;
    let scopes = client
        .token_scopes(&chain.access_token)
        .map_err(|error| Ended::Reason(error.to_string()))?;
    check_scopes(scopes.as_deref())?;
    Ok(chain)
}

/// GitHub must have granted exactly [`SCOPE`]: the worker never gets a broader token.
fn check_scopes(scopes: Option<&[String]>) -> std::result::Result<(), Ended> {
    match scopes {
        Some([only]) if only == SCOPE => Ok(()),
        Some(scopes) => Err(Ended::Reason(format!(
            "GitHub granted more or other than read:packages ({}), so the worker does not get it.",
            scopes.join(", ")
        ))),
        None => Err(Ended::Reason(
            "GitHub did not say which scopes it granted, so the worker does not get the access.".into(),
        )),
    }
}

#[derive(Serialize)]
struct Payload<'a> {
    version: u8,
    client_id: &'a str,
    repositories: &'a [String],
    chain: PayloadChain<'a>,
}

#[derive(Serialize)]
struct PayloadChain<'a> {
    access_token: &'a str,
    access_expires_at: u64,
    refresh_token: &'a str,
    refresh_expires_at: u64,
}

/// The request for the worker's root service: the client ID, the cloud's own repositories and
/// the chain, no secret.
fn payload(chain: &Chain, repositories: &[String]) -> Result<tempfile::NamedTempFile> {
    let unix = |at: std::time::SystemTime| {
        at.duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs())
    };
    let value = Payload {
        version: 1,
        client_id: publish::CLIENT_ID,
        repositories,
        chain: PayloadChain {
            access_token: chain.access_token.expose(),
            access_expires_at: unix(chain.access_expires_at),
            refresh_token: chain.refresh_token.expose(),
            refresh_expires_at: unix(chain.refresh_expires_at),
        },
    };
    let mut file = tempfile::NamedTempFile::new()?;
    serde_json::to_writer(&mut file, &value).map_err(|_| Error::Json)?;
    file.flush()?;
    Ok(file)
}

/// Gives the chain to root on the worker over SSH stdin; no token enters argv or output.
fn install(chain: &Chain, repositories: &[String], connection: &Connection, runner: &Runner<'_>) -> Result<()> {
    let file = payload(chain, repositories)?;
    runner.private_payload(&mut connection.command(INSTALL), file.path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_cloud::github::Secret;

    fn sample(token: &str) -> Chain {
        let now = std::time::SystemTime::now();
        Chain {
            access_token: Secret::new(token.into()),
            access_expires_at: now + Duration::from_hours(8),
            refresh_token: Secret::new(format!("{token}-refresh")),
            refresh_expires_at: now + Duration::from_hours(180 * 24),
        }
    }

    #[test]
    fn the_worker_status_says_what_it_holds() {
        let line = |packages: &str| format!("{{\"state\":\"ok\",\"repositories\":[\"acme/api\"]{packages}}}");
        assert_eq!(held(&line(",\"packages\":null")), Held::Missing);
        assert_eq!(held(&line(",\"packages\":{\"state\":\"ok\"}")), Held::Current);
        assert_eq!(held(&line(",\"packages\":{\"state\":\"revoked\"}")), Held::Missing);
        assert_eq!(held(&line("")), Held::Unsupported);
        assert_eq!(held("not json"), Held::Unsupported);
    }

    #[test]
    fn only_exactly_read_packages_reaches_the_worker() {
        let scopes = |list: &[&str]| list.iter().map(|scope| (*scope).to_owned()).collect::<Vec<_>>();
        assert!(check_scopes(Some(&scopes(&["read:packages"]))).is_ok());
        for list in [
            &["write:packages"][..],
            &["read:packages", "repo"],
            &[],
            &["READ:PACKAGES"],
        ] {
            assert!(
                matches!(check_scopes(Some(&scopes(list))), Err(Ended::Reason(reason)) if reason.contains("does not get")),
                "{list:?}"
            );
        }
        assert!(matches!(check_scopes(None), Err(Ended::Reason(_))));
    }

    #[test]
    fn the_payload_holds_the_client_id_and_the_chain_but_no_secret() {
        let repositories = vec!["acme/api".to_owned(), "acme/web".to_owned()];
        let file = payload(&sample("gho_synthetic-packages"), &repositories).unwrap();
        let value: serde_json::Value = serde_json::from_reader(file.reopen().unwrap()).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["client_id"], publish::CLIENT_ID);
        assert_eq!(value["repositories"], serde_json::json!(["acme/api", "acme/web"]));
        assert_eq!(value["chain"]["access_token"], "gho_synthetic-packages");
        assert_eq!(value["chain"]["refresh_token"], "gho_synthetic-packages-refresh");
        assert!(value["chain"]["access_expires_at"].as_u64().unwrap() > 0);
        assert!(value.get("client_secret").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(file.as_file().metadata().unwrap().permissions().mode() & 0o077, 0);
        }
    }
}
