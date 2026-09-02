//! Backend-neutral bookkeeping for a selector wait observed inside the
//! driver loop: one audited action that polls the page at a bounded cadence
//! and settles on the selector state, the deadline, or a cancellation.

use std::time::{Duration, Instant};

use crate::{
    AgentAction, BrowserControlFailure, BrowserControlValue, BrowserNode, DEFAULT_WAIT_TIMEOUT_MILLIS, SelectorState,
    WaitOutcome,
};

/// Interval between page observations while a wait is pending.
pub(crate) const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Bounds for one synchronous page observation: never shorter than a
/// coordination tick, never longer than the drivers' own command timeout.
pub(crate) const MIN_OBSERVATION_BUDGET: Duration = Duration::from_millis(250);
pub(crate) const MAX_OBSERVATION_BUDGET: Duration = Duration::from_secs(5);
/// Consecutive retryable observation failures tolerated before the wait
/// surfaces the failure itself instead of retrying.
pub(crate) const MAX_TRANSIENT_FAILURES: u32 = 5;
/// Most matching nodes a wait reports in its outcome.
pub(crate) const WAIT_MAX_RESULTS: u32 = 20;
/// Matches the engine evaluates the condition over (the protocol's query
/// maximum), so a visible match beyond the reported ones still counts.
pub(crate) const WAIT_QUERY_RESULTS: u32 = crate::MAX_QUERY_RESULTS;

/// Why a pending wait was cancelled before its condition was met.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WaitStop {
    /// The page generation changed: earlier observations no longer apply.
    NavigationInvalidated,
    /// The requesting actor no longer holds the panel lease.
    OwnershipLost,
    /// A handoff to the user is pending; agent observation must stop.
    HandoffPending,
    /// A later wait on the same panel replaced this one.
    Superseded,
}

pub(crate) type WaitResult = Result<BrowserControlValue, BrowserControlFailure>;

#[derive(Debug)]
pub(crate) struct PendingWait {
    pub(crate) request: AgentAction,
    pub(crate) selector: String,
    state: SelectorState,
    started: Instant,
    deadline: Instant,
    next_poll: Instant,
    generation_at_start: u64,
    polls: u32,
    last_match_count: usize,
    consecutive_failures: u32,
    /// Set while an in-flight navigation kept this wait from observing; once
    /// that navigation settles the wait is invalidated, because the document
    /// it may now observe is not the one it was asked about.
    blocked_by_navigation: bool,
    /// A satisfied observation held until the driver has re-read the
    /// coordination signals and drained backend events, so a lease or
    /// handoff change (or a navigation) during the blocking observation
    /// still cancels the wait. Stored with the signal epoch it was observed
    /// under; it is released only once a later epoch proves a refresh ran.
    deferred: Option<(WaitResult, u64)>,
}

impl PendingWait {
    pub(crate) fn new(
        request: AgentAction,
        selector: String,
        state: SelectorState,
        timeout_millis: Option<u64>,
        queued_for: Duration,
        generation: u64,
        now: Instant,
    ) -> Self {
        let started = now.checked_sub(queued_for).unwrap_or(now);
        Self {
            request,
            selector,
            state,
            started,
            deadline: started + Duration::from_millis(timeout_millis.unwrap_or(DEFAULT_WAIT_TIMEOUT_MILLIS)),
            next_poll: now,
            generation_at_start: generation,
            polls: 0,
            last_match_count: 0,
            consecutive_failures: 0,
            blocked_by_navigation: false,
            deferred: None,
        }
    }

    /// Record that an in-flight navigation is blocking observation.
    pub(crate) fn block_for_navigation(&mut self) {
        self.blocked_by_navigation = true;
    }

    /// Whether an in-flight navigation blocked this wait earlier.
    pub(crate) fn blocked_by_navigation(&self) -> bool {
        self.blocked_by_navigation
    }

    /// Hold a satisfied result, observed under `signal_epoch`, until the
    /// guards have been re-evaluated on fresher signals.
    pub(crate) fn defer(&mut self, result: WaitResult, signal_epoch: u64) {
        self.deferred = Some((result, signal_epoch));
    }

