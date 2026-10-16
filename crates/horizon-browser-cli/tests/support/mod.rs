//! Host-side queue polling shared by the CLI integration tests.

use horizon_browser_control::manifest::provider_usage::{UsageQueue, UsageRequest};

/// Claims this host's requests the way the Horizon host polls them: a claim that
/// times out on the queue lock, which the CLI holds while it publishes or takes a
/// request, is an empty poll, not a failure.
pub fn claim_host_requests(queue: &UsageQueue, host: &str) -> Vec<UsageRequest> {
    match queue.claim(host) {
        Ok(requests) => requests,
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => Vec::new(),
        Err(error) => panic!("host queue: {error}"),
    }
}
