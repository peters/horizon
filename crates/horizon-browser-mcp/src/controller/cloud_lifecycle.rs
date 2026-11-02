//! Cloud lifecycle requests for the running Horizon: it stops or resumes the worker of one
//! cloud in the caller's workspace when that caller's agent panel created the cloud, and
//! answers `status` for the operation ID the caller chose.
use super::BrowserController;
use horizon_browser_control::manifest::provider_usage::{self, CloudLifecycleRequest, REQUEST_DEADLINE_MILLIS};
use serde_json::Value;
use std::time::{Duration, Instant};

/// The request's deadline plus a moment for Horizon's answer to be read.
const WAIT: Duration = Duration::from_millis(REQUEST_DEADLINE_MILLIS.unsigned_abs() + 1_000);

/// Why no answer came. A stop or a resume may already have been sent, so the caller must
/// read the operation instead of sending a second one.
const TIMED_OUT: &str = "cloud_lifecycle_timed_out: Horizon did not answer; is it running? The action may already have been sent, so run cloud_list and cloud_lifecycle status with operation_id";

impl BrowserController {
    pub(crate) async fn cloud_lifecycle(&self, request: CloudLifecycleRequest) -> Result<Value, String> {
        let operation_id = request.operation_id().to_owned();
        let id = provider_usage::enqueue_cloud_lifecycle(self.identity(), request).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => "cloud_lifecycle_unavailable: requires a Horizon agent panel",
                std::io::ErrorKind::InvalidInput => {
                    "cloud_lifecycle_invalid_request: operation_id is one lowercase UUID; submit needs action stop or resume and one cloud ID"
                }
                _ => "cloud_lifecycle_unavailable: could not queue the request",
            }
            .to_string()
        })?;
        let started = Instant::now();
        loop {
            if let Some(result) = self
                .take_host_answer(&id)
                .map_err(|_| "cloud_lifecycle_result_unavailable: Horizon's answer could not be read".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return result.cloud_lifecycle.ok_or_else(|| {
                    "cloud_lifecycle_result_unavailable: Horizon's answer had no lifecycle result".to_string()
                });
            }
            if started.elapsed() >= WAIT {
                return Err(format!("{TIMED_OUT} {operation_id}"));
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
    fn an_unanswered_request_names_the_operation_that_may_be_stopping() {
        assert!(TIMED_OUT.starts_with("cloud_lifecycle_timed_out"));
        assert!(
            TIMED_OUT.contains("cloud_lifecycle status"),
            "a caller that stops waiting reads the operation instead: {TIMED_OUT}"
        );
    }

    #[test]
    fn a_submission_states_its_action_and_an_unknown_operation_is_refused() {
        let request = serde_json::from_value::<CloudLifecycleRequest>(json!({
            "operation": "submit", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5",
            "action": "stop", "cloud": "cloud-1",
        }))
        .unwrap();
        assert!(request.valid());
        assert_eq!(request.operation_id(), "27d497e2-1f54-4bc4-b746-ab2a3d9efad5");
        for invalid in [
            json!({"operation": "submit", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5", "cloud": "cloud-1"}),
            json!({"operation": "status", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5", "action": "stop"}),
        ] {
            assert!(
                !serde_json::from_value::<CloudLifecycleRequest>(invalid.clone())
                    .unwrap()
                    .valid(),
                "{invalid}"
            );
        }
        for invalid in [
            json!({"operation": "cancel", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5"}),
            json!({"operation": "submit", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5", "action": "stop", "cloud": "c", "force": true}),
        ] {
            assert!(
                serde_json::from_value::<CloudLifecycleRequest>(invalid.clone()).is_err(),
                "{invalid}"
            );
        }
    }
}
