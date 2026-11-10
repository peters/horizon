use std::time::{Duration, Instant};

use horizon_browser_control::manifest::device::{self, Operation, Outcome};

use super::BrowserController;

impl BrowserController {
    pub(crate) async fn device_panel(&self, operation: Operation) -> Result<Outcome, String> {
        let browser_panel = match &operation {
            Operation::BrowserScreenshot { input } => {
                let identity = self.identity();
                if !identity.workspace_scoped()
                    || identity
                        .host_instance
                        .is_none_or(|host| !horizon_browser_control::manifest::valid_host_instance(host))
                {
                    return Err(
                        "Browser screenshots require an agent launched inside Horizon with its host identity".into(),
                    );
                }
                self.ensure_claim(&input.panel_id).map_err(|error| error.to_string())?;
                Some(input.panel_id.clone())
            }
            _ => None,
        };
        let started = Instant::now();
        let mut last_heartbeat = started;
        let timeout = Duration::from_secs(10);
        let request = device::enqueue(self.identity(), operation, timeout).map_err(|error| error.to_string())?;
        loop {
            if let Some(result) = device::take_result(&request).map_err(|error| error.to_string())? {
                if let Some(panel_id) = &browser_panel {
                    self.refresh_claim(panel_id).map_err(|error| error.to_string())?;
                }
                return Ok(result);
            }
            if started.elapsed() >= timeout + Duration::from_secs(5) {
                return Ok(Outcome::failed(
                    "host_timeout",
                    "The Horizon host did not answer within 15 seconds. List panels before retrying a mutation; it may have completed. Older hosts do not support device_panel.",
                ));
            }
            if let Some(panel_id) = &browser_panel
                && last_heartbeat.elapsed() >= super::HEARTBEAT_INTERVAL
            {
                self.refresh_claim(panel_id).map_err(|error| error.to_string())?;
                last_heartbeat = Instant::now();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
