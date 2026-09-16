//! Agent/user handoff: request steering, then wait until the user hands back.

use std::time::{Duration, Instant};

use horizon_browser_control::manifest;

use super::{BrowserController, ControlError, HEARTBEAT_INTERVAL};

/// Default wait for a human to finish steering (sign-in, 2FA, a consent
/// dialog). Injected MCP clients (Codex, Claude, Grok CLI) keep a tool
/// timeout above [`MAX_HANDOFF_TIMEOUT_MILLIS`] so transport can still
/// deliver this result.
pub(crate) const DEFAULT_HANDOFF_TIMEOUT_MILLIS: u64 = 900_000;
pub(crate) const MIN_HANDOFF_TIMEOUT_MILLIS: u64 = 1_000;
pub(crate) const MAX_HANDOFF_TIMEOUT_MILLIS: u64 = 3_600_000;
/// Match the driver's coordination cadence. A 20 ms action-result poll would
/// reread the manifest tens of thousands of times during a human handoff.
const HANDOFF_POLL_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) struct HandoffReceipt {
    pub(crate) panel_id: String,
    pub(crate) request_id: String,
    pub(crate) handoff_pending: bool,
    pub(crate) elapsed_millis: u64,
}

impl BrowserController {
    /// Ask the user to steer `panel_id`. When `wait` is true (the agent
    /// default), block until the live handoff is marked done (adopting a
    /// replacement request id if a later `browser_handoff` superseded this
    /// one), ownership is lost, the panel leaves the workspace, or
    /// `timeout_millis` elapses.
    pub(crate) async fn request_handoff(
        &self,
        panel_id: &str,
        reason: &str,
        wait: bool,
        timeout_millis: Option<u64>,
    ) -> Result<HandoffReceipt, ControlError> {
        let started = Instant::now();
        self.ensure_claim(panel_id)?;
        let request_id = manifest::request_handoff(panel_id, self.identity(), reason)
            .map_err(|source| self.denied(panel_id, "could not request browser handoff", source))?;
        if !wait {
            return Ok(HandoffReceipt {
                panel_id: panel_id.to_string(),
                request_id,
                handoff_pending: true,
                elapsed_millis: elapsed_millis(started),
            });
        }
        self.wait_for_handoff(panel_id, request_id, started, timeout_millis)
            .await
    }

    async fn wait_for_handoff(
        &self,
        panel_id: &str,
        request_id: String,
        started: Instant,
        timeout_millis: Option<u64>,
    ) -> Result<HandoffReceipt, ControlError> {
        let timeout_millis = bounded_handoff_timeout(timeout_millis);
        let timeout = Duration::from_millis(timeout_millis);
        let mut last_heartbeat = started;
        let mut request_id = request_id;
        loop {
            let snapshot = self.authorized_manifest(panel_id)?;
            match snapshot.handoff.as_ref() {
                Some(handoff) if !handoff.done => {
                    if !handoff.request_id.is_empty() {
                        request_id.clone_from(&handoff.request_id);
                    }
                }
                Some(handoff) => {
                    if !handoff.request_id.is_empty() {
                        request_id.clone_from(&handoff.request_id);
                    }
                    return self.finish_handoff(panel_id, request_id, started);
                }
                None => return self.finish_handoff(panel_id, request_id, started),
            }
            let now = manifest::now_millis();
            if snapshot.live_owner(now).is_none_or(|owner| owner.name != self.actor) {
                return Err(ControlError::internal_io(
                    "lost browser ownership while waiting for hand-back",
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "agent does not have a live ownership claim",
                    ),
                ));
            }
            if started.elapsed() >= timeout {
                return Err(ControlError::HandoffTimeout {
                    request_id,
                    timeout_millis,
                });
            }
            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                self.refresh_claim(panel_id)?;
                last_heartbeat = Instant::now();
            }
            tokio::time::sleep(HANDOFF_POLL_INTERVAL).await;
        }
    }

    fn finish_handoff(
        &self,
        panel_id: &str,
        request_id: String,
        started: Instant,
    ) -> Result<HandoffReceipt, ControlError> {
        if let Err(refresh_error) = self.refresh_claim(panel_id)
            && !refresh_error.is_missing_panel()
        {
            return Err(refresh_error);
        }
        Ok(HandoffReceipt {
            panel_id: panel_id.to_string(),
            request_id,
            handoff_pending: false,
            elapsed_millis: elapsed_millis(started),
        })
    }
}

fn bounded_handoff_timeout(timeout_millis: Option<u64>) -> u64 {
    timeout_millis
        .unwrap_or(DEFAULT_HANDOFF_TIMEOUT_MILLIS)
        .clamp(MIN_HANDOFF_TIMEOUT_MILLIS, MAX_HANDOFF_TIMEOUT_MILLIS)
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_timeout_is_bounded() {
        assert_eq!(bounded_handoff_timeout(None), DEFAULT_HANDOFF_TIMEOUT_MILLIS);
        assert_eq!(bounded_handoff_timeout(Some(0)), MIN_HANDOFF_TIMEOUT_MILLIS);
        assert_eq!(bounded_handoff_timeout(Some(u64::MAX)), MAX_HANDOFF_TIMEOUT_MILLIS);
        assert_eq!(
            bounded_handoff_timeout(Some(DEFAULT_HANDOFF_TIMEOUT_MILLIS)),
            DEFAULT_HANDOFF_TIMEOUT_MILLIS
        );
    }
}
