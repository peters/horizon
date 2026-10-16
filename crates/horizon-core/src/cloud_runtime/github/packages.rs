//! The package read access of an Upkeep's cloud. GitHub's package API accepts no token of the
//! GitHub App behind Connect GitHub, so the agent could not check that a private package
//! version exists or how old it is. During a deployment of an Upkeep's cloud, right after its
//! GitHub access, Horizon therefore asks the person once: a device code for Horizon's own OAuth
//! app, with the scope `read:packages` only, approved with the person's own GitHub account.
//! Other clouds are not asked. The chain goes to
//! the worker's root GitHub service only (`horizon-worker-github packages`), which adds its
//! token to package reads and renews it with the client ID alone. No agent gets it, and Horizon
//! keeps no copy. A skipped or refused sign-in leaves the cloud without package reads and the
//! deployment continues.
use super::{Error, Event, Prompt, Result, Runner, Settings, publish, signin};
use crate::cloud_runtime::{ssh::Connection, state::Deployment};
use horizon_cloud::github::{Chain, Client};
use serde::Serialize;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
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
const WITHOUT: &str = "This Upkeep has no package read access, so checks of private package versions need a person.";

/// Saved beside the Upkeep's choice when the person skipped the package sign-in: a reconnect
/// does not ask again; Connect GitHub again on the card does.
const SKIPPED: &str = "package-access-skipped";

/// Whether the person skipped the package sign-in of the Upkeep `cloud_id`.
#[must_use]
pub fn skipped(cloud_id: &str) -> bool {
    crate::upkeep::worker::directory(cloud_id).join(SKIPPED).exists()
}

/// Forgets a skip, so the next deployment of the Upkeep `cloud_id` asks again. Blocks on the
/// disk.
/// # Errors
/// The mark is there but cannot be removed.
pub fn ask_again(cloud_id: &str) -> std::io::Result<()> {
    match std::fs::remove_file(crate::upkeep::worker::directory(cloud_id).join(SKIPPED)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// What the worker holds, from its GitHub service's `status`.
#[derive(Debug, PartialEq, Eq)]
enum Held {
    /// An image whose service has no package read access.
    Unsupported,
    /// No chain, or one that GitHub ended.
    Missing,
    /// A chain the worker renews, for these repositories (lowercase, sorted); `change` is
    /// whether its image takes new repositories without a new chain.
    Current { repositories: Vec<String>, change: bool },
}

/// The worker's package read access, and whether its GitHub service serves GitHub access.
fn held(output: &str) -> (Held, bool) {
    let Some(value) = output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
    else {
        return (Held::Unsupported, false);
    };
    // The GitHub step's own reading: a chain that its running service serves for repositories.
    let github = matches!(
        super::worker::parse_status(output),
        super::worker::Status::Current { .. }
    );
    let held = match value.get("packages") {
        None => Held::Unsupported,
        Some(packages) if packages.get("state").and_then(serde_json::Value::as_str) == Some("ok") => {
            let mut repositories: Vec<String> = packages
                .get("repositories")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_ascii_lowercase)
                .collect();
            repositories.sort();
            let change = packages.get("repositories_change").and_then(serde_json::Value::as_bool) == Some(true);
            Held::Current { repositories, change }
        }
        Some(_) => Held::Missing,
    };
    (held, github)
}

/// The Upkeep's own repositories, which alone open packages on the worker; never a
/// repository that an agent asked for. At most [`MAX_REPOSITORIES`], in lowercase.
fn own_repositories(config: &crate::upkeep::worker::Config) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let names: Vec<String> = config
        .repositories
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .filter(|name| horizon_cloud::github::valid_repository(name) && seen.insert(name.clone()))
        .collect();
    if names.len() > MAX_REPOSITORIES {
        return Err(Error::Invalid(
            "An Upkeep gives package read access for at most 50 repositories",
        ));
    }
    Ok(names)
}

