//! Cloud list requests for the running Horizon: the clouds in the caller's workspace
//! with their group, status line and hourly rate, and attach, park and stop of one of
//! them, as the sidebar offers them to the person.
use super::BrowserController;
use horizon_browser_control::manifest::provider_usage::{
    self, CloudListOperation, CloudListRequest, REQUEST_DEADLINE_MILLIS,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloudListInput {
    /// `list` (the default) reads the clouds of your workspace. `attach` moves the
    /// person's view to a cloud, `park` parks the terminals of a cloud out of view and
    /// `stop` stops the worker of an idle cloud.
    #[serde(default)]
    pub operation: CloudListOperation,
    /// The cloud's ID from `list`. Required by every operation except `list`.
    pub cloud: Option<String>,
}

/// The request's deadline plus a moment for Horizon's answer to be read.
const WAIT: Duration = Duration::from_millis(REQUEST_DEADLINE_MILLIS.unsigned_abs() + 1_000);

impl BrowserController {
    pub(crate) async fn cloud_list(&self, input: CloudListInput) -> Result<Value, String> {
        let request = CloudListRequest {
            operation: input.operation,
            cloud: input.cloud,
        };
        let id = provider_usage::enqueue_cloud_list(self.identity(), request).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => "cloud_list_unavailable: requires a Horizon agent panel",
                std::io::ErrorKind::InvalidInput => {
                    "cloud_list_invalid_request: list takes no cloud; attach, park and stop need a cloud ID from list"
                }
                _ => "cloud_list_unavailable: could not queue the request",
            }
            .to_string()
        })?;
        let started = Instant::now();
        loop {
            if let Some(result) = provider_usage::take_provider_usage_result(self.identity(), &id)
                .map_err(|_| "cloud_list_result_unavailable".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return result
                    .cloud_list
                    .ok_or_else(|| "cloud_list_result_unavailable".to_string());
            }
            if started.elapsed() >= WAIT {
                return Err("cloud_list_timed_out: Horizon did not answer; is it running?".into());
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
    fn the_operation_defaults_to_list_and_unknown_fields_are_refused() {
        let input = serde_json::from_value::<CloudListInput>(json!({})).unwrap();
        assert_eq!((input.operation, input.cloud), (CloudListOperation::List, None));
        let input = serde_json::from_value::<CloudListInput>(json!({"operation": "park", "cloud": "c"})).unwrap();
        assert_eq!(input.operation, CloudListOperation::Park);
        assert!(serde_json::from_value::<CloudListInput>(json!({"workspace": "w"})).is_err());
    }
}
