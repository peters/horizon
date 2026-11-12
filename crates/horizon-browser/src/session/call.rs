//! The budget of one protocol call, and the one call kind that a far browser
//! may answer twice.
//!
//! A browser that this process started answers on a loopback socket, so five
//! seconds are more than a whole call needs. A browser that another process or
//! another computer runs answers across a link: at 350 ms of round trip a call
//! is still fine, but a busy page plus a slow link is not, so those calls hold a
//! larger budget.
//!
//! A call that timed out is sent once more only when sending it again cannot
//! change anything that the first send may have changed. The kinds come from
//! the code that sends them:
//!
//! | Kind | Methods | Sent again after a timeout |
//! |---|---|---|
//! | page query, scan and snapshot | `Runtime.evaluate` of a read script | yes |
//! | frame tree, layout metrics | `Page.getFrameTree`, `Page.getLayoutMetrics` | no |
//! | click, drag, wheel | `Input.dispatchMouseEvent` | no |
//! | key press and typed text | `Input.dispatchKeyEvent`, `Input.insertText` | no |
//! | navigation, history, reload | `Page.navigate`, `Page.reload`, `History.*` | no |
//! | file chooser, download policy, focus, viewport | `DOM.setFileInputFiles`, `Browser.setDownloadBehavior`, `Emulation.*`, `Page.bringToFront` | no |
//! | viewport screenshot | `Page.captureScreenshot` | no bound is waited on |
//!
//! A read is only ever retried, never a call of the second kind: a click that
//! the page answered after the first bound would otherwise press twice. The
//! frame tree and the layout metrics are not repeated here: the scrollbar layout
//! asks again on its own delay, and the passkey request holds a short bound on
//! purpose because the frame must still show the calling document. The viewport
//! screenshot is a request whose answer replaces the next frame, so no call waits
//! for it and there is no bound to pass.

use std::time::Duration;

use crate::cdp::CdpError;

/// The budget of one call to a browser of this computer.
pub(crate) const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// The budget of one call to a browser that another process or another computer
/// runs, which answers across a link.
pub(super) const REMOTE_CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// Whether one protocol call changes the page, which decides whether a timeout
/// may be sent again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// The call only reads the page or its browser.
    Read,
    /// The call changes the page or its browser, so a second send could do the
    /// change twice.
    Change,
}

/// One protocol call: its kind and whether its browser answers across a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Call {
    pub kind: Kind,
    /// The browser of this call runs in another process or on another computer.
    pub far: bool,
}

impl Call {
    /// How long this call may wait for its answer.
    pub(super) fn budget(self) -> Duration {
        if self.far { REMOTE_CALL_TIMEOUT } else { CALL_TIMEOUT }
    }

    /// Whether this call is sent once more after it timed out. Only a read of a
    /// far browser is: a loopback browser that needs more than its budget has a
    /// real problem, and a call that changes the page must not run twice.
    pub(super) fn repeats_after_timeout(self) -> bool {
        self.far && self.kind == Kind::Read
    }

    /// Whether `error` is a timeout that this call may answer with one more
    /// send.
    pub(super) fn retries(self, error: &CdpError) -> bool {
        matches!(error, CdpError::Timeout { .. }) && self.repeats_after_timeout()
    }

    /// The call of one session's browser: its answer crosses a link when another
    /// process or another computer runs that browser.
    pub(super) fn of(browser: &crate::session::BrowserSessionConfig, kind: Kind) -> Self {
        Self {
            kind,
            far: browser.attach.is_some() || browser.remote.is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_far_browser_holds_a_larger_budget_than_a_loopback_one() {
        assert!(REMOTE_CALL_TIMEOUT > CALL_TIMEOUT);
        assert_eq!(
            Call {
                kind: Kind::Read,
                far: false
            }
            .budget(),
            CALL_TIMEOUT
        );
        assert_eq!(
            Call {
                kind: Kind::Read,
                far: true
            }
            .budget(),
            REMOTE_CALL_TIMEOUT
        );
    }

    #[test]
    fn only_a_read_of_a_far_browser_is_sent_again_after_a_timeout() {
        let timeout = CdpError::Timeout {
            method: "Runtime.evaluate".to_string(),
        };
        let closed = CdpError::Closed {
            method: "Runtime.evaluate".to_string(),
        };
        assert!(
            Call {
                kind: Kind::Read,
                far: true
            }
            .retries(&timeout),
            "a query of a far browser is a read that is safe to repeat"
        );
        assert!(
            !Call {
                kind: Kind::Change,
                far: true
            }
            .retries(&timeout),
            "a click, a key press or a navigation is never sent twice"
        );
        assert!(
            !Call {
                kind: Kind::Read,
                far: false
            }
            .retries(&timeout),
            "a loopback browser that misses its budget is a real problem"
        );
        assert!(
            !Call {
                kind: Kind::Read,
                far: true
            }
            .retries(&closed),
            "a closed link is no timeout, and a retry needs the same link"
        );
    }
}