/// Gives the worker of an Upkeep package read access when it holds none and its GitHub access
/// works: one device sign-in, unless the person skipped it and `renew` (Connect GitHub again)
/// is false. When the worker holds access for other repositories than the Upkeep's list, it
/// gets the new list without a sign-in; an image that cannot take it asks again. Never fails the
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
    // Only an Upkeep's cloud is asked; other clouds keep their flow without a second code.
    let Some(config) = settings.and_then(|_| crate::upkeep::worker::load(&state.cloud_id).ok()) else {
        return Ok(());
    };
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
    let (worker, github) = held(&output);
    if worker == Held::Unsupported {
        return Ok(());
    }
    if !github {
        say("this cloud has no GitHub access, so Horizon does not ask for package read access.");
        return Ok(());
    }
    let repositories = match own_repositories(&config) {
        Ok(repositories) if !repositories.is_empty() => repositories,
        Ok(_) => return Ok(()),
        Err(error) => {
            say(&error.to_string());
            say(WITHOUT);
            return Ok(());
        }
    };
    let mut wanted = repositories.clone();
    wanted.sort();
    let held_access = matches!(worker, Held::Current { .. });
    if !renew
        && covered(
            &worker,
            &wanted,
            &repositories,
            &state.cloud_id,
            connection,
            runner,
            &say,
        )?
    {
        return Ok(());
    }
    let cloud_id = &state.cloud_id;
    if renew {
        if let Err(error) = ask_again(cloud_id) {
            say(&format!("the earlier skip could not be forgotten: {error}"));
        }
    } else if skipped(cloud_id) {
        say(WITHOUT);
        return Ok(());
    }
    let client = Client::new();
    let outcome = sign_in(&client, cloud_id, runner);
    let ended = |allowed| (runner.emit)(Event::GitHub(Prompt::PackagesEnded { allowed }));
    if outcome.is_err() {
        ended(false);
    }
    let chain = match outcome {
        Ok(chain) => chain,
        Err(ended) => return without(ended, held_access, cloud_id, &say),
    };
    let closed = closed_owners(&client, &chain, &repositories, &say);
    let installed = install(&chain, &repositories, connection, runner);
    if installed.is_ok() {
        remember(cloud_id, wanted);
    }
    // Only now does the card learn the outcome: the worker holds the access or not.
    ended(installed.is_ok());
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

/// After a package sign-in that gave no chain: an earlier access stays as it was; otherwise a
/// skip is saved for this Upkeep, and the output says that package checks need a person.
/// # Errors
/// The deployment was cancelled.
fn without(ended: Ended, held_access: bool, cloud_id: &str, say: &dyn Fn(&str)) -> Result<()> {
    match ended {
        Ended::Cancelled(error) => return Err(error),
        Ended::Reason(reason) => say(&reason),
        Ended::Skipped if !held_access => {
            let mark = crate::upkeep::worker::directory(cloud_id).join(SKIPPED);
            if let Err(error) = crate::upkeep::worker::write_atomically(&mark, "skipped\n") {
                say(&format!(
                    "the skip could not be saved, so the next deployment asks again: {error}"
                ));
            }
        }
        Ended::Skipped => {}
    }
    say(if held_access {
        "the earlier package read access of this Upkeep stays as it was."
    } else {
        WITHOUT
    });
    Ok(())
}

/// Whether the worker's access already covers the Upkeep's repositories (`wanted`, sorted), or
/// covers them now because the worker took the new list; false when Horizon must ask again.
/// # Errors
/// The deployment was cancelled.
fn covered(
    worker: &Held,
    wanted: &[String],
    repositories: &[String],
    cloud_id: &str,
    connection: &Connection,
    runner: &Runner<'_>,
    say: &dyn Fn(&str),
) -> Result<bool> {
    let Held::Current {
        repositories: held,
        change: can_change,
    } = worker
    else {
        return Ok(false);
    };
    if held == wanted {
        return Ok(true);
    }
    if *can_change {
        let changed = change(repositories, &mut connection.command(INSTALL), runner);
        runner.cancel.check()?;
        if changed.is_ok() {
            remember(cloud_id, wanted.to_vec());
            say("the package read access now follows the Upkeep's repositories.");
            return Ok(true);
        }
        say("the worker could not change the repositories of its package read access.");
    }
    say("asking again, so that the package read access covers the Upkeep's repositories.");
    Ok(false)
}

