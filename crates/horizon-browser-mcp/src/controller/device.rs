use std::time::{Duration, Instant};

use horizon_browser_control::manifest::device::{self, Operation, Outcome};

use super::BrowserController;

impl BrowserController {
    pub(crate) async fn device_panel(&self, operation: Operation) -> Result<Outcome, String> {
        let timeout = Duration::from_secs(10);
        let request = device::enqueue(self.identity(), operation, timeout).map_err(|error| error.to_string())?;
        let started = Instant::now();
        loop {
            if let Some(result) = device::take_result(&request).map_err(|error| error.to_string())? {
                return Ok(result);
            }
            if started.elapsed() >= timeout + Duration::from_secs(5) {
                return Ok(Outcome::failed(
                    "host_timeout",
                    "The Horizon host did not answer within 15 seconds. List panels before retrying a mutation; it may have completed. Older hosts do not support device_panel.",
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
