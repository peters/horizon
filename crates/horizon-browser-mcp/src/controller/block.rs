use std::time::{Duration, Instant};

use horizon_browser_control::block::parse_document;
use horizon_browser_control::manifest::block::{self, Operation, Outcome};
use horizon_browser_control::manifest::block_terminal;

use super::BrowserController;

/// The bound a caller waits beyond the wait the operation itself asks for, as
/// the start of the coordination worker and the answer write are included.
const CALLER_HEADROOM: Duration = Duration::from_secs(5);
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
        if let Some(refusal) = refused_terminal(&operation) {
            return Ok(refusal);
        }
        // A `next_click` holds its request open for the wait it asks for, so
        // both the request deadline and the caller's bound grow with it.
        let wait = operation
            .wait_timeout_millis()
            .map_or(Duration::ZERO, Duration::from_millis);
        let caller_timeout = block::HOST_APPLY_TIMEOUT + wait + CALLER_HEADROOM;
        let started = Instant::now();
        let request_timeout = block::request_timeout(&operation);
        let request = block::enqueue(self.identity(), operation, request_timeout).map_err(|error| error.to_string())?;
        loop {
            if let Some(result) = block::take_result(&request).map_err(|error| error.to_string())? {
                return Ok(result);
            }
            if started.elapsed() >= caller_timeout {
                return Ok(Outcome::failed(
                    "host_timeout",
                    &format!(
                        "The Horizon host did not answer within {} seconds. List the cards before retrying a mutation; it may have completed. Older hosts do not support block_panel.",
                        caller_timeout.as_secs()
                    ),
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
        Operation::List
        | Operation::Inspect { .. }
        | Operation::NextClick { .. }
        | Operation::Visibility { .. }
        | Operation::Close { .. }
        | Operation::OpenTerminal { .. } => {
            return None;
        }
    };
    let encoded = serde_json::to_string(document).ok()?;
    parse_document(&encoded)
        .err()
        .map(|error| Outcome::failed(error.code(), &error.to_string()))
}

/// The route and the remote command of an `open_terminal`, checked before the
/// queue holds them: the answer names the field at once and no request file is
/// written for a route that Horizon would refuse.
fn refused_terminal(operation: &Operation) -> Option<Outcome> {
    let Operation::OpenTerminal {
        ssh, remote_command, ..
    } = operation
    else {
        return None;
    };
    block_terminal::check(ssh, remote_command.as_deref())
        .err()
        .map(|refusal| Outcome::failed(refusal.code, &refusal.message))
}