/// The owners of `repositories` whose organization limits OAuth apps, so `chain` reads none of
/// their packages; each is named with its grant page. A check that fails is said too.
fn closed_owners(client: &Client, chain: &Chain, repositories: &[String], say: &dyn Fn(&str)) -> Vec<String> {
    let owners: BTreeSet<&str> = repositories
        .iter()
        .filter_map(|name| name.split_once('/').map(|(owner, _)| owner))
        .collect();
    let mut closed = Vec::new();
    for owner in owners {
        match client.packages_restricted(&chain.access_token, owner) {
            Ok(false) => {}
            Ok(true) => {
                closed.push(owner.to_owned());
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
    closed
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

#[derive(Serialize)]
struct Change<'a> {
    version: u8,
    repositories: &'a [String],
}

/// Tells root on the worker the Upkeep's repositories for the chain it holds, without a token,
/// through `command` (`horizon-worker-github packages`).
fn change(repositories: &[String], command: &mut std::process::Command, runner: &Runner<'_>) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new()?;
    serde_json::to_writer(
        &mut file,
        &Change {
            version: 1,
            repositories,
        },
    )
    .map_err(|_| Error::Json)?;
    file.flush()?;
    runner.private_payload(command, file.path())
}

/// The repositories each Upkeep's worker last took, so an exchange asks the worker only after
/// the Upkeep's list changed (and once after Horizon starts).
static FOLLOWED: LazyLock<Mutex<HashMap<String, Vec<String>>>> = LazyLock::new(Mutex::default);

fn remember(cloud_id: &str, repositories: Vec<String>) {
    if let Ok(mut followed) = FOLLOWED.lock() {
        followed.insert(cloud_id.to_owned(), repositories);
    }
}

/// Keeps the package read access of an Upkeep's worker on the Upkeep's repositories between
/// deployments, as each exchange runs: when the person changes the list, the worker gets the
/// new one without a sign-in. It asks the worker only when the list differs from the one it
/// last took, and changes nothing when the worker holds no access (the next deployment asks)
/// or its image cannot take a change. Never fails the exchange.
pub fn follow(connection: &Connection, runner: &Runner<'_>, cloud_id: &str, config: &crate::upkeep::worker::Config) {
    let Ok(mut wanted) = own_repositories(config) else {
        return;
    };
    wanted.sort();
    if wanted.is_empty()
        || FOLLOWED
            .lock()
            .is_ok_and(|followed| followed.get(cloud_id) == Some(&wanted))
    {
        return;
    }
    let Ok(output) = runner.run_parsed(
        "Package read access status",
        &mut connection.pinned_command(STATUS),
        Duration::from_secs(20),
    ) else {
        return;
    };
    match held(&output).0 {
        Held::Current { repositories, .. } if repositories == wanted => remember(cloud_id, wanted),
        Held::Current { change: true, .. }
            if change(&wanted, &mut connection.pinned_command(INSTALL), runner).is_ok() =>
        {
            remember(cloud_id, wanted);
        }
        _ => {}
    }
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
    fn the_worker_status_says_what_it_holds_and_whether_github_works() {
        let line = |state: &str, extra: &str| {
            format!(
                "{{\"state\":\"{state}\",\"login\":\"octo\",\"repositories\":[{{\"repository\":\"acme/api\",\"target\":\"primary\"}}]{extra}}}"
            )
        };
        assert_eq!(held(&line("ok", ",\"packages\":null")), (Held::Missing, true));
        assert_eq!(
            held(&line(
                "ok",
                ",\"packages\":{\"state\":\"ok\",\"repositories\":[\"B/x\",\"a/y\"],\"repositories_change\":true}"
            )),
            (
                Held::Current {
                    repositories: vec!["a/y".into(), "b/x".into()],
                    change: true
                },
                true
            )
        );
        let old = held(&line(
            "ok",
            ",\"packages\":{\"state\":\"ok\",\"repositories\":[\"a/y\"]}",
        ));
        assert!(matches!(old.0, Held::Current { change: false, .. }));
        assert_eq!(
            held(&line("ok", ",\"packages\":{\"state\":\"revoked\"}")).0,
            Held::Missing
        );
        // The GitHub step's own reading: an absent chain, a stopped service or no repository is no access.
        assert_eq!(held(&line("absent", ",\"packages\":null")), (Held::Missing, false));
        assert!(!held(&line("ok", ",\"packages\":null,\"serving\":false")).1);
        assert_eq!(
            held("{\"state\":\"ok\",\"repositories\":[],\"packages\":null}"),
            (Held::Missing, false)
        );
        assert_eq!(held(&line("ok", "")).0, Held::Unsupported);
        assert_eq!(held("not json"), (Held::Unsupported, false));
    }

    #[test]
    fn only_the_upkeep_repositories_go_with_the_chain_once_each_and_at_most_fifty() {
        let config = |repositories: Vec<String>| crate::upkeep::worker::Config {
            repositories,
            ..crate::upkeep::worker::Config::default()
        };
        let names = |list: &[&str]| list.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            own_repositories(&config(names(&["Acme/API", "acme/api", "acme/web", "not a repo"]))).unwrap(),
            ["acme/api", "acme/web"]
        );
        let many = (0..51).map(|index| format!("acme/r{index}")).collect();
        assert!(own_repositories(&config(many)).is_err());
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
