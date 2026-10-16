//! Reads the running Horizon's answers to queued requests.
use super::BrowserController;
use horizon_browser_control::manifest::provider_usage::{self, UsageResult};

impl BrowserController {
    /// Take Horizon's answer to request `id`, or `None` while it is not ready.
    ///
    /// # Errors
    /// Result storage is unavailable or does not match the caller's identity.
    pub(super) fn take_host_answer(&self, id: &str) -> std::io::Result<Option<UsageResult>> {
        answer_still_coming_on_lock_timeout(provider_usage::take_provider_usage_result(self.identity(), id))
    }
}

/// Horizon holds the queue lock while it writes an answer, which a slow disk can stretch
/// past the lock wait: the answer is still coming, so the caller polls again.
fn answer_still_coming_on_lock_timeout<T>(taken: std::io::Result<Option<T>>) -> std::io::Result<Option<T>> {
    match taken {
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => Ok(None),
        taken => taken,
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind};

    use super::answer_still_coming_on_lock_timeout;

    #[test]
    fn a_queue_lock_timeout_reads_as_an_answer_still_coming() {
        let timed_out = Err::<Option<u8>, _>(Error::new(ErrorKind::TimedOut, "timed out waiting for manifest lock"));
        assert_eq!(answer_still_coming_on_lock_timeout(timed_out).ok(), Some(None));
    }

    #[test]
    fn other_take_outcomes_pass_through() {
        assert_eq!(answer_still_coming_on_lock_timeout(Ok(Some(7))).ok(), Some(Some(7)));
        assert_eq!(answer_still_coming_on_lock_timeout(Ok(None::<u8>)).ok(), Some(None));
        let mismatch = Err::<Option<u8>, _>(Error::new(ErrorKind::PermissionDenied, "identity mismatch"));
        assert_eq!(
            answer_still_coming_on_lock_timeout(mismatch).map_err(|error| error.kind()),
            Err(ErrorKind::PermissionDenied)
        );
    }
}
