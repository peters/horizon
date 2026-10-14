//! Owner-fulfilled metadata requests. Secret entry stays in the native editor.
use super::BrowserController;
use horizon_browser_control::manifest::provider_usage::{self, CredentialRequest, REQUEST_DEADLINE_MILLIS};
use serde_json::Value;
use std::time::{Duration, Instant};
impl BrowserController {
    pub(crate) async fn tailnet_api_credential(&self, request: CredentialRequest) -> Result<Value, String> {
        let id = provider_usage::enqueue_tailnet_credential(self.identity(), request).map_err(|_| {
            "tailnet_credential_unavailable: requires valid metadata and a live Horizon agent panel".to_string()
        })?;
        let start = Instant::now();
        loop {
            if let Some(result) = provider_usage::take_provider_usage_result(self.identity(), &id)
                .map_err(|_| "tailnet_credential_result_unavailable".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return result
                    .tailnet_credential
                    .ok_or_else(|| "tailnet_credential_result_unavailable".into());
            }
            if start.elapsed() >= Duration::from_millis(REQUEST_DEADLINE_MILLIS.unsigned_abs() + 1000) {
                return Err(
                    "tailnet_credential_timed_out: read status with the same request_id; timeout does not cancel"
                        .into(),
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
