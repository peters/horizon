//! Progress guards and host-side queue polling shared by the CLI integration tests.

use std::time::Duration;

use horizon_browser_control::manifest::provider_usage::{UsageQueue, UsageRequest};

/// Polls for job progress return as soon as the job reaches the awaited state; this bound
/// only turns a hung job into a failure, so it must exceed what a healthy job can take.
/// The first launch of a freshly linked binary can take seconds on Windows while it is
/// scanned, and after a deadline every durable write may retry a blocked replace for up to
/// two seconds before the job exits.
pub const JOB_PROGRESS_GUARD: Duration = Duration::from_mins(1);

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