    /// The held result, once a coordination refresh newer than the one it
    /// was observed under has run and the guards allowed it.
    pub(crate) fn take_deferred(&mut self, signal_epoch: u64) -> Option<WaitResult> {
        match self.deferred.take() {
            Some((result, observed_epoch)) if observed_epoch != signal_epoch => Some(result),
            other => {
                self.deferred = other;
                None
            }
        }
    }

    pub(crate) fn poll_due(&self, now: Instant) -> bool {
        now >= self.next_poll
    }

    /// How long one observation may block: the remaining bound, clamped so a
    /// blocking evaluation cannot overrun the deadline by more than a tick.
    pub(crate) fn observation_budget(&self, now: Instant) -> Duration {
        self.deadline
            .saturating_duration_since(now)
            .clamp(MIN_OBSERVATION_BUDGET, MAX_OBSERVATION_BUDGET)
    }

    /// Record one page observation; `Some` when the condition is met.
    pub(crate) fn observe(
        &mut self,
        generation: u64,
        revision: u64,
        nodes: Vec<BrowserNode>,
        now: Instant,
    ) -> Option<WaitResult> {
        self.polls = self.polls.saturating_add(1);
        self.next_poll = now + WAIT_POLL_INTERVAL;
        self.last_match_count = nodes.len();
        self.consecutive_failures = 0;
        if generation != self.generation_at_start {
            // The page navigated while the query was in flight: these nodes
            // belong to another document.
            return Some(self.stopped(WaitStop::NavigationInvalidated, now));
        }
        // `now` is taken after the query returned: an observation that
        // overran the bound is a timeout, whatever it saw.
        if now >= self.deadline {
            return Some(self.timed_out(now));
        }
        // Decide on every match the query returned; report at most the cap.
        if !self.state.satisfied_by(&nodes) {
            return None;
        }
        let mut nodes = nodes;
        nodes.truncate(WAIT_MAX_RESULTS as usize);
        Some(Ok(BrowserControlValue::Wait {
            wait: WaitOutcome {
                state: self.state,
                generation,
                revision,
                nodes,
                elapsed_millis: self.elapsed_millis(now),
                polls: self.polls,
            },
        }))
    }

    /// A failed observation. Only an execution-context replacement (a page
    /// mid-navigation) or an observation that timed out is retried, and only
    /// a few times in a row; every other failure (a closed backend, a missing
    /// page session, an invalid result), and a persistent one, is returned as
    /// the typed failure it is.
    pub(crate) fn observe_failure(&mut self, failure: BrowserControlFailure, now: Instant) -> Option<WaitResult> {
        self.polls = self.polls.saturating_add(1);
        self.next_poll = now + WAIT_POLL_INTERVAL;
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if !observation_failure_is_transient(&failure) || self.consecutive_failures >= MAX_TRANSIENT_FAILURES {
            return Some(Err(failure));
        }
        (now >= self.deadline).then(|| self.timed_out(now))
    }

    /// Cancel or time out the wait; `Some` when it must be completed now. A
    /// satisfied result held for guard re-evaluation is still invalidated by
    /// a navigation, but the deadline no longer applies to it: it was met in
    /// time.
    pub(crate) fn tick(&self, generation: u64, now: Instant) -> Option<WaitResult> {
        if generation != self.generation_at_start {
            return Some(self.stopped(WaitStop::NavigationInvalidated, now));
        }
        if self.deferred.is_some() {
            return None;
        }
        (now >= self.deadline).then(|| self.timed_out(now))
    }

    fn timed_out(&self, now: Instant) -> WaitResult {
        let bound_millis =
            u64::try_from(self.deadline.saturating_duration_since(self.started).as_millis()).unwrap_or(u64::MAX);
        Err(BrowserControlFailure::new(
            "wait_timeout",
            format!(
                "selector did not become {} within the {} ms bound (elapsed {} ms, {} observations, last match count {})",
                state_name(self.state),
                bound_millis,
                self.elapsed_millis(now),
                self.polls,
                self.last_match_count
            ),
        ))
    }

