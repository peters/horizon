//! Companion cloud requests for the running Horizon. Ensure Ready and Stop answer once
//! Horizon has recorded the operation; callers poll it with the same cloud and alias
//! and its operation ID.
use super::BrowserController;
use horizon_browser_control::manifest::provider_usage::{
    self, CompanionAction, CompanionRequest, REQUEST_DEADLINE_MILLIS, new_operation_id,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompanionInput {
    /// The source cloud's ID from `cloud_companions`, in your workspace.
    pub cloud: String,
    /// The companion's alias from the source repository's `.horizon/cloud.yml`.
    pub alias: String,
    /// A UUID to reuse after a lost answer. Omit it and one is generated and returned.
    pub operation_id: Option<String>,
    /// A saved tailnet ID from `cloud_companions` for provisioning; "none" selects no network.
    pub tailnet: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompanionOperationInput {
    /// The source cloud's ID.
    pub cloud: String,
    /// The companion's alias.
    pub alias: String,
    /// The operation ID an Ensure Ready or Stop returned.
    pub operation_id: String,
}

/// The request's deadline plus a moment for Horizon's answer to be read.
const WAIT: Duration = Duration::from_millis(REQUEST_DEADLINE_MILLIS.unsigned_abs() + 1_000);

impl BrowserController {
    pub(crate) async fn cloud_companions(&self) -> Result<Value, String> {
        self.companion_request(CompanionRequest {
            action: CompanionAction::List,
            cloud: None,
            alias: None,
            operation_id: None,
            tailnet: None,
        })
        .await
    }

    pub(crate) async fn cloud_companion(
        &self,
        action: CompanionAction,
        input: CompanionInput,
    ) -> Result<Value, String> {
        let operation_id = input.operation_id.unwrap_or_else(new_operation_id);
        self.companion_request(CompanionRequest {
            action,
            cloud: Some(input.cloud),
            alias: Some(input.alias),
            operation_id: Some(operation_id.clone()),
            tailnet: input.tailnet,
        })
        .await
        .map_err(|error| {
            if error.starts_with("cloud_companion_timed_out") {
                // Horizon may have recorded it; the ID lets the caller find out.
                format!("{error}; poll cloud_companion_operation with the same cloud and alias and operation_id {operation_id}")
            } else {
                error
            }
        })
    }

    pub(crate) async fn cloud_companion_operation(&self, input: CompanionOperationInput) -> Result<Value, String> {
        self.companion_request(CompanionRequest {
            action: CompanionAction::Status,
            cloud: Some(input.cloud),
            alias: Some(input.alias),
            operation_id: Some(input.operation_id),
            tailnet: None,
        })
        .await
    }

    async fn companion_request(&self, request: CompanionRequest) -> Result<Value, String> {
        let id = provider_usage::enqueue_cloud_companion(self.identity(), request).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => "cloud_companion_unavailable: requires a Horizon agent panel",
                std::io::ErrorKind::InvalidInput => {
                    "cloud_companion_invalid_request: cloud and alias are IDs, and operation_id is a UUID"
                }
                _ => "cloud_companion_unavailable: could not queue the request",
            }
            .to_string()
        })?;
        let started = Instant::now();
        loop {
            if let Some(result) = provider_usage::take_provider_usage_result(self.identity(), &id)
                .map_err(|_| "cloud_companion_result_unavailable".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return result
                    .companion
                    .ok_or_else(|| "cloud_companion_result_unavailable".to_string());
            }
            if started.elapsed() >= WAIT {
                return Err("cloud_companion_timed_out: Horizon did not answer; is it running?".into());
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
    fn companion_inputs_reject_unknown_fields() {
        assert!(serde_json::from_value::<CompanionInput>(json!({"cloud": "a", "alias": "b", "worker": "c"})).is_err());
        let input = serde_json::from_value::<CompanionInput>(json!({"cloud": "a", "alias": "b"})).unwrap();
        assert!(input.operation_id.is_none());
        assert!(serde_json::from_value::<CompanionOperationInput>(json!({"cloud": "a", "alias": "b"})).is_err());
    }
}
