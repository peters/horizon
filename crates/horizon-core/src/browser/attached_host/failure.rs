//! Why a panel that follows a browser host could not attach to its browser.
//!
//! Every reason is a typed kind and a message that keeps the words of `ssh` and
//! of the start snippet, because a refused host key, a missing Chromium and a
//! browser that wrote no endpoint need different work from the person who reads
//! the panel.

use horizon_browser_control::ssh_forward::SshForwardError;

use super::AttachedHost;

/// The exit code of the start snippet when that machine has no Chromium, and
/// when the browser of that machine wrote no endpoint. They are the exit codes
/// of [`horizon_browser_control::ssh_forward::remote_browser_command`].
const NO_BROWSER_EXIT: i32 = 3;
const NO_ENDPOINT_EXIT: i32 = 5;

/// The kind of a connect that could not reach the browser of another machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachedHostFailureKind {
    /// This computer did not reach the host, or `ssh` refused it. The reason
    /// keeps the words of `ssh`, which name an unknown host key or a refused
    /// password.
    Unreachable,
    /// That machine has no Chromium program to start.
    NoBrowser,
    /// The browser of that machine wrote no endpoint within its bound.
    NotStarted,
    /// The endpoint of that browser gave no answer through the forward.
    NoEndpoint,
    /// The browser answered when the panel connected and then stopped, so the
    /// page of that panel lost its connection.
    ConnectionLost,
    /// That machine has no `horizon` program, or not this one, so the driver of
    /// its browser never started.
    NoDriver,
    /// The driver on that machine speaks another version of the link, or is
    /// another Horizon build.
    NotThisVersion,
    /// The driver runs on another system or CPU, so a copy of this computer's
    /// binary is not the program that runs there.
    OtherSystem,
    /// The driver of that machine refused the link in its own words: its
    /// browser, its driver program or its profile needs work there.
    Refused,
}

impl AttachedHostFailureKind {
    /// The typed code of this kind, for a manifest and for a log.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unreachable => "attached_host_unreachable",
            Self::NoBrowser => "attached_host_no_browser",
            Self::NotStarted => "attached_host_not_started",
            Self::NoEndpoint => "attached_host_no_endpoint",
            Self::ConnectionLost => "attached_host_connection_lost",
            Self::NoDriver => "attached_host_no_driver",
            Self::NotThisVersion => "attached_host_version",
            Self::OtherSystem => "attached_host_system",
            Self::Refused => "attached_host_refused",
        }
    }
}

/// Why a panel that follows a browser host could not attach to its browser, with
/// the words of the host when it gave any.
///
/// [`Self::message`] is the sentence that the panel shows to the person: it names
/// the machine, the profile and the work that is left. [`Self::detail`] keeps the
/// words of `ssh` or of the browser engine, which name a refused host key or a
/// lost connection. The detail goes to the log and to the `failure` field of an
/// agent, and the panel keeps the plain sentence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachedHostFailure {
    kind: AttachedHostFailureKind,
    message: String,
    detail: Option<String>,
}

impl AttachedHostFailure {
    /// The kind of the failure.
    #[must_use]
    pub const fn kind(&self) -> AttachedHostFailureKind {
        self.kind
    }

