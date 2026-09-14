//! Where a `WebDriver` session's classic transport comes from and how its
//! liveness is judged. The session code talks to the host through this seam
//! only, so a remote grid stands in for a local driver process without
//! touching navigation, input, capture or teardown.

use std::process::ExitStatus;
use std::time::Instant;

use super::remote::{RemoteExpiry, RemoteHost, RemoteReleaseOutcome};
use super::service::WebDriverService;
use super::transport::ClassicTransport;

pub(super) enum DriverHost {
    /// A driver process Horizon spawned and owns on this machine.
    Local(WebDriverService),
    /// A session at a remote grid, bounded by the local watchdog.
    Remote(RemoteHost),
}

/// Why the host is no longer usable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HostExit {
    Process(ExitStatus),
    Expired(RemoteExpiry),
}

impl HostExit {
    pub(super) fn code(self) -> Option<i32> {
        match self {
            Self::Process(status) => status.code(),
            Self::Expired(_) => None,
        }
    }
}

impl DriverHost {
    pub(super) fn transport(&self) -> &dyn ClassicTransport {
        match self {
            Self::Local(service) => &service.http,
            Self::Remote(host) => host.transport(),
        }
    }

    pub(super) fn is_remote(&self) -> bool {
        matches!(self, Self::Remote(_))
    }

    pub(super) fn remote(&mut self) -> Option<&mut RemoteHost> {
        match self {
            Self::Local(_) => None,
            Self::Remote(host) => Some(host),
        }
    }

    /// The reason the host is gone; `None` while it is alive.
    pub(super) fn exit(&mut self, now: Instant) -> Option<HostExit> {
        match self {
            Self::Local(service) => service.process.child_status().map(HostExit::Process),
            Self::Remote(host) => host.check_expiry(now).map(HostExit::Expired),
        }
    }

    pub(super) fn has_exited(&mut self) -> bool {
        self.exit(Instant::now()).is_some()
    }

    /// Release the session at the host. Remote hosts report what the
    /// provider established; local drivers are killed right after.
    pub(super) fn release(&mut self, session_id: &str) -> Option<RemoteReleaseOutcome> {
        match self {
            Self::Local(service) => {
                service.delete_session(session_id);
                let _ = service.process.kill();
                None
            }
            Self::Remote(host) => Some(host.release(session_id)),
        }
    }

    /// Best-effort session delete during startup failure paths.
    pub(super) fn delete_session(&mut self, session_id: &str) {
        let _ = self.release(session_id);
    }
}
