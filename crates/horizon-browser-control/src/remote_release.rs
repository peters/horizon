//! What a finished remote teardown lets a close report.
//!
//! The driver's own release answer is one input, and the exact session's
//! record at the original provider is the other. A provider can answer the
//! release request in a way the driver cannot take as proof while its exact
//! session record already shows the session is gone, so a close reports
//! `release_failed` only when provider evidence establishes that the session
//! is still held. Both the desktop host and the standalone remote host decide
//! their close answer here, so one provider answer is never classified two
//! ways.

use horizon_browser::{RemoteRecoveryStatus, RemoteReleaseOutcome};

/// What the provider evidence established about one exact session after its
/// teardown finished.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseRelease {
    /// The provider established the release: publish `closed`.
    Established,
    /// Provider evidence still reports the exact session as held.
    Retained,
    /// Nothing establishes the release or a hold; the phrase names the cause.
    Unconfirmed(&'static str),
    /// The exact session has not been checked yet: keep the close open and
    /// check it, then decide again.
    Checking,
}

/// The typed failure a close publishes when its release is not established.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloseReleaseFailure {
    pub code: &'static str,
    pub message: String,
}

/// Cause of an unconfirmed release whose credential the provider rejected.
pub const CREDENTIAL_REJECTED: &str = "the original credential was rejected";
/// Cause of an unconfirmed release with no trustworthy provider answer.
pub const PROVIDER_UNAVAILABLE: &str = "the provider gave no trustworthy session result";
/// Cause of an unconfirmed release from an unsupported provider answer.
pub const UNSUPPORTED_RESPONSE: &str = "the provider returned an unsupported session response";
/// Cause of an unconfirmed release with no exact session identity to check.
pub const NO_IDENTITY: &str = "no exact session identity was received";
/// Cause of an unconfirmed release whose check outlived the close deadline.
pub const CHECK_PENDING: &str = "the check at the provider had not finished at the close deadline";

/// The close answer for one finished teardown. `driver` is what the release
/// request established, and `recovery` the exact session's status when this
/// host holds an allocation record for it.
///
/// The driver's proof decides first because it is the answer to the request
/// itself. Otherwise only provider evidence decides: a terminal completed
/// status establishes the release even when the release request was refused,
/// and `Active` is the one status that establishes a hold.
#[must_use]
pub const fn close_release(
    driver: Option<&RemoteReleaseOutcome>,
    recovery: Option<RemoteRecoveryStatus>,
) -> CloseRelease {
    if matches!(
        driver,
        Some(RemoteReleaseOutcome::Released | RemoteReleaseOutcome::AlreadyGone | RemoteReleaseOutcome::NeverAllocated)
    ) {
        return CloseRelease::Established;
    }
    match recovery {
        Some(RemoteRecoveryStatus::Released | RemoteRecoveryStatus::PersistenceUnavailable) => {
            CloseRelease::Established
        }
        Some(RemoteRecoveryStatus::Active) => CloseRelease::Retained,
        Some(RemoteRecoveryStatus::AuthenticationRequired) => CloseRelease::Unconfirmed(CREDENTIAL_REJECTED),
        Some(RemoteRecoveryStatus::ProviderUnavailable) => CloseRelease::Unconfirmed(PROVIDER_UNAVAILABLE),
        Some(RemoteRecoveryStatus::UnsupportedResponse) => CloseRelease::Unconfirmed(UNSUPPORTED_RESPONSE),
        Some(RemoteRecoveryStatus::Unresolved | RemoteRecoveryStatus::Reconciling | RemoteRecoveryStatus::InUse) => {
            CloseRelease::Checking
        }
        // Without a record there is no exact session to check, so nothing
        // establishes either the release or a hold.
        Some(RemoteRecoveryStatus::IdentityUnavailable) | None => CloseRelease::Unconfirmed(NO_IDENTITY),
    }
}

impl CloseRelease {
    /// The typed failure to publish, or `None` while the close must stay open
    /// (`Established` publishes `closed`, `Checking` waits for the check).
    #[must_use]
    pub fn refusal(self) -> Option<CloseReleaseFailure> {
        match self {
            Self::Established | Self::Checking => None,
            Self::Retained => Some(CloseReleaseFailure {
                code: "release_failed",
                message: "browser panel closed but the provider still reports its exact session as active; \
                          capacity remains held, so reconcile the retained allocation before creating again"
                    .to_string(),
            }),
            Self::Unconfirmed(cause) => Some(CloseReleaseFailure::unconfirmed(cause)),
        }
    }