    /// The typed code of the failure.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.kind.code()
    }

    /// What the panel shows: one sentence that names the machine, the profile
    /// and what the person can do next.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The words of `ssh`, of the start snippet or of the browser engine, when it
    /// gave any. The log and the reason of an agent carry them; the panel does
    /// not.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// What an agent reads as the failure of this panel: the sentence that the
    /// person sees, and then the words of the tool, so an agent can act on both.
    #[must_use]
    pub fn report(&self) -> String {
        match self.detail.as_deref() {
            Some(detail) => format!("{} ({detail})", self.message),
            None => self.message.clone(),
        }
    }

    pub(crate) fn unreachable(host: &AttachedHost, reason: &str) -> Self {
        Self {
            kind: AttachedHostFailureKind::Unreachable,
            detail: None,
            message: format!(
                "the browser host {} could not be reached over SSH: {reason}; check the SSH target, then use Retry",
                host.ssh_target()
            ),
        }
    }

    fn no_browser(host: &AttachedHost, reason: &str) -> Self {
        Self {
            kind: AttachedHostFailureKind::NoBrowser,
            detail: None,
            message: format!(
                "{} has no Chromium program to start: {reason}; install Chromium or Google Chrome on that machine, then use Retry",
                host.ssh_target()
            ),
        }
    }

    pub(super) fn not_started(host: &AttachedHost, reason: &str) -> Self {
        Self {
            kind: AttachedHostFailureKind::NotStarted,
            detail: None,
            message: format!(
                "the browser of {} in the profile {} wrote no DevTools endpoint: {reason}; check that machine, then use Retry",
                host.ssh_target(),
                host.profile()
            ),
        }
    }

    pub(super) fn no_endpoint(host: &AttachedHost, reason: &str) -> Self {
        Self {
            kind: AttachedHostFailureKind::NoEndpoint,
            detail: None,
            message: format!(
                "the browser of {} in the profile {} gave no answer on its DevTools port through the forward: {reason}; use Retry",
                host.ssh_target(),
                host.profile()
            ),
        }
    }

    /// The endpoint of that browser answered nothing inside its bound. The
    /// browser may be well; the forward it answers through is what took too
    /// long, so this sentence names the link and says what to expect.
    pub(super) fn no_endpoint_too_slow(host: &AttachedHost, seconds: u64, reason: &str) -> Self {
        Self {
            kind: AttachedHostFailureKind::NoEndpoint,
            detail: None,
            message: format!(
                "the link to {} did not carry an answer from the DevTools port of its browser in the profile {} within {seconds} {}: {reason}; a slow link or a busy machine delays that answer, so use Retry",
                host.ssh_target(),
                host.profile(),
                if seconds == 1 { "second" } else { "seconds" }
            ),
        }
    }

    /// The failure of a panel whose browser answered and then stopped. The
    /// panel says one plain sentence, because the words of a lost WebSocket do
    /// not help the person; the log and an agent keep those words.
    pub(crate) fn connection_lost(host: &AttachedHost, reason: &str) -> Self {
        Self {
            kind: AttachedHostFailureKind::ConnectionLost,
            message: format!(
                "the browser of {} in the profile {} lost its connection, so this panel keeps its last page; Retry connects again",
                host.ssh_target(),
                host.profile()
            ),
            detail: Some(reason.to_owned()),
        }
    }

    /// The failure of one `ssh` step of a connect.
    pub(super) fn from_ssh(host: &AttachedHost, error: &SshForwardError) -> Self {
        match error {
            SshForwardError::RemoteCommandFailed {
                code: Some(code),
                reason,
                ..
            } => match *code {
                NO_BROWSER_EXIT => Self::no_browser(host, reason),
                NO_ENDPOINT_EXIT => Self::not_started(host, reason),
                _ => Self::unreachable(host, reason),
            },
            SshForwardError::InvalidTarget { .. }
            | SshForwardError::InvalidPort { .. }
            | SshForwardError::Spawn { .. }
            | SshForwardError::ForwardEnded { .. }
            | SshForwardError::NotListening { .. }
            | SshForwardError::RemoteCommandFailed { .. }
            | SshForwardError::RemoteNoAnswer { .. } => Self::unreachable(host, &error.to_string()),
        }
    }

    /// The failure of a driver link whose command on the host is no `horizon`:
    /// `ssh` answers with the exit code of a shell that found no program.
    pub(crate) fn no_driver(host: &AttachedHost, reason: &str) -> Self {
        Self {
            kind: AttachedHostFailureKind::NoDriver,
            message: format!(
                "{} runs no horizon for the driver of its browser in the profile {}: {reason}; copy the horizon binary of this computer to that machine, or name its path in Settings, then use Retry",
                host.ssh_target(),
                host.profile()
            ),
            detail: None,
        }
    }

    /// The failure of a link whose other end speaks another version of it.
    pub(crate) fn version_mismatch(host: &AttachedHost, protocol: u32, host_version: &str) -> Self {
        Self {
            kind: AttachedHostFailureKind::NotThisVersion,
            message: format!(
                "the driver of {} in the profile {} is Horizon {host_version} and speaks version {protocol} of the browser host link, and this computer speaks version {}: copy the horizon binary of this computer to that machine, then use Retry",
                host.ssh_target(),
                host.profile(),
                super::HOST_LINK_VERSION
            ),
            detail: None,
        }
    }

    /// The failure of a link whose driver runs on another system or CPU.
    pub(crate) fn other_system(_host: &AttachedHost, words: &str) -> Self {
        Self {
            kind: AttachedHostFailureKind::OtherSystem,
            message: format!("{words}; then use Retry"),
            detail: None,
        }
    }

    /// The failure of a driver of that machine that refused the link in its own
    /// words.
    pub(crate) fn refused(host: &AttachedHost, code: &str, message: &str) -> Self {
        Self {
            kind: AttachedHostFailureKind::Refused,
            message: format!(
                "the driver of {} in the profile {} refused the link: {message}; fix that on the machine of the host, then use Retry",
                host.ssh_target(),
                host.profile()
            ),
            detail: Some(format!("{code}: {message}")),
        }
    }

    /// The failure of a connect whose worker ended without an outcome.
    pub(crate) fn closed(host: &AttachedHost) -> Self {
        Self::unreachable(host, "the connect ended before it named a browser")
    }
}
