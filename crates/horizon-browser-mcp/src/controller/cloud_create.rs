//! Cloud create requests for the running Horizon: it rents a quick-start cloud in the
//! caller's workspace when the machine's policy admits it, and answers `status` and
//! `cancel` for the operation ID the caller chose.
use super::BrowserController;
use horizon_browser_control::manifest::provider_usage::{self, CloudCreateRequest, REQUEST_DEADLINE_MILLIS};
use serde_json::Value;
use std::time::{Duration, Instant};

/// The request's deadline plus a moment for Horizon's answer to be read.
const WAIT: Duration = Duration::from_millis(REQUEST_DEADLINE_MILLIS.unsigned_abs() + 1_000);

/// Why no answer came. Horizon may have rented the worker before it stopped answering, so
/// the caller must read the cloud list instead of assuming that nothing exists.
const TIMED_OUT: &str = "cloud_create_timed_out: Horizon did not answer; is it running? A cloud may have been rented, so run cloud_list and stop it if it exists, then poll cloud_create status with operation_id";

impl BrowserController {
    pub(crate) async fn cloud_create(&self, request: CloudCreateRequest) -> Result<Value, String> {
        let operation_id = request.operation_id().to_owned();
        let id = provider_usage::enqueue_cloud_create(self.identity(), request).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => "cloud_create_unavailable: requires a Horizon agent panel",
                std::io::ErrorKind::InvalidInput => {
                    "cloud_create_invalid_request: operation_id is one lowercase UUID; submit needs a repository and quick_start true"
                }
                _ => "cloud_create_unavailable: could not queue the request",
            }
            .to_string()
        })?;
        let started = Instant::now();
        loop {
            if let Some(result) = self
                .take_host_answer(&id)
                .map_err(|_| "cloud_create_result_unavailable: Horizon's answer could not be read".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return result.cloud_create.ok_or_else(|| {
                    "cloud_create_result_unavailable: Horizon's answer had no create result".to_string()
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
    fn an_unanswered_request_names_the_cloud_that_may_exist() {
        assert!(TIMED_OUT.starts_with("cloud_create_timed_out"));
        assert!(
            TIMED_OUT.contains("cloud_list") && TIMED_OUT.contains("may have been rented"),
            "a caller that stops waiting must read the cloud list: {TIMED_OUT}"
        );
    }

    #[test]
    fn a_submission_states_its_repository_and_quick_start_and_an_unknown_operation_is_refused() {
        let request = serde_json::from_value::<CloudCreateRequest>(json!({
            "operation": "submit", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5",
            "repository": "/home/person/shop", "quick_start": true,
        }))
        .unwrap();
        assert!(request.valid());
        assert_eq!(request.operation_id(), "27d497e2-1f54-4bc4-b746-ab2a3d9efad5");
        let status = serde_json::from_value::<CloudCreateRequest>(json!({
            "operation": "status", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5",
        }))
        .unwrap();
        assert!(status.valid());
        for invalid in [
            json!({"operation": "submit", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5"}),
            json!({"operation": "submit", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5", "repository": "/home/person/shop"}),
            json!({"operation": "status", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5", "name": "shop"}),
        ] {
            assert!(
                !serde_json::from_value::<CloudCreateRequest>(invalid.clone())
                    .unwrap()
                    .valid(),
                "{invalid}"
            );
        }
        for invalid in [
            json!({"operation": "start", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5"}),
            json!({"operation": "status", "operation_id": "27d497e2-1f54-4bc4-b746-ab2a3d9efad5", "provider": "hetzner"}),
        ] {
            assert!(
                serde_json::from_value::<CloudCreateRequest>(invalid.clone()).is_err(),
                "{invalid}"
            );
        }
    }
}
