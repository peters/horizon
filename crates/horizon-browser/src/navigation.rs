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
/// A page signal with the backend's navigation identity when it carries one
/// (Chromium loader ids, `BiDi` navigation ids); classic `WebDriver` has none.
#[derive(Clone, Copy, Debug)]
pub(crate) enum NavigationSignal<'a> {
    /// The top-level document committed at this URL.
    Committed {
        url: &'a str,
        id: Option<&'a str>,
    },
    /// A same-document (fragment or history API) change to this URL.
    SameDocument {
        url: &'a str,
        id: Option<&'a str>,
    },
    DomContentLoaded {
        id: Option<&'a str>,
    },
    Load {
        id: Option<&'a str>,
    },
    /// The navigation failed after dispatch (unreachable destination).
    Failed {
        message: &'a str,
        id: Option<&'a str>,
    },
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
    /// Whether the backend has answered the dispatch; until then a commit
    /// or failure cannot be attributed and is held in `deferred`, one entry
    /// per navigation id, until the reply names ours.
    dispatch_known: bool,
    /// Backend identity of this navigation, once the dispatch reply names it.
    expected_id: Option<String>,
    deferred: Vec<DeferredSignal>,
}

/// A commit or failure observed before the dispatch reply identified the
/// navigation it belongs to.
#[derive(Debug)]
enum DeferredSignal {
    /// A commit, with the readiness the same navigation reached meanwhile
    /// (Firefox reports the commit at `DOMContentLoaded`, so readiness can
    /// precede the dispatch reply too).
    Committed {
        url: String,
        id: Option<String>,
        dom_content_loaded: bool,
        loaded: bool,
    },
    SameDocument {
        url: String,
        id: Option<String>,
    },
    Failed {
        message: String,
        id: Option<String>,
    },
}

impl DeferredSignal {
    fn id(&self) -> Option<&str> {
        match self {
            Self::Committed { id, .. } | Self::SameDocument { id, .. } | Self::Failed { id, .. } => id.as_deref(),
        }
    }

