//! The terminal failures a requested browser create reports.

use horizon_core::browser::manifest::{self, BrowserCreateAuditStatus, BrowserCreateRequest, BrowserCreateResult};
use horizon_core::browser::{BackendKind, BrowserStatus};

use super::browser_create_lifecycle::PendingBrowserCreate;
use super::browser_host_io::HostIo;

/// A create that did not become controllable before its deadline.
pub(super) const CREATE_TIMEOUT: (&str, &str) = (
    "create_timeout",
    "the browser panel did not become controllable before the create deadline",
);

/// The typed, fixed-text failure a create reports for a panel that will
/// not become controllable. A remote lifecycle names its own terminal
/// outcome (the provider refused, the allocation is unknown, or the device
/// did not meet the target); provider-reported detail never reaches the
/// result.
pub(super) fn terminal_create_failure(
    browser: &horizon_core::browser::BrowserPanelState,
) -> Option<(&'static str, &'static str)> {
    match &browser.status {
        BrowserStatus::Starting | BrowserStatus::Ready => None,
        BrowserStatus::Error { .. } | BrowserStatus::Stopped { .. } => Some(browser.remote_failure().map_or_else(
            || match &browser.status {
                BrowserStatus::Error { .. } => (
                    "backend_start_failed",
                    "the selected browser backend did not start; inspect the local logs",
                ),
                _ => (
                    "backend_stopped",
                    "the selected browser backend stopped before it became controllable",
                ),
            },
            |failure| (failure.code, failure.message),
        )),
    }
}

/// Audits and publishes a failed create on the coordination worker.
pub(super) fn record_and_complete_failure(io: &mut HostIo, pending: &PendingBrowserCreate, code: &str, message: &str) {
    let result = BrowserCreateResult::failed(&pending.request, code, message);
    let (panel_local_id, request) = (pending.panel_local_id.clone(), pending.request.clone());
    let (backend, orientation) = (pending.backend, pending.startup_orientation);
    io.write(move || {
        record_create_failure(&panel_local_id, &request, backend, orientation);
        complete_result(&result);
    });
}

pub(super) fn record_create_failure(
    panel_local_id: &str,
    request: &BrowserCreateRequest,
    backend: BackendKind,
    orientation: Option<horizon_core::browser::remote::RemoteOrientation>,
) {
    if let Err(error) = manifest::record_create_status(
        panel_local_id,
        request,
        backend,
        orientation,
        BrowserCreateAuditStatus::Failed,
    ) {
        tracing::warn!(request_id = %request.request_id, %error, "could not append failed browser creation audit");
    }
}

pub(super) fn complete_result(result: &BrowserCreateResult) {
    if let Err(error) = manifest::complete_create_request(result) {
        tracing::error!(request_id = %result.request_id, %error, "could not publish browser create result");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_lifecycle_failures_report_their_own_typed_codes_with_fixed_text() {
        use horizon_core::browser::{BrowserPanelState, RemoteReleaseOutcome, RemoteSessionEvent};

        let mut rejected = BrowserPanelState::inert_remote("ios_phone", "grid");
        rejected.apply_remote_session_event_for_tests(RemoteSessionEvent::DeviceRejected {
            label: "ios_phone".into(),
            reason: "device model is Emulator 3000, target requires iPhone 16".into(),
            released: RemoteReleaseOutcome::Released,
        });
        rejected.status = BrowserStatus::Error {
            message: "remote session released at once".into(),
        };
        let (code, message) = terminal_create_failure(&rejected).expect("terminal");
        assert_eq!(code, "remote_device_rejected");
        assert!(
            !message.contains("Emulator"),
            "provider text never reaches the result: {message}"
        );

        let mut refused = BrowserPanelState::inert_remote("ios_phone", "grid");
        refused.apply_remote_session_event_for_tests(RemoteSessionEvent::AllocationFailed {
            label: "ios_phone".into(),
            reason: "session not created: no device available".into(),
            refusal: horizon_core::browser::AllocationRefusal::DeviceUnavailable,
        });
        refused.status = BrowserStatus::Stopped { code: None };
        let (code, message) = terminal_create_failure(&refused).expect("terminal");
        assert_eq!(code, "remote_device_unavailable");
        assert!(!message.contains("no device available"));

        let mut unknown = BrowserPanelState::inert_remote("ios_phone", "grid");
        unknown.apply_remote_session_event_for_tests(RemoteSessionEvent::AllocationUnknown {
            label: "ios_phone".into(),
            reason: "timed out".into(),
        });
        unknown.status = BrowserStatus::Error {
            message: "unknown".into(),
        };
        assert_eq!(
            terminal_create_failure(&unknown).map(|(code, _)| code),
            Some("remote_allocation_unknown")
        );

        let mut local = BrowserPanelState::inert();
        local.status = BrowserStatus::Error {
            message: "chrome died".into(),
        };
        assert_eq!(
            terminal_create_failure(&local).map(|(code, _)| code),
            Some("backend_start_failed")
        );
        local.status = BrowserStatus::Stopped { code: Some(1) };
        assert_eq!(
            terminal_create_failure(&local).map(|(code, _)| code),
            Some("backend_stopped")
        );
        local.status = BrowserStatus::Ready;
        assert_eq!(terminal_create_failure(&local), None);
    }
}
