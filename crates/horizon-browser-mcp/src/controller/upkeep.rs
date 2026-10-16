//! Upkeep requests for the running Horizon: read the Upkeep of the caller's workspace, find
//! the repositories with open bot pull requests, propose a new Upkeep or more repositories
//! for the person to confirm, and remove repositories, run now, pause or resume, as the
//! board offers them to the person.
use super::BrowserController;
use horizon_browser_control::manifest::provider_usage::{
    self, REQUEST_DEADLINE_MILLIS, UpkeepOperation, UpkeepRequest,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpkeepInput {
    /// `status` (the default) reads the Upkeep of your workspace. `discover` lists the
    /// repositories the GitHub app reaches with their open pull requests from the bots.
    /// `create` and `add` show the person New Upkeep or the board's repository list with
    /// your proposal; nothing changes until the person confirms. `remove` removes
    /// repositories. `run_now`, `pause` and `resume` change when it runs.
    #[serde(default)]
    pub operation: UpkeepOperation,
    /// GitHub `owner/name` repositories, for `create`, `add` and `remove`.
    #[serde(default)]
    pub repositories: Vec<String>,
    /// The bots: `app/dependabot` (the default), `app/renovate` or both, for `create` and
    /// `discover`.
    #[serde(default)]
    pub authors: Vec<String>,
    /// 60 (the default), 240 or 1440, for `create`.
    pub every_minutes: Option<u32>,
    /// The instructions every run starts from, for `create`; the default tells the agent to
    /// follow each repository's AGENTS.md.
    pub instructions: Option<String>,
    /// How the worker gets code, for `create`: `on_demand` (the default: GitHub search finds
    /// the pull requests and a repository is cloned only when one needs work), `partial`
    /// (every repository partially cloned) or `full` (every repository fully cloned with
    /// LFS, which can take hours and many gigabytes).
    pub checkouts: Option<String>,
}

/// The request's deadline plus a moment for Horizon's answer to be read.
const WAIT: Duration = Duration::from_millis(REQUEST_DEADLINE_MILLIS.unsigned_abs() + 1_000);

impl BrowserController {
    pub(crate) async fn upkeep(&self, input: UpkeepInput) -> Result<Value, String> {
        let request = UpkeepRequest {
            operation: input.operation,
            repositories: input.repositories,
            authors: input.authors,
            every_minutes: input.every_minutes,
            instructions: input.instructions,
            checkouts: input.checkouts,
        };
        let id = provider_usage::enqueue_upkeep(self.identity(), request).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => "upkeep_unavailable: requires a Horizon agent panel",
                std::io::ErrorKind::InvalidInput => {
                    "upkeep_invalid_request: create, add and remove need 1 to 50 owner/name repositories; authors are app/dependabot or app/renovate, for create and discover; every_minutes (60, 240 or 1440), instructions and checkouts are for create only"
                }
                _ => "upkeep_unavailable: could not queue the request",
            }
            .to_string()
        })?;
        let started = Instant::now();
        loop {
            if let Some(result) = self
                .take_host_answer(&id)
                .map_err(|_| "upkeep_result_unavailable".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return result
                    .upkeep
                    .ok_or_else(|| "upkeep_unavailable: this Horizon has no Upkeep; update Horizon".to_string());
            }
            if started.elapsed() >= WAIT {
                return Err("upkeep_timed_out: Horizon did not answer; is it running?".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_operation_defaults_to_status_and_unknown_fields_are_refused() {
        let input = serde_json::from_value::<UpkeepInput>(json!({})).unwrap();
        assert_eq!(input.operation, UpkeepOperation::Status);
        let input =
            serde_json::from_value::<UpkeepInput>(json!({"operation": "add", "repositories": ["acme/web"]})).unwrap();
        assert_eq!(
            (input.operation, input.repositories),
            (UpkeepOperation::Add, vec!["acme/web".to_owned()])
        );
        assert!(serde_json::from_value::<UpkeepInput>(json!({"workspace": "w"})).is_err());
        assert!(serde_json::from_value::<UpkeepInput>(json!({"agent": "codex"})).is_err());
    }
}
