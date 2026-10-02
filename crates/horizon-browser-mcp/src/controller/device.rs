use std::time::{Duration, Instant};

use horizon_browser_control::manifest::device::{self, Operation, Outcome};

use super::BrowserController;

impl BrowserController {
    pub(crate) async fn device_panel(&self, operation: Operation) -> Result<Outcome, String> {
        let started = Instant::now();
        let timeout = Duration::from_secs(10);
        let request = device::enqueue(self.identity(), operation, timeout).map_err(|error| error.to_string())?;
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

impl BrowserController {
    pub(crate) async fn cast(
        &self,
        operation: horizon_browser_control::manifest::cast::CastOperation,
    ) -> Result<horizon_browser_control::manifest::cast::CastOutcome, String> {
        use horizon_browser_control::manifest::cast;
        if !cfg!(target_os = "linux") {
            return Ok(cast::CastOutcome::failed("Casting is supported on Linux only"));
        }
        let request =
            cast::enqueue(self.identity(), operation, Duration::from_secs(10)).map_err(|error| error.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(result) = cast::take_result(&request).map_err(|error| error.to_string())? {
                return Ok(result);
            }
            if Instant::now() >= deadline {
                return Ok(cast::CastOutcome::failed(
                    "Host did not answer. Inspect status before retrying; the operation may have completed.",
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
