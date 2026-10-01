use std::time::{Duration, Instant};

use horizon_browser_control::manifest::agent_panels::{self, Operation, Outcome};

use super::BrowserController;

impl BrowserController {
    pub(crate) async fn agent_panels(&self, operation: Operation) -> Result<Outcome, String> {
        let started = Instant::now();
        let timeout = Duration::from_secs(10);
        let request = agent_panels::enqueue(self.identity(), operation, timeout).map_err(|error| error.to_string())?;
        loop {
            if let Some(result) = agent_panels::take_result(&request).map_err(|error| error.to_string())? {
                return Ok(result);
            }
            if started.elapsed() >= timeout + Duration::from_secs(5) {
                return Ok(Outcome::failed(
                    "host_timeout",
                    "The Horizon host did not answer within 15 seconds. List the agents before retrying a send; it may have gone through. Older hosts do not support agent_panels.",
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
