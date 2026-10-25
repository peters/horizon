//! One bounded command in a cloud worker, answered by the running Horizon through the
//! recorded SSH route as the worker's unprivileged agent user.
use super::BrowserController;
use horizon_browser_control::manifest::provider_usage::{self, CloudExecRequest, DEFAULT_TIMEOUT_MILLIS};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloudExecInput {
    /// The cloud's ID from `cloud_list`.
    pub cloud: String,
    /// One shell command text. The worker runs it with `/bin/sh -c` as its agent user.
    pub command: String,
    /// The wall-clock bound in milliseconds, 1000 to 120000; 30000 when omitted.
    pub timeout_millis: Option<u64>,
}

impl BrowserController {
    pub(crate) async fn cloud_exec(&self, input: CloudExecInput) -> Result<Value, String> {
        let request = CloudExecRequest {
            cloud: input.cloud,
            command: input.command,
            timeout_millis: input.timeout_millis.unwrap_or(DEFAULT_TIMEOUT_MILLIS),
        };
        if !request.valid() {
            return Err(
                "cloud_exec_invalid_request: one command of at most 8192 bytes, a cloud ID from cloud_list, and a timeout of 1000 to 120000 ms"
                    .into(),
            );
        }
        // The host answers when the command ends, so the caller waits for its bound plus the
        // host's own headroom for the route to the worker.
        let wait =
            Duration::from_millis(u64::try_from(request.answer_millis().saturating_add(1_000)).unwrap_or(u64::MAX));
        let id = provider_usage::enqueue_cloud_exec(self.identity(), request).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => "cloud_exec_unavailable: requires a Horizon agent panel",
                std::io::ErrorKind::InvalidInput => {
                    "cloud_exec_invalid_request: one command of at most 8192 bytes, a cloud ID from cloud_list, and a timeout of 1000 to 120000 ms"
                }
                _ => "cloud_exec_unavailable: could not queue the request",
            }
            .to_string()
        })?;
        let started = Instant::now();
        loop {
            if let Some(result) = self
                .take_host_answer(&id)
                .map_err(|_| "cloud_exec_result_unavailable".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return result
                    .cloud_exec
                    .ok_or_else(|| "cloud_exec_result_unavailable".to_string());
            }
            if started.elapsed() >= wait {
                return Err(
                    "cloud_exec_timed_out: Horizon did not answer; a command it had started may still run in the worker"
                        .into(),
                );
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_bound_and_the_cloud_are_the_only_inputs_and_unknown_fields_are_refused() {
        let input = serde_json::from_value::<CloudExecInput>(json!({"cloud": "c", "command": "git status"})).unwrap();
        assert_eq!(
            (input.timeout_millis, input.cloud.as_str(), input.command.as_str()),
            (None, "c", "git status")
        );
        for field in ["user", "uid", "env"] {
            let mut value = json!({"cloud": "c", "command": "git status"});
            value[field] = json!("root");
            assert!(
                serde_json::from_value::<CloudExecInput>(value).is_err(),
                "{field} must not decode"
            );
        }
        let request = CloudExecRequest {
            cloud: "c".into(),
            command: "git status".into(),
            timeout_millis: DEFAULT_TIMEOUT_MILLIS,
        };
        assert_eq!(
            request.answer_millis(),
            i64::try_from(DEFAULT_TIMEOUT_MILLIS).unwrap() + provider_usage::ANSWER_HEADROOM_MILLIS
        );
    }
}
