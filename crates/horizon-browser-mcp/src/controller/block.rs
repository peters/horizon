use std::time::{Duration, Instant};

use horizon_browser_control::block::parse_document;
use horizon_browser_control::manifest::block::{self, Operation, Outcome};

use super::BrowserController;

/// How long the host has to apply a card request before its deadline passes.
const HOST_TIMEOUT: Duration = Duration::from_secs(10);
/// The bound a caller waits, including the start of the coordination worker.
const CALLER_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

impl BrowserController {
    pub(crate) async fn block_panel(&self, operation: Operation) -> Result<Outcome, String> {
        let identity = self.identity();
        if !identity.workspace_scoped()
            || identity
                .host_instance
                .is_none_or(|host| !horizon_browser_control::manifest::valid_host_instance(host))
        {
            return Ok(Outcome::failed(
                "caller_unavailable",
                "block_panel requires an agent launched inside Horizon with its host identity",
            ));
        }
        if let Some(refusal) = refused_document(&operation) {
            return Ok(refusal);
        }
        let started = Instant::now();
        let request = block::enqueue(self.identity(), operation, HOST_TIMEOUT).map_err(|error| error.to_string())?;
        loop {
            if let Some(result) = block::take_result(&request).map_err(|error| error.to_string())? {
                return Ok(result);
            }
            if started.elapsed() >= CALLER_TIMEOUT {
                return Ok(Outcome::failed(
                    "host_timeout",
                    "The Horizon host did not answer within 15 seconds. List the cards before retrying a mutation; it may have completed. Older hosts do not support block_panel.",
                ));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// The document a request carries, checked before the queue holds it: a card is
/// untrusted agent text, and a refusal here names the offending path at once.
fn refused_document(operation: &Operation) -> Option<Outcome> {
    let document = match operation {
        Operation::Create { document, .. } | Operation::Update { document, .. } => document,
        Operation::List | Operation::Inspect { .. } | Operation::Visibility { .. } | Operation::Close { .. } => {
            return None;
        }
    };
    let encoded = serde_json::to_string(document).ok()?;
    parse_document(&encoded)
        .err()
        .map(|error| Outcome::failed(error.code(), &error.to_string()))
}
