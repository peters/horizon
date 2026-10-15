//! Host-scoped image destination actions; secrets never cross the public queue.
use super::BrowserController;
use horizon_browser_control::manifest::provider_usage::{
    self, ImageDestinationAction, ImageDestinationRequest, REQUEST_DEADLINE_MILLIS,
};
use serde_json::Value;
use std::time::{Duration, Instant};
impl BrowserController {
    pub(crate) async fn cloud_image_destination(&self, mut request: ImageDestinationRequest) -> Result<Value, String> {
        let authentication = matches!(request.action, ImageDestinationAction::Authenticate { .. });
        let start = Instant::now();
        let mut status = self.image_destination_reply(request.clone()).await?;
        if authentication {
            request.profile = None;
            request.action = ImageDestinationAction::Status {};
            while status["done"] == false && status["sign_in"].is_null() && start.elapsed() < Duration::from_secs(10) {
                tokio::time::sleep(Duration::from_millis(100)).await;
                status = self.image_destination_reply(request.clone()).await?;
            }
        }
        Ok(status)
    }

    async fn image_destination_reply(&self, request: ImageDestinationRequest) -> Result<Value, String> {
        let id = provider_usage::enqueue_image_destination(self.identity(), request).map_err(|_| {
            "image_destination_unavailable: requires valid metadata and a live Horizon agent panel".to_string()
        })?;
        let start = Instant::now();
        loop {
            if let Some(result) = provider_usage::take_provider_usage_result(self.identity(), &id)
                .map_err(|_| "image_destination_result_unavailable".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return result
                    .image_destination
                    .ok_or_else(|| "image_destination_result_unavailable".into());
            }
            if start.elapsed() >= Duration::from_millis(REQUEST_DEADLINE_MILLIS.unsigned_abs() + 1000) {
                return Err(
                    "image_destination_timed_out: read status with the same operation_id; timeout does not cancel"
                        .into(),
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
