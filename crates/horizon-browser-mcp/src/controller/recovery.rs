use super::BrowserController;
use horizon_browser_control::manifest::recovery;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

impl BrowserController {
    pub(crate) async fn remote_allocations(&self, reference: Option<String>) -> Result<Value, String> {
        self.require_host_instance()
            .map_err(|_| "remote recovery requires a Horizon host identity".to_string())?;
        let id = recovery::enqueue_recovery(self.identity(), reference)
            .map_err(|_| "could not queue remote recovery request".to_string())?;
        let started = Instant::now();
        loop {
            if let Some(result) = recovery::take_recovery_result(self.identity(), &id)
                .map_err(|_| "could not read remote recovery result".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return Ok(json!({"allocations": result.allocations}));
            }
            if started.elapsed() >= Duration::from_secs(20) {
                return Err(
                    "remote recovery timed out; capacity remains held unless exact release was established".into(),
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