    /// Record readiness observed for the same navigation as a held commit.
    fn note_readiness(&mut self, signal_id: Option<&str>, loaded_now: bool) {
        if let Self::Committed {
            id,
            dom_content_loaded,
            loaded,
            ..
        } = self
            && (id.is_none() || signal_id.is_none() || id.as_deref() == signal_id)
        {
            *dom_content_loaded = true;
            if loaded_now {
                *loaded = true;
            }
        }
    }
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
            dispatch_known: false,
            expected_id: None,
            deferred: Vec::new(),
        }
    }

    /// The backend answered the dispatch, naming the navigation (or `None`
    /// when the backend gives none, as for same-document changes). A commit
    /// or failure held back until now is applied if it belongs to this
    /// navigation and dropped otherwise, so an earlier navigation still in
    /// flight cannot settle this one.
    pub(crate) fn attach_id(&mut self, id: Option<&str>, now: Instant) -> Option<NavigationResult> {
        self.dispatch_known = true;
        self.expected_id = id.map(str::to_string);
        // Keep the newest held signal that belongs to this navigation; every
        // other candidate came from a navigation still in flight.
        let candidates = std::mem::take(&mut self.deferred);
        let deferred = candidates.into_iter().rev().find(|candidate| {
            let cross_document = !matches!(candidate, DeferredSignal::SameDocument { .. });
            self.correlates(candidate.id()) && !(cross_document && self.foreign_to_id_less_dispatch(candidate.id()))
        })?;
        match deferred {
            DeferredSignal::Committed {
                url,
                id,
                dom_content_loaded,
                loaded,
            } => {
                let id = id.as_deref();
                self.apply(NavigationSignal::Committed { url: &url, id }, now);
                if dom_content_loaded {
                    self.apply(NavigationSignal::DomContentLoaded { id }, now);
                }
                if loaded {
                    self.apply(NavigationSignal::Load { id }, now);
                }
                self.settled_state().map(|state| Ok(self.outcome(state, now)))
            }
            DeferredSignal::SameDocument { url, id } => self.apply(
                NavigationSignal::SameDocument {
                    url: &url,
                    id: id.as_deref(),
                },
                now,
            ),
            DeferredSignal::Failed { message, id } => self.apply(
                NavigationSignal::Failed {
                    message: &message,
                    id: id.as_deref(),
                },
                now,
            ),
        }
    }

    /// Whether a signal carrying `id` belongs to this navigation, as far as
    /// the dispatch reply has told us.
    pub(crate) fn correlates(&self, id: Option<&str>) -> bool {
        match (self.expected_id.as_deref(), id) {
            (Some(expected), Some(observed)) => expected == observed,
            _ => true,
        }
    }

    /// A dispatch reply without an id means the backend ran a same-document
    /// navigation; an identified cross-document commit or failure cannot be
    /// ours then (it belongs to an earlier navigation still in flight).
    fn foreign_to_id_less_dispatch(&self, id: Option<&str>) -> bool {
        self.dispatch_known && self.expected_id.is_none() && id.is_some()
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
        let id = match signal {
            NavigationSignal::Committed { id, .. }
            | NavigationSignal::SameDocument { id, .. }
            | NavigationSignal::DomContentLoaded { id }
            | NavigationSignal::Load { id }
            | NavigationSignal::Failed { id, .. } => id,
            NavigationSignal::Title(_) => None,
        };
        if !self.correlates(id) {
            return None;
        }
        if matches!(
            signal,
            NavigationSignal::Committed { .. } | NavigationSignal::Failed { .. }
        ) && self.foreign_to_id_less_dispatch(id)
        {
            return None;
        }
        if let NavigationSignal::SameDocument { url, id: None } = signal
            && !same_document_destination(&self.requested_url, url)
        {
            // Chromium's same-document event carries no loader id: only a
            // change to the requested document itself can be ours; a
            // superseded navigation's fragment change on another page is not.
            return None;
        }
        if !self.dispatch_known && id.is_some() {
            // Signals that name a navigation cannot be attributed before the
            // dispatch reply names ours; hold them back per navigation id so
            // a foreign late signal cannot displace this navigation's own.
            match signal {
                NavigationSignal::Committed { url, id } => self.hold(DeferredSignal::Committed {
                    url: url.to_string(),
                    id: id.map(str::to_string),
                    dom_content_loaded: false,
                    loaded: false,
                }),
                NavigationSignal::SameDocument { url, id } => self.hold(DeferredSignal::SameDocument {
                    url: url.to_string(),
                    id: id.map(str::to_string),
                }),
                NavigationSignal::Failed { message, id } => self.hold(DeferredSignal::Failed {
                    message: message.to_string(),
                    id: id.map(str::to_string),
                }),
                NavigationSignal::DomContentLoaded { id } => {
                    for deferred in &mut self.deferred {
                        deferred.note_readiness(id, false);
                    }
                }
                NavigationSignal::Load { id } => {
                    for deferred in &mut self.deferred {
                        deferred.note_readiness(id, true);
                    }
                }
                NavigationSignal::Title(_) => unreachable!("titles carry no navigation id"),
            }
            return None;
        }
        self.apply(signal, now)
    }

    /// Hold a signal back, replacing an earlier one for the same navigation.
    fn hold(&mut self, signal: DeferredSignal) {
        let id = signal.id().map(str::to_string);
        self.deferred.retain(|held| held.id() != id.as_deref());
        self.deferred.push(signal);
    }

    fn apply(&mut self, signal: NavigationSignal<'_>, now: Instant) -> Option<NavigationResult> {
        match signal {
            NavigationSignal::Committed { url, .. } => {
                self.committed_url = Some(url.to_string());
                self.title = None;
                self.dom_content_loaded = false;
                self.loaded = false;
            }
            NavigationSignal::SameDocument { url, .. } => {
                self.committed_url = Some(url.to_string());
                self.dom_content_loaded = true;
                self.loaded = true;
            }
            NavigationSignal::DomContentLoaded { .. } if self.committed_url.is_some() => self.dom_content_loaded = true,
            NavigationSignal::Load { .. } if self.committed_url.is_some() => {
                self.dom_content_loaded = true;
                self.loaded = true;
            }
            NavigationSignal::DomContentLoaded { .. } | NavigationSignal::Load { .. } => {}
            NavigationSignal::Title(title) => {
                if self.committed_url.is_some() && !title.is_empty() {
                    self.title = Some(title.to_string());
                }
                return None;
            }
            NavigationSignal::Failed { message, .. } => {
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
    /// A classic backend's blocking navigation hit the action bound: report
    /// `timed_out` with whatever the browser committed meanwhile.
    pub(crate) fn settle_timed_out(&mut self, committed_url: Option<&str>, now: Instant) -> BrowserControlValue {
        self.committed_url = committed_url.map(str::to_string);
        self.outcome(NavigationState::TimedOut, now)
    }

    /// Remaining time before this navigation's bound elapses.
    pub(crate) fn remaining(&self, now: Instant) -> Duration {
        self.deadline.saturating_duration_since(now)
    }

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
/// Same-document changes differ from the requested URL by fragment at most.
fn same_document_destination(requested: &str, committed: &str) -> bool {
    let strip = |url: &str| {
        url.split_once('#')
            .map_or(url.to_string(), |(base, _)| base.to_string())
    };
    same_destination(&strip(requested), &strip(committed))
}

fn same_destination(requested: &str, committed: &str) -> bool {
    requested == committed || bare_origin_variant(requested, committed) || bare_origin_variant(committed, requested)
}

/// `https://host` and `https://host/` name the same document because the
/// browser adds the root slash itself; a trailing slash on a deeper path
/// (`/docs` and `/docs/`) is a real redirect.
fn bare_origin_variant(short: &str, long: &str) -> bool {
    let bare_origin = short
        .split_once("://")
        .is_some_and(|(_, rest)| !rest.is_empty() && !rest.contains('/'));
    bare_origin && long.len() == short.len() + 1 && long.starts_with(short) && long.ends_with('/')
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
        assert!(
            pending
                .observe(NavigationSignal::DomContentLoaded { id: None }, now)
                .is_none()
        );
        assert!(pending.observe(NavigationSignal::Title("early"), now).is_none());
        let outcome = navigation(pending.observe(
            NavigationSignal::Committed {
                url: "https://example.test/start",
                id: None,
            },
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
        let outcome = navigation(pending.observe(
            NavigationSignal::Committed {
                url: "https://example.test/other",
                id: None,
            },
            now,
        ));
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
        let outcome = navigation(bare.observe(
            NavigationSignal::Committed {
                url: "https://example.test/",
                id: None,
            },
            now,
        ));
        assert!(!outcome.redirected);

        let mut path = PendingNavigation::new(
            bare.request.clone(),
            "https://example.test/docs/".to_string(),
            NavigationWait::Commit,
            Duration::from_secs(1),
            Duration::ZERO,
            now,
        );
        let outcome = navigation(path.observe(
            NavigationSignal::Committed {
                url: "https://example.test/docs",
                id: None,
            },
            now,
        ));
        assert!(outcome.redirected, "a trailing slash on a path is a real redirect");
        assert!(same_destination("https://example.test/", "https://example.test"));
        assert!(!same_destination("https://example.test/a", "https://example.test/a/"));
    }

    #[test]
    fn signals_from_another_navigation_never_settle_a_pending_one() {
        let now = Instant::now();
        // The dispatch reply arrives first and names this navigation.
        let mut named = pending(NavigationWait::DomContentLoaded, now);
        assert!(named.attach_id(Some("loader-b"), now).is_none());
        assert!(
            named
                .observe(
                    NavigationSignal::Committed {
                        url: "https://example.test/a",
                        id: Some("loader-a")
                    },
                    now
                )
                .is_none(),
            "an earlier navigation's commit is ignored"
        );
        assert!(
            named
                .observe(
                    NavigationSignal::Failed {
                        message: "a failed",
                        id: Some("loader-a")
                    },
                    now
                )
                .is_none(),
            "an earlier navigation's failure is ignored"
        );
        assert!(
            named
                .observe(
                    NavigationSignal::Committed {
                        url: "https://example.test/start",
                        id: Some("loader-b")
                    },
                    now
                )
                .is_none()
        );
        assert!(
            named
                .observe(NavigationSignal::DomContentLoaded { id: Some("loader-a") }, now)
                .is_none()
        );
        let outcome = navigation(named.observe(NavigationSignal::DomContentLoaded { id: Some("loader-b") }, now));
        assert_eq!(outcome.state, NavigationState::DomContentLoaded);
        assert_eq!(outcome.committed_url.as_deref(), Some("https://example.test/start"));

        // The commit arrives before the dispatch reply: it is held back until
        // the reply proves it belongs to this navigation.
        let mut early = pending(NavigationWait::Commit, now);
        assert!(
            early
                .observe(
                    NavigationSignal::Committed {
                        url: "https://example.test/start",
                        id: Some("loader-b")
                    },
                    now
                )
                .is_none()
        );
        let outcome = navigation(early.attach_id(Some("loader-b"), now + Duration::from_millis(5)));
        assert_eq!(outcome.state, NavigationState::Committed);
        assert_eq!(outcome.elapsed_millis, 5);

        // A held-back commit from another navigation is dropped when the reply
        // names a different one, and the bound then reports no commit.
        let mut foreign = pending(NavigationWait::Commit, now);
        assert!(
            foreign
                .observe(
                    NavigationSignal::Committed {
                        url: "https://example.test/a",
                        id: Some("loader-a")
                    },
                    now
                )
                .is_none()
        );
        assert!(foreign.attach_id(Some("loader-b"), now).is_none());
        let outcome = navigation(foreign.tick(now + Duration::from_secs(1)));
        assert_eq!(outcome.state, NavigationState::TimedOut);
        assert!(outcome.committed_url.is_none());

        // Failures carried by the dispatch reply itself have no id and apply
        // at once; unidentified signals match once the reply arrived.
        let mut reply_failure = pending(NavigationWait::Commit, now);
        let failure = reply_failure
            .observe(
                NavigationSignal::Failed {
                    message: "net::ERR_ABORTED",
                    id: None,
                },
                now,
            )
            .expect("settled")
            .expect_err("failed");
        assert_eq!(failure.code, "navigation_failed");
        let mut unnamed = pending(NavigationWait::Commit, now);
        assert!(unnamed.attach_id(None, now).is_none());
        let outcome = navigation(unnamed.observe(
            NavigationSignal::SameDocument {
                url: "https://example.test/start#x",
                id: None,
            },
            now,
        ));
        assert_eq!(outcome.state, NavigationState::Committed);
    }

    #[test]
    fn readiness_observed_before_the_dispatch_reply_is_replayed_with_the_commit() {
        let now = Instant::now();
        // Firefox reports the commit at DOMContentLoaded; both can precede the
        // navigate reply.
        let mut ready = pending(NavigationWait::DomContentLoaded, now);
        assert!(
            ready
                .observe(
                    NavigationSignal::Committed {
                        url: "https://example.test/start",
                        id: Some("nav-b")
                    },
                    now
                )
                .is_none()
        );
        assert!(
            ready
                .observe(NavigationSignal::DomContentLoaded { id: Some("nav-b") }, now)
                .is_none()
        );
        let outcome = navigation(ready.attach_id(Some("nav-b"), now));
        assert_eq!(outcome.state, NavigationState::DomContentLoaded);
        assert!(outcome.loading, "load has not fired yet");

        let mut loaded = pending(NavigationWait::Commit, now);
        assert!(
            loaded
                .observe(
                    NavigationSignal::Committed {
                        url: "https://example.test/start",
                        id: Some("nav-b")
                    },
                    now
                )
                .is_none()
        );
        assert!(
            loaded
                .observe(NavigationSignal::Load { id: Some("nav-b") }, now)
                .is_none()
        );
        let outcome = navigation(loaded.attach_id(Some("nav-b"), now));
        assert_eq!(outcome.state, NavigationState::Committed);
        assert!(!outcome.loading, "load observed before the reply is replayed");

        // Readiness from another navigation is not attributed to the held commit.
        let mut foreign = pending(NavigationWait::DomContentLoaded, now);
        assert!(
            foreign
                .observe(
                    NavigationSignal::Committed {
                        url: "https://example.test/start",
                        id: Some("nav-b")
                    },
                    now
                )
                .is_none()
        );
        assert!(
            foreign
                .observe(NavigationSignal::DomContentLoaded { id: Some("nav-a") }, now)
                .is_none()
        );
        assert!(
            foreign.attach_id(Some("nav-b"), now).is_none(),
            "a foreign DOMContentLoaded does not satisfy the wait"
        );
        assert!(foreign.correlates(Some("nav-b")));
        assert!(!foreign.correlates(Some("nav-a")));
    }

    #[test]
    fn unidentified_same_document_events_must_name_the_requested_document() {
        let now = Instant::now();
        let mut pending = pending(NavigationWait::Commit, now);
        assert!(pending.attach_id(None, now).is_none());
        assert!(
            pending
                .observe(
                    NavigationSignal::SameDocument {
                        url: "https://example.test/other#section",
                        id: None
                    },
                    now
                )
                .is_none(),
            "a superseded navigation's fragment change on another page is ignored"
        );
        let outcome = navigation(pending.observe(
            NavigationSignal::SameDocument {
                url: "https://example.test/start#section",
                id: None,
            },
            now,
        ));
        assert_eq!(outcome.state, NavigationState::Committed);
        assert_eq!(
            outcome.committed_url.as_deref(),
            Some("https://example.test/start#section")
        );
    }

    #[test]
    fn an_id_less_dispatch_rejects_identified_cross_document_signals() {
        let now = Instant::now();
        // Cross-document A is superseded by same-document B: A's loader-scoped
        // commit arrives before B's id-less reply and must not settle B.
        let mut pending = pending(NavigationWait::Commit, now);
        assert!(
            pending
                .observe(
                    NavigationSignal::Committed {
                        url: "https://example.test/a",
                        id: Some("loader-a")
                    },
                    now
                )
                .is_none()
        );
        assert!(
            pending.attach_id(None, now).is_none(),
            "the held cross-document commit is dropped"
        );
        assert!(
            pending
                .observe(
                    NavigationSignal::Committed {
                        url: "https://example.test/a",
                        id: Some("loader-a")
                    },
                    now
                )
                .is_none(),
            "identified commits stay foreign after an id-less dispatch"
        );
        assert!(
            pending
                .observe(
                    NavigationSignal::Failed {
                        message: "a failed",
                        id: Some("loader-a")
                    },
                    now
                )
                .is_none()
        );
        let outcome = navigation(pending.observe(
            NavigationSignal::SameDocument {
                url: "https://example.test/start#x",
                id: None,
            },
            now,
        ));
        assert_eq!(outcome.state, NavigationState::Committed);
        assert_eq!(outcome.committed_url.as_deref(), Some("https://example.test/start#x"));

        // A bound already exhausted in the queue reports timed_out before any
        // dispatch would happen.
        let request = pending.request.clone();
        let expired = PendingNavigation::new(
            request,
            "https://example.test/start".to_string(),
            NavigationWait::Commit,
            Duration::from_millis(500),
            Duration::from_millis(600),
            now,
        );
        assert!(expired.remaining(now).is_zero());
        assert_eq!(navigation(expired.tick(now)).state, NavigationState::TimedOut);
    }

    #[test]
    fn a_foreign_late_signal_does_not_displace_the_held_commit() {
        let now = Instant::now();
        // B commits before its dispatch reply; superseded A then fails and
        // commits late. B's own commit must survive until the reply names B.
        let mut held = pending(NavigationWait::Commit, now);
        assert!(
            held.observe(
                NavigationSignal::Committed {
                    url: "https://example.test/start",
                    id: Some("loader-b")
                },
                now
            )
            .is_none()
        );
        assert!(
            held.observe(
                NavigationSignal::Failed {
                    message: "a failed",
                    id: Some("loader-a")
                },
                now
            )
            .is_none()
        );
        assert!(
            held.observe(
                NavigationSignal::Committed {
                    url: "https://example.test/a",
                    id: Some("loader-a")
                },
                now
            )
            .is_none()
        );
        assert!(
            held.observe(NavigationSignal::Load { id: Some("loader-b") }, now)
                .is_none()
        );
        let outcome = navigation(held.attach_id(Some("loader-b"), now));
        assert_eq!(outcome.state, NavigationState::Committed);
        assert_eq!(outcome.committed_url.as_deref(), Some("https://example.test/start"));
        assert!(!outcome.loading, "B's own readiness was kept with B's commit");

        // A later signal for the same navigation replaces the earlier one.
        let mut replaced = pending(NavigationWait::Commit, now);
        assert!(
            replaced
                .observe(
                    NavigationSignal::Committed {
                        url: "https://example.test/first",
                        id: Some("loader-b")
                    },
                    now
                )
                .is_none()
        );
        assert!(
            replaced
                .observe(
                    NavigationSignal::Failed {
                        message: "b failed",
                        id: Some("loader-b")
                    },
                    now
                )
                .is_none()
        );
        let failure = replaced
            .attach_id(Some("loader-b"), now)
            .expect("settled")
            .expect_err("failed");
        assert_eq!(failure.code, "navigation_failed");
    }

    #[test]
    fn classic_backends_report_the_bound_with_whatever_committed() {
        let now = Instant::now();
        let mut pending = pending(NavigationWait::Commit, now);
        assert_eq!(
            pending.remaining(now + Duration::from_millis(400)),
            Duration::from_millis(600)
        );
        let outcome = match pending.settle_timed_out(Some("https://example.test/slow"), now + Duration::from_secs(1)) {
            BrowserControlValue::Navigation { navigation } => navigation,
            other => panic!("unexpected value {other:?}"),
        };
        assert_eq!(outcome.state, NavigationState::TimedOut);
        assert_eq!(outcome.committed_url.as_deref(), Some("https://example.test/slow"));
        assert!(outcome.loading);
        assert!(outcome.redirected);
        assert_eq!(outcome.elapsed_millis, 1_000);
        let no_commit = match pending.settle_timed_out(None, now + Duration::from_secs(1)) {
            BrowserControlValue::Navigation { navigation } => navigation,
            other => panic!("unexpected value {other:?}"),
        };
        assert!(no_commit.committed_url.is_none());
    }

    #[test]
    fn a_dom_content_loaded_wait_needs_the_commit_first_and_reports_load_state() {
        let now = Instant::now();
        let mut pending = pending(NavigationWait::DomContentLoaded, now);
        assert!(
            pending.observe(NavigationSignal::Load { id: None }, now).is_none(),
            "load before commit is the old page"
        );
        assert!(
            pending
                .observe(
                    NavigationSignal::Committed {
                        url: "https://example.test/start",
                        id: None
                    },
                    now
                )
                .is_none()
        );
        assert!(pending.observe(NavigationSignal::Title("Start"), now).is_none());
        let outcome = navigation(pending.observe(NavigationSignal::DomContentLoaded { id: None }, now));
        assert_eq!(outcome.state, NavigationState::DomContentLoaded);
        assert_eq!(outcome.title.as_deref(), Some("Start"));
        assert!(outcome.loading, "DOMContentLoaded precedes the load event");

        let mut loaded = pending_with_commit(now);
        let outcome = navigation(loaded.observe(NavigationSignal::Load { id: None }, now));
        assert_eq!(outcome.state, NavigationState::DomContentLoaded);
        assert!(!outcome.loading);
    }

    fn pending_with_commit(now: Instant) -> PendingNavigation {
        let mut pending = pending(NavigationWait::DomContentLoaded, now);
        assert!(
            pending
                .observe(
                    NavigationSignal::Committed {
                        url: "https://example.test/start",
                        id: None
                    },
                    now
                )
                .is_none()
        );
        pending
    }

    #[test]
    fn same_document_changes_commit_and_load_at_once() {
        let now = Instant::now();
        let mut pending = pending(NavigationWait::DomContentLoaded, now);
        let outcome = navigation(pending.observe(
            NavigationSignal::SameDocument {
                url: "https://example.test/start#a",
                id: None,
            },
            now,
        ));
        assert_eq!(outcome.state, NavigationState::DomContentLoaded);
        assert!(!outcome.loading);
    }

    #[test]
    fn failures_timeouts_and_supersession_are_typed() {
        let now = Instant::now();
        let mut pending = pending(NavigationWait::Commit, now);
        let failure = pending
            .observe(
                NavigationSignal::Failed {
                    message: "net::ERR_CONNECTION_REFUSED",
                    id: None,
                },
                now,
            )
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
