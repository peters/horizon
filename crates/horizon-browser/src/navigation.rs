//! Backend-neutral bookkeeping for a navigation action whose outcome is
//! settled later by page events rather than by the command acknowledgement.
//!
//! Both drivers dispatch the navigation immediately, keep one
//! [`PendingNavigation`] per session, feed it the signals they already
//! observe (commit, `DOMContentLoaded`, load, same-document change, failure,
//! title), and complete the action as soon as it reports a result or its
//! bound elapses.

use std::time::{Duration, Instant};

use crate::{
    AgentAction, BrowserControlFailure, BrowserControlValue, NavigationOutcome, NavigationState, NavigationWait,
};

/// What executing an agent action produced right away.
pub(crate) enum AgentActionExecution {
    /// The action finished; audit and publish this result.
    Done(Result<BrowserControlValue, BrowserControlFailure>),
    /// The action is waiting for page signals; the driver keeps it pending.
    Pending,
}

/// A page signal relevant to a navigation in flight.
#[derive(Clone, Copy, Debug)]
pub(crate) enum NavigationSignal<'a> {
    /// The top-level document committed at this URL.
    Committed(&'a str),
    /// A same-document (fragment or history API) change to this URL.
    SameDocument(&'a str),
    DomContentLoaded,
    Load,
    /// The navigation failed after dispatch (unreachable destination).
    Failed(&'a str),
    /// The document title became known.
    Title(&'a str),
}

#[derive(Debug)]
pub(crate) struct PendingNavigation {
    pub(crate) request: AgentAction,
    requested_url: String,
    wait: NavigationWait,
    started: Instant,
    deadline: Instant,
    committed_url: Option<String>,
    title: Option<String>,
    dom_content_loaded: bool,
    loaded: bool,
}

pub(crate) type NavigationResult = Result<BrowserControlValue, BrowserControlFailure>;

/// Wall-clock milliseconds, comparable with `AgentAction::requested_at_millis`.
pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}

impl PendingNavigation {
    /// `queued_for` is how long the action already waited in the host queue
    /// before dispatch: the bound and the reported elapsed time both count
    /// from the moment the caller queued the action, so the typed timeout
    /// report reaches the caller before the caller's own deadline.
    pub(crate) fn new(
        request: AgentAction,
        requested_url: String,
        wait: NavigationWait,
        timeout: Duration,
        queued_for: Duration,
        now: Instant,
    ) -> Self {
        let started = now.checked_sub(queued_for).unwrap_or(now);
        Self {
            request,
            requested_url,
            wait,
            started,
            deadline: started + timeout,
            committed_url: None,
            title: None,
            dom_content_loaded: false,
            loaded: false,
        }
    }

    /// Time an action spent queued before the driver picked it up.
    pub(crate) fn queued_for(request: &AgentAction, now_millis: i64) -> Duration {
        Duration::from_millis(u64::try_from(now_millis - request.requested_at_millis).unwrap_or(0))
    }

    /// Outcome for a caller that only asked for dispatch.
    pub(crate) fn dispatched(&self, now: Instant) -> BrowserControlValue {
        self.outcome(NavigationState::Dispatched, now)
    }

    /// Apply one signal; `Some` when the action is settled.
    pub(crate) fn observe(&mut self, signal: NavigationSignal<'_>, now: Instant) -> Option<NavigationResult> {
        match signal {
            NavigationSignal::Committed(url) => {
                self.committed_url = Some(url.to_string());
                self.title = None;
                self.dom_content_loaded = false;
                self.loaded = false;
            }
            NavigationSignal::SameDocument(url) => {
                self.committed_url = Some(url.to_string());
                self.dom_content_loaded = true;
                self.loaded = true;
            }
            NavigationSignal::DomContentLoaded if self.committed_url.is_some() => self.dom_content_loaded = true,
            NavigationSignal::Load if self.committed_url.is_some() => {
                self.dom_content_loaded = true;
                self.loaded = true;
            }
            NavigationSignal::DomContentLoaded | NavigationSignal::Load => {}
            NavigationSignal::Title(title) => {
                if self.committed_url.is_some() && !title.is_empty() {
                    self.title = Some(title.to_string());
                }
                return None;
            }
            NavigationSignal::Failed(message) => {
                return Some(Err(BrowserControlFailure::new("navigation_failed", message)));
            }
        }
        self.settled_state().map(|state| Ok(self.outcome(state, now)))
    }

    /// Report a timeout with the latest page state once the bound elapsed.
    pub(crate) fn tick(&self, now: Instant) -> Option<NavigationResult> {
        (now >= self.deadline).then(|| Ok(self.outcome(NavigationState::TimedOut, now)))
    }

    /// Outcome for a navigation replaced by a later navigation action.
    pub(crate) fn superseded(&self, now: Instant) -> BrowserControlValue {
        self.outcome(NavigationState::Superseded, now)
    }

    /// Settle a navigation the backend completed synchronously (classic
    /// `WebDriver` blocks until the document committed and loaded).
    pub(crate) fn settle_loaded(&mut self, committed_url: &str, title: &str, now: Instant) -> BrowserControlValue {
        self.committed_url = Some(committed_url.to_string());
        self.title = (!title.is_empty()).then(|| title.to_string());
        self.dom_content_loaded = true;
        self.loaded = true;
        let state = match self.wait {
            NavigationWait::Dispatched => NavigationState::Dispatched,
            NavigationWait::Commit => NavigationState::Committed,
            NavigationWait::DomContentLoaded => NavigationState::DomContentLoaded,
        };
        self.outcome(state, now)
    }

    fn settled_state(&self) -> Option<NavigationState> {
        match self.wait {
            NavigationWait::Dispatched => Some(NavigationState::Dispatched),
            NavigationWait::Commit => self.committed_url.as_ref().map(|_| NavigationState::Committed),
            NavigationWait::DomContentLoaded => {
                (self.committed_url.is_some() && self.dom_content_loaded).then_some(NavigationState::DomContentLoaded)
            }
        }
    }

    fn outcome(&self, state: NavigationState, now: Instant) -> BrowserControlValue {
        let redirected = self
            .committed_url
            .as_deref()
            .is_some_and(|committed| !same_destination(&self.requested_url, committed));
        BrowserControlValue::Navigation {
            navigation: NavigationOutcome {
                requested_url: self.requested_url.clone(),
                wait: self.wait,
                state,
                committed_url: self.committed_url.clone(),
                title: self.title.clone(),
                loading: !self.loaded,
                redirected,
                elapsed_millis: u64::try_from(now.saturating_duration_since(self.started).as_millis())
                    .unwrap_or(u64::MAX),
            },
        }
    }
}

/// Whether a committed URL is the requested destination rather than a
/// redirect. Browsers add or drop a trailing slash on bare origins, so that
/// difference alone is not a redirect.
fn same_destination(requested: &str, committed: &str) -> bool {
    requested == committed || requested.trim_end_matches('/') == committed.trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrowserControlAction;

    fn pending(wait: NavigationWait, now: Instant) -> PendingNavigation {
        let request = AgentAction {
            action_id: "action-1".to_string(),
            actor: "horizon:agent".to_string(),
            requested_at_millis: 0,
            action: BrowserControlAction::Navigate {
                url: "https://example.test/start".to_string(),
                wait,
                timeout_millis: Some(1_000),
            },
        };
        PendingNavigation::new(
            request,
            "https://example.test/start".to_string(),
            wait,
            Duration::from_millis(1_000),
            Duration::ZERO,
            now,
        )
    }

    fn navigation(result: Option<NavigationResult>) -> NavigationOutcome {
        match result.expect("settled").expect("completed") {
            BrowserControlValue::Navigation { navigation } => navigation,
            other => panic!("unexpected value {other:?}"),
        }
    }

    #[test]
    fn a_commit_wait_settles_on_the_committed_document() {
        let now = Instant::now();
        let mut pending = pending(NavigationWait::Commit, now);
        assert!(pending.observe(NavigationSignal::DomContentLoaded, now).is_none());
        assert!(pending.observe(NavigationSignal::Title("early"), now).is_none());
        let outcome = navigation(pending.observe(
            NavigationSignal::Committed("https://example.test/start"),
            now + Duration::from_millis(40),
        ));
        assert_eq!(outcome.state, NavigationState::Committed);
        assert_eq!(outcome.committed_url.as_deref(), Some("https://example.test/start"));
        assert_eq!(
            outcome.title, None,
            "a title seen before the commit belongs to the old page"
        );
        assert!(outcome.loading);
        assert!(!outcome.redirected);
        assert_eq!(outcome.elapsed_millis, 40);
    }

    #[test]
    fn redirects_are_reported_but_a_trailing_slash_is_not_one() {
        let now = Instant::now();
        let mut pending = pending(NavigationWait::Commit, now);
        let outcome = navigation(pending.observe(NavigationSignal::Committed("https://example.test/other"), now));
        assert!(outcome.redirected);

        let request = pending.request.clone();
        let mut bare = PendingNavigation::new(
            request,
            "https://example.test".to_string(),
            NavigationWait::Commit,
            Duration::from_secs(1),
            Duration::ZERO,
            now,
        );
        let outcome = navigation(bare.observe(NavigationSignal::Committed("https://example.test/"), now));
        assert!(!outcome.redirected);
    }

    #[test]
    fn a_dom_content_loaded_wait_needs_the_commit_first_and_reports_load_state() {
        let now = Instant::now();
        let mut pending = pending(NavigationWait::DomContentLoaded, now);
        assert!(
            pending.observe(NavigationSignal::Load, now).is_none(),
            "load before commit is the old page"
        );
        assert!(
            pending
                .observe(NavigationSignal::Committed("https://example.test/start"), now)
                .is_none()
        );
        assert!(pending.observe(NavigationSignal::Title("Start"), now).is_none());
        let outcome = navigation(pending.observe(NavigationSignal::DomContentLoaded, now));
        assert_eq!(outcome.state, NavigationState::DomContentLoaded);
        assert_eq!(outcome.title.as_deref(), Some("Start"));
        assert!(outcome.loading, "DOMContentLoaded precedes the load event");

        let mut loaded = pending_with_commit(now);
        let outcome = navigation(loaded.observe(NavigationSignal::Load, now));
        assert_eq!(outcome.state, NavigationState::DomContentLoaded);
        assert!(!outcome.loading);
    }

    fn pending_with_commit(now: Instant) -> PendingNavigation {
        let mut pending = pending(NavigationWait::DomContentLoaded, now);
        assert!(
            pending
                .observe(NavigationSignal::Committed("https://example.test/start"), now)
                .is_none()
        );
        pending
    }

    #[test]
    fn same_document_changes_commit_and_load_at_once() {
        let now = Instant::now();
        let mut pending = pending(NavigationWait::DomContentLoaded, now);
        let outcome = navigation(pending.observe(NavigationSignal::SameDocument("https://example.test/start#a"), now));
        assert_eq!(outcome.state, NavigationState::DomContentLoaded);
        assert!(!outcome.loading);
    }

    #[test]
    fn failures_timeouts_and_supersession_are_typed() {
        let now = Instant::now();
        let mut pending = pending(NavigationWait::Commit, now);
        let failure = pending
            .observe(NavigationSignal::Failed("net::ERR_CONNECTION_REFUSED"), now)
            .expect("settled")
            .expect_err("failed");
        assert_eq!(failure.code, "navigation_failed");

        let pending = super::PendingNavigation::new(
            pending.request.clone(),
            "https://example.test/start".to_string(),
            NavigationWait::Commit,
            Duration::from_millis(500),
            Duration::ZERO,
            now,
        );
        assert!(pending.tick(now + Duration::from_millis(499)).is_none());
        let outcome = navigation(pending.tick(now + Duration::from_millis(500)));
        assert_eq!(outcome.state, NavigationState::TimedOut);
        assert_eq!(outcome.committed_url, None);
        assert_eq!(outcome.elapsed_millis, 500);

        // Queue latency counts against the bound and the reported elapsed time.
        let queued = super::PendingNavigation::new(
            pending.request.clone(),
            "https://example.test/start".to_string(),
            NavigationWait::Commit,
            Duration::from_millis(500),
            Duration::from_millis(300),
            now,
        );
        assert!(queued.tick(now + Duration::from_millis(199)).is_none());
        let outcome = navigation(queued.tick(now + Duration::from_millis(200)));
        assert_eq!(outcome.state, NavigationState::TimedOut);
        assert_eq!(outcome.elapsed_millis, 500);
        assert_eq!(
            PendingNavigation::queued_for(&pending.request, 1_250),
            Duration::from_millis(1_250),
            "queue latency is measured from requested_at_millis"
        );
        assert_eq!(PendingNavigation::queued_for(&pending.request, -5), Duration::ZERO);

        let outcome = navigation(Some(Ok(pending.superseded(now + Duration::from_millis(10)))));
        assert_eq!(outcome.state, NavigationState::Superseded);
        assert_eq!(outcome.elapsed_millis, 10);

        let outcome = navigation(Some(Ok(pending.dispatched(now))));
        assert_eq!(outcome.state, NavigationState::Dispatched);
        assert!(outcome.loading);
    }

    #[test]
    fn synchronous_backends_settle_at_the_requested_readiness() {
        let now = Instant::now();
        for (wait, expected) in [
            (NavigationWait::Dispatched, NavigationState::Dispatched),
            (NavigationWait::Commit, NavigationState::Committed),
            (NavigationWait::DomContentLoaded, NavigationState::DomContentLoaded),
        ] {
            let mut pending = pending(wait, now);
            let outcome = navigation(Some(Ok(pending.settle_loaded(
                "https://example.test/start",
                "Start",
                now,
            ))));
            assert_eq!(outcome.state, expected);
            assert_eq!(outcome.title.as_deref(), Some("Start"));
            assert!(!outcome.loading);
        }
    }
}
