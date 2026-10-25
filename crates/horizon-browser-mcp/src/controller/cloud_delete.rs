//! Cloud delete requests for the running Horizon: it deletes one cloud's provider
//! resources in the caller's workspace when that caller's agent panel created the cloud,
//! and answers `status` and `cancel` for the operation ID the caller chose.
use super::BrowserController;
use horizon_browser_control::manifest::provider_usage::{self, CloudDeleteRequest, REQUEST_DEADLINE_MILLIS};
use serde_json::Value;
use std::time::{Duration, Instant};

/// The request's deadline plus a moment for Horizon's answer to be read.
const WAIT: Duration = Duration::from_millis(REQUEST_DEADLINE_MILLIS.unsigned_abs() + 1_000);

/// Why no answer came. The delete may already have started, so the caller must read the
/// operation instead of sending a second one.
const TIMED_OUT: &str = "cloud_delete_timed_out: Horizon did not answer; is it running? A delete may already have started, so run cloud_list and cloud_delete status with operation_id";

impl BrowserController {
    pub(crate) async fn cloud_delete(&self, request: CloudDeleteRequest) -> Result<Value, String> {
        let operation_id = request.operation_id().to_owned();
        let id = provider_usage::enqueue_cloud_delete(self.identity(), request).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => "cloud_delete_unavailable: requires a Horizon agent panel",
                std::io::ErrorKind::InvalidInput => {
                    "cloud_delete_invalid_request: operation_id is one lowercase UUID; submit needs one cloud id"
                }
                _ => "cloud_delete_unavailable: could not queue the request",
            }
            .to_string()
        })?;
        let started = Instant::now();
        loop {
            if let Some(result) = self
                .take_host_answer(&id)
                .map_err(|_| "cloud_delete_result_unavailable: Horizon's answer could not be read".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return result.cloud_delete.ok_or_else(|| {
                    "cloud_delete_result_unavailable: Horizon's answer had no delete result".to_string()
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
    fn an_unanswered_request_names_the_operation_that_may_be_deleting() {
        assert!(TIMED_OUT.starts_with("cloud_delete_timed_out"));
        assert!(
            TIMED_OUT.contains("may already have started") && TIMED_OUT.contains("status"),
            "a caller that stops waiting must read the operation, never delete again: {TIMED_OUT}"
        );
    }

    #[test]
    fn a_submission_states_its_cloud_and_an_unknown_operation_is_refused() {
        let request = serde_json::from_value::<CloudDeleteRequest>(json!({
            "operation": "submit", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5",
            "cloud": "cloud-1",
        }))
        .unwrap();
        assert!(request.valid());
        assert_eq!(request.operation_id(), "27d497e2-1f54-4bc4-b746-ab2a3d9efad5");
        for invalid in [
            json!({"operation": "submit", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5"}),
            json!({"operation": "status", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5", "cloud": "cloud-1"}),
        ] {
            assert!(
                !serde_json::from_value::<CloudDeleteRequest>(invalid.clone())
                    .unwrap()
                    .valid(),
                "{invalid}"
            );
        }
        for invalid in [
            json!({"operation": "remove", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5"}),
            json!({"operation": "submit", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5", "cloud": "c", "force": true}),
        ] {
            assert!(
                serde_json::from_value::<CloudDeleteRequest>(invalid.clone()).is_err(),
                "{invalid}"
            );
        }
    }
}
