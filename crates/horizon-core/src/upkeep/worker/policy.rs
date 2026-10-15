//! The Upkeep policy of the worker's GitHub access. Before each exchange, Horizon tells the
//! worker's root GitHub service, as root, the loop's session, the repositories and bots of
//! the Upkeep, and which workflows an agent may start. The service then refuses the changes
//! that an Upkeep never needs and the dispatch of a workflow that is not listed, and writes
//! the marker without which the loop starts no agent
//! (`examples/cloud-worker/horizon-worker-github-upkeep`). The worker keeps the session, the
//! repositories and the bots for the parts of its policy that use them.
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::Config;
use crate::cloud_runtime::{Error, Result, command::Runner, ssh::Connection};

/// At most this many workflows an agent may start.
pub const WORKFLOW_LIMIT: usize = 20;
const COMMAND: &str = "horizon-worker-github upkeep";
const TIMEOUT: Duration = Duration::from_secs(30);

/// A workflow that an agent of the Upkeep may start with a dispatch: the repository and the
/// workflow file in its `.github/workflows`, such as `release.yml`. It runs with the
/// repository's secrets.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub repository: String,
    pub workflow: String,
}

/// Checks the workflows of `config`: at most [`WORKFLOW_LIMIT`], each in a repository of the
/// Upkeep and named by a plain `.yml` or `.yaml` file name, each one time.
pub(super) fn validate(config: &Config) -> Result<()> {
    if config.workflows.len() > WORKFLOW_LIMIT {
        return Err(Error::Invalid(
            "An Upkeep lists at most 20 workflows that an agent may start",
        ));
    }
    for (index, entry) in config.workflows.iter().enumerate() {
        if !config
            .repositories
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&entry.repository))
        {
            return Err(Error::Invalid(
                "An Upkeep workflow names a repository that is not in the list",
            ));
        }
        if !valid_workflow(&entry.workflow) {
            return Err(Error::Invalid(
                "An Upkeep workflow is the name of a .yml or .yaml file in .github/workflows",
            ));
        }
        if config.workflows[..index]
            .iter()
            .any(|held| held.repository.eq_ignore_ascii_case(&entry.repository) && held.workflow == entry.workflow)
        {
            return Err(Error::Invalid("An Upkeep workflow is listed twice"));
        }
    }
    Ok(())
}

/// The same rule as the worker's: a file name of letters, digits, `.`, `_` and `-` that ends
/// in `.yml` or `.yaml` and does not start with `.` or `-`.
fn valid_workflow(name: &str) -> bool {
    let stem = name.strip_suffix(".yml").or_else(|| name.strip_suffix(".yaml"));
    stem.is_some_and(|stem| {
        !stem.is_empty()
            && name.len() <= 100
            && !stem.starts_with(['.', '-'])
            && stem.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    })
}

/// What the worker receives: the session, the repositories, the bots and the workflows,
/// nothing else.
fn request(cloud_id: &str, config: &Config) -> Result<Vec<u8>> {
    serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "session": super::session(cloud_id),
        "repositories": config.repositories,
        "bots": config.authors,
        "workflows": config.workflows,
    }))
    .map_err(|_| Error::Invalid("The Upkeep policy could not be written"))
}

/// Sets the policy on the worker as root. Fails on a worker image without the policy; the
/// loop there starts no agent.
/// # Errors
/// The saved choice is not valid, or the worker could not be reached or refused the policy.
pub fn send(connection: &Connection, runner: &Runner<'_>, cloud_id: &str, config: &Config) -> Result<()> {
    // A saved choice edited by hand says what is wrong with it here, not on the worker.
    config.validate()?;
    let body = request(cloud_id, config)?;
    runner.private_exchange(&mut connection.command(COMMAND), &body, TIMEOUT)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let mut config = super::super::tests::config();
        config.repositories = vec!["acme/api".into(), "acme/web".into()];
        config
    }

    fn workflow(repository: &str, workflow: &str) -> Workflow {
        Workflow {
            repository: repository.into(),
            workflow: workflow.into(),
        }
    }

    #[test]
    fn listed_workflows_are_files_of_the_upkeeps_repositories() {
        let mut config = config();
        config.workflows = vec![
            workflow("ACME/api", "release.yml"),
            workflow("acme/web", "test-channel.yaml"),
        ];
        assert!(config.validate().is_ok());
        for entries in [
            vec![workflow("acme/other", "release.yml")],
            vec![workflow("acme/api", ".github/workflows/release.yml")],
            vec![workflow("acme/api", "release.sh")],
            vec![workflow("acme/api", ".yml")],
            vec![workflow("acme/api", "-x.yml")],
            vec![workflow("acme/api", "a b.yml")],
            vec![workflow("acme/api", "a.yml"), workflow("Acme/API", "a.yml")],
            (0..=WORKFLOW_LIMIT)
                .map(|n| workflow("acme/api", &format!("w{n}.yml")))
                .collect(),
        ] {
            config.workflows = entries;
            assert!(config.validate().is_err(), "{:?}", config.workflows);
        }
    }

    #[test]
    fn the_worker_receives_the_session_repositories_bots_and_workflows_only() {
        let mut config = config();
        config.workflows = vec![workflow("acme/api", "release.yml")];
        let sent: serde_json::Value = serde_json::from_slice(&request("c1", &config).unwrap()).unwrap();
        assert_eq!(
            sent,
            serde_json::json!({"version": 1, "session": "upkeep-c1", "repositories": ["acme/api", "acme/web"],
                               "bots": config.authors,
                               "workflows": [{"repository": "acme/api", "workflow": "release.yml"}]})
        );
        let saved = serde_json::to_value(super::super::tests::config()).unwrap();
        assert!(
            saved.get("workflows").is_none(),
            "an Upkeep without workflows is saved as before"
        );
    }
}