    /// Whether the exact session must be checked at the provider before this
    /// close is answered.
    #[must_use]
    pub const fn needs_check(self) -> bool {
        matches!(self, Self::Checking)
    }
}

impl CloseReleaseFailure {
    /// The failure a close publishes when provider evidence did not establish
    /// the release, with `cause` naming why.
    #[must_use]
    pub fn unconfirmed(cause: &str) -> Self {
        Self {
            code: "release_unknown",
            message: format!(
                "browser panel closed but its release is unconfirmed: {cause}; reconcile the retained \
                 allocation before creating again"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed() -> RemoteReleaseOutcome {
        RemoteReleaseOutcome::Failed {
            error: "release refused".into(),
            message: "private provider text".into(),
        }
    }

    #[test]
    fn the_drivers_own_proof_establishes_the_release() {
        for outcome in [
            RemoteReleaseOutcome::Released,
            RemoteReleaseOutcome::AlreadyGone,
            RemoteReleaseOutcome::NeverAllocated,
        ] {
            assert_eq!(
                close_release(Some(&outcome), Some(RemoteRecoveryStatus::Active)),
                CloseRelease::Established,
                "a proven release wins over a stale status"
            );
            assert!(close_release(Some(&outcome), None).refusal().is_none());
        }
    }

    #[test]
    fn a_completed_session_record_establishes_a_release_the_driver_could_not() {
        // The provider refused the release request, and its exact session
        // record still proves the session is gone.
        for status in [
            RemoteRecoveryStatus::Released,
            RemoteRecoveryStatus::PersistenceUnavailable,
        ] {
            assert_eq!(close_release(Some(&failed()), Some(status)), CloseRelease::Established);
        }
    }

    #[test]
    fn only_a_provider_that_still_holds_the_session_reports_release_failed() {
        let retained = close_release(Some(&failed()), Some(RemoteRecoveryStatus::Active))
            .refusal()
            .expect("a held session fails the close");
        assert_eq!(retained.code, "release_failed");
        assert!(retained.message.contains("still reports its exact session as active"));
        assert!(
            !retained.message.contains("private provider text"),
            "provider text stays private"
        );
    }

    #[test]
    fn an_unconfirmed_release_names_its_cause_instead_of_a_refusal() {
        for (status, cause) in [
            (
                RemoteRecoveryStatus::AuthenticationRequired,
                "the original credential was rejected",
            ),
            (
                RemoteRecoveryStatus::ProviderUnavailable,
                "the provider gave no trustworthy session result",
            ),
            (
                RemoteRecoveryStatus::UnsupportedResponse,
                "the provider returned an unsupported session response",
            ),
            (
                RemoteRecoveryStatus::IdentityUnavailable,
                "no exact session identity was received",
            ),
        ] {
            let failure = close_release(Some(&failed()), Some(status))
                .refusal()
                .expect("an unconfirmed release is not a close");
            assert_eq!(failure.code, "release_unknown", "{status:?}");
            assert!(failure.message.contains(cause), "{}", failure.message);
            assert!(!failure.message.contains("still reports"), "{status:?} is not a hold");
        }
        let unknown = close_release(
            Some(&RemoteReleaseOutcome::ReleaseUnknown {
                attempts: 3,
                reason: "private reason".into(),
            }),
            None,
        )
        .refusal()
        .expect("an unanswered release is not a close");
        assert_eq!(unknown.code, "release_unknown");
        assert!(unknown.message.contains(NO_IDENTITY));
        assert!(!unknown.message.contains("private reason"));
    }

    #[test]
    fn a_retired_session_that_was_never_checked_is_checked_before_the_close_answers() {
        for status in [RemoteRecoveryStatus::Unresolved, RemoteRecoveryStatus::Reconciling] {
            assert!(close_release(Some(&failed()), Some(status)).needs_check());
        }
        assert!(!close_release(Some(&RemoteReleaseOutcome::Released), None).needs_check());
    }
}
