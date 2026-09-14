//! Where a `WebDriver` session's classic transport comes from and how its
//! liveness is judged. The session code talks to the host through this seam
//! only, so a remote grid can stand in for a local driver process later
//! without touching navigation, input, capture or teardown.

use std::process::ExitStatus;

use super::service::WebDriverService;
use super::transport::ClassicTransport;

pub(super) enum DriverHost {
    /// A driver process Horizon spawned and owns on this machine.
    Local(WebDriverService),
}

impl DriverHost {
    pub(super) fn transport(&self) -> &dyn ClassicTransport {
        match self {
            Self::Local(service) => &service.http,
        }
    }

    /// The exit status once the host is gone; `None` while it is alive.
    pub(super) fn exit_status(&mut self) -> Option<ExitStatus> {
        match self {
            Self::Local(service) => service.process.child_status(),
        }
    }

    pub(super) fn has_exited(&mut self) -> bool {
        self.exit_status().is_some()
    }

    /// Best-effort session release at the host, ignoring failures.
    pub(super) fn delete_session(&self, session_id: &str) {
        match self {
            Self::Local(service) => service.delete_session(session_id),
        }
    }

    /// Stop whatever the host runs for this session.
    pub(super) fn shutdown(&mut self) {
        match self {
            Self::Local(service) => {
                let _ = service.process.kill();
            }
        }
    }
}