    pub(crate) fn stopped(&self, stop: WaitStop, now: Instant) -> WaitResult {
        let (code, reason) = match stop {
            WaitStop::NavigationInvalidated => ("wait_navigation_invalidated", "the page navigated while waiting"),
            WaitStop::OwnershipLost => ("wait_ownership_lost", "the agent lost the panel lease while waiting"),
            WaitStop::HandoffPending => ("wait_handoff_pending", "a handoff to the user is pending"),
            WaitStop::Superseded => ("wait_superseded", "a later wait replaced this one"),
        };
        Err(BrowserControlFailure::new(
            code,
            format!(
                "{reason} after {} ms ({} observations)",
                self.elapsed_millis(now),
                self.polls
            ),
        ))
    }

    fn elapsed_millis(&self, now: Instant) -> u64 {
        u64::try_from(now.saturating_duration_since(self.started).as_millis()).unwrap_or(u64::MAX)
    }
}

const fn state_name(state: SelectorState) -> &'static str {
    match state {
        SelectorState::Present => "present",
        SelectorState::Visible => "visible",
        SelectorState::Hidden => "hidden",
    }
}

/// Whether an observation failure is worth retrying: the drivers fold every
/// backend error into `protocol_error` (Chromium) or `javascript_error`
/// (`WebDriver`), so the message decides. A replaced execution context (the
/// page is navigating) and an observation that timed out are transient; a
/// closed backend, a missing page session, or a script error are not.
fn observation_failure_is_transient(failure: &BrowserControlFailure) -> bool {
    if !matches!(failure.code.as_str(), "protocol_error" | "javascript_error") {
        return false;
    }
    let message = failure.message.to_ascii_lowercase();
    ["context", "timed out", "timeout", "unloaded", "navigat"]
        .iter()
        .any(|needle| message.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrowserControlAction;

    fn node(visible: bool) -> BrowserNode {
        BrowserNode {
            reference: "g1s1e1".to_string(),
            role: "paragraph".to_string(),
            name: String::new(),
            text: "late".to_string(),
            visible,
            enabled: true,
            bounds: None,
        }
    }

    fn pending(state: SelectorState, timeout_millis: u64, now: Instant) -> PendingWait {
        let request = AgentAction {
            action_id: "wait-1".to_string(),
            actor: "horizon:agent".to_string(),
            requested_at_millis: 0,
            action: BrowserControlAction::WaitForSelector {
                selector: "#late".to_string(),
                state,
                timeout_millis: Some(timeout_millis),
            },
        };
        PendingWait::new(
            request,
            "#late".to_string(),
            state,
            Some(timeout_millis),
            Duration::ZERO,
            7,
            now,
        )
    }

    fn outcome(result: Option<WaitResult>) -> WaitOutcome {
        match result.expect("settled").expect("satisfied") {
            BrowserControlValue::Wait { wait } => wait,
            other => panic!("unexpected value {other:?}"),
        }
    }

    #[test]
    fn a_wait_settles_when_the_selector_reaches_the_requested_state() {
        let now = Instant::now();
        let mut wait = pending(SelectorState::Visible, 5_000, now);
        assert!(wait.poll_due(now));
        assert!(wait.observe(7, 1, Vec::new(), now).is_none(), "no match keeps waiting");
        assert!(
            !wait.poll_due(now + Duration::from_millis(50)),
            "observations are paced"
        );
        assert!(wait.poll_due(now + WAIT_POLL_INTERVAL));
        assert!(
            wait.observe(7, 2, vec![node(false)], now + Duration::from_millis(100))
                .is_none(),
            "a hidden match does not satisfy visible"
        );
        let satisfied = outcome(wait.observe(7, 3, vec![node(true)], now + Duration::from_millis(1_500)));
        assert_eq!(satisfied.state, SelectorState::Visible);
        assert_eq!(satisfied.polls, 3);
        assert_eq!(satisfied.elapsed_millis, 1_500);
        assert_eq!(satisfied.nodes.len(), 1);

        let mut hidden = pending(SelectorState::Hidden, 5_000, now);
        assert!(hidden.observe(7, 1, vec![node(true)], now).is_none());
        assert!(
            outcome(hidden.observe(7, 2, Vec::new(), now)).nodes.is_empty(),
            "an empty match set is hidden"
        );

        let mut present = pending(SelectorState::Present, 5_000, now);
        assert_eq!(outcome(present.observe(7, 1, vec![node(false)], now)).polls, 1);
    }

    #[test]
    fn timeouts_and_cancellations_are_typed_failures() {
        let now = Instant::now();
        let mut wait = pending(SelectorState::Present, 1_000, now);
        assert!(wait.observe(7, 1, Vec::new(), now).is_none());
        assert!(wait.tick(7, now + Duration::from_millis(999)).is_none());
        let timeout = wait
            .tick(7, now + Duration::from_millis(1_000))
            .expect("settled")
            .expect_err("timed out");
        assert_eq!(timeout.code, "wait_timeout");
        assert!(timeout.message.contains("1 observations"), "{}", timeout.message);

        let invalidated = wait.tick(8, now).expect("settled").expect_err("cancelled");
        assert_eq!(invalidated.code, "wait_navigation_invalidated");
        assert_eq!(
            wait.stopped(WaitStop::OwnershipLost, now).expect_err("cancelled").code,
            "wait_ownership_lost"
        );
        assert_eq!(
            wait.stopped(WaitStop::HandoffPending, now).expect_err("cancelled").code,
            "wait_handoff_pending"
        );
        assert_eq!(
            wait.stopped(WaitStop::Superseded, now).expect_err("cancelled").code,
            "wait_superseded"
        );

        let transient = BrowserControlFailure::new("javascript_error", "Execution context was destroyed");
        assert!(
            wait.observe_failure(transient, now).is_none(),
            "transient failures are retried"
        );
        let invalid = BrowserControlFailure::new("invalid_selector", "bad selector");
        assert_eq!(
            wait.observe_failure(invalid, now)
                .expect("settled")
                .expect_err("failed")
                .code,
            "invalid_selector"
        );

        let queued = PendingWait::new(
            wait.request.clone(),
            "#late".to_string(),
            SelectorState::Present,
            Some(1_000),
            Duration::from_millis(400),
            7,
            now,
        );
        assert!(queued.tick(7, now + Duration::from_millis(599)).is_none());
        assert!(
            queued.tick(7, now + Duration::from_millis(600)).is_some(),
            "queue latency counts against the bound"
        );
    }

    #[test]
    fn the_condition_counts_every_match_and_the_outcome_is_capped() {
        let now = Instant::now();
        let mut nodes: Vec<BrowserNode> = (0..25).map(|_| node(false)).collect();
        nodes.push(node(true));
        let mut visible = pending(SelectorState::Visible, 1_000, now);
        let result = visible
            .observe(7, 1, nodes.clone(), now)
            .expect("a visible match beyond the cap counts");
        match result.expect("satisfied") {
            BrowserControlValue::Wait { wait } => assert_eq!(wait.nodes.len(), WAIT_MAX_RESULTS as usize),
            other => panic!("unexpected value {other:?}"),
        }
        let mut hidden = pending(SelectorState::Hidden, 1_000, now);
        assert!(
            hidden.observe(7, 1, nodes, now).is_none(),
            "a visible match beyond the cap keeps hidden unsatisfied"
        );
    }

    #[test]
    fn an_observation_from_another_page_generation_invalidates_the_wait() {
        let now = Instant::now();
        let mut pending = pending(SelectorState::Present, 1_000, now);
        let result = pending.observe(7 + 1, 1, vec![node(true)], now).expect("settled");
        assert_eq!(result.expect_err("invalidated").code, "wait_navigation_invalidated");
    }

    #[test]
    fn observations_are_budgeted_and_rechecked_against_the_deadline() {
        let now = Instant::now();
        let pending_wait = pending(SelectorState::Present, 1_000, now);
        assert_eq!(pending_wait.observation_budget(now), Duration::from_millis(1_000));
        assert_eq!(
            pending_wait.observation_budget(now + Duration::from_millis(900)),
            MIN_OBSERVATION_BUDGET,
            "the last observation still gets a tick"
        );
        let long = pending(SelectorState::Present, 60_000, now);
        assert_eq!(long.observation_budget(now), MAX_OBSERVATION_BUDGET);

        // A query that returned after the bound is a timeout even when it saw
        // the element, and a transient failure after the bound is one too.
        let late = now + Duration::from_millis(1_001);
        let mut overran = pending(SelectorState::Present, 1_000, now);
        let result = overran.observe(7, 1, vec![node(true)], late).expect("settled");
        let failure = result.expect_err("timed out");
        assert_eq!(failure.code, "wait_timeout");
        assert!(
            failure.message.contains("within the 1000 ms bound (elapsed 1001 ms"),
            "the message names the configured bound and the elapsed time separately: {}",
            failure.message
        );
        let mut failed_late = pending(SelectorState::Present, 1_000, now);
        let failure = BrowserControlFailure::new("protocol_error", "evaluate timed out");
        assert_eq!(
            failed_late
                .observe_failure(failure, late)
                .expect("settled")
                .expect_err("timed out")
                .code,
            "wait_timeout"
        );
        let mut failed_early = pending(SelectorState::Present, 1_000, now);
        assert!(
            failed_early
                .observe_failure(
                    BrowserControlFailure::new("protocol_error", "Execution context was destroyed"),
                    now
                )
                .is_none(),
            "a transient failure inside the bound is retried"
        );
    }

    #[test]
    fn only_retryable_failures_are_retried_and_only_a_few_times() {
        let now = Instant::now();
        let mut pending_wait = pending(SelectorState::Present, 10_000, now);
        assert!(
            pending_wait
                .observe_failure(
                    BrowserControlFailure::new("javascript_error", "Execution context was destroyed"),
                    now
                )
                .is_none(),
            "a replaced execution context is retried"
        );
        assert!(
            pending_wait
                .observe_failure(
                    BrowserControlFailure::new("protocol_error", "Runtime.evaluate timed out after 250 ms"),
                    now
                )
                .is_none(),
            "an observation timeout is retried"
        );
        for (code, message) in [
            ("protocol_error", "CDP connection closed"),
            ("protocol_error", "no page session for Runtime.evaluate"),
            ("javascript_error", "ReferenceError: foo is not defined"),
            ("invalid_result", "not a node list"),
        ] {
            let mut fresh = pending(SelectorState::Present, 10_000, now);
            let surfaced = fresh
                .observe_failure(BrowserControlFailure::new(code, message), now)
                .expect("settled");
            assert_eq!(surfaced.expect_err("failed").code, code, "{message} is permanent");
        }

        let mut persistent = pending(SelectorState::Present, 10_000, now);
        for _ in 1..MAX_TRANSIENT_FAILURES {
            assert!(
                persistent
                    .observe_failure(BrowserControlFailure::new("protocol_error", "evaluate timed out"), now)
                    .is_none()
            );
        }
        let surfaced = persistent
            .observe_failure(BrowserControlFailure::new("protocol_error", "evaluate timed out"), now)
            .expect("settled");
        assert_eq!(surfaced.expect_err("failed").code, "protocol_error");

        // A satisfied result held for guard re-evaluation survives the
        // deadline passing meanwhile; a navigation still invalidates it.
        let mut deferred = pending(SelectorState::Present, 1_000, now);
        let result = deferred.observe(7, 1, vec![node(true)], now).expect("satisfied");
        deferred.defer(result, 3);
        assert!(
            deferred.tick(7, now + Duration::from_secs(2)).is_none(),
            "the deadline does not apply to a held result"
        );
        assert!(
            deferred.tick(8, now + Duration::from_secs(2)).is_some(),
            "a navigation still invalidates it"
        );
        assert!(
            deferred.take_deferred(3).is_none(),
            "no refresh has run since the observation"
        );
        assert!(deferred.take_deferred(4).is_some(), "a later refresh releases it");
        assert!(deferred.take_deferred(5).is_none());
    }
}
