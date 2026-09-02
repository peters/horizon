//! Backend-neutral bookkeeping for a selector wait observed inside the
//! driver loop: one audited action that polls the page at a bounded cadence
//! and settles on the selector state, the deadline, or a cancellation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::semantic::ScanSummary;
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
/// Most matching nodes a wait reports in its outcome; the observation scan
/// returns no more than this while its summary counts every match.
pub(crate) const WAIT_MAX_RESULTS: u32 = 20;
/// How long one pre-release document check (the drivers' final identity
/// read before a held match is returned) may block. The condition was met
/// in time, so this check is bounded by its own budget and retry count, not
/// by the wait's remaining time.
pub(crate) const RELEASE_CHECK_BUDGET: Duration = Duration::from_secs(1);

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
    /// The browser backend stopped before the wait reached a terminal state.
    BrowserUnavailable,
}

pub(crate) type WaitResult = Result<BrowserControlValue, BrowserControlFailure>;

/// Outcome of work guarded by process-liveness checks immediately before and
/// after it runs.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum BackendChecked<T> {
    Available(T),
    Unavailable,
}

/// Run one wait step only while its backend process is live. The second poll
/// catches a process that exits during a blocking page observation, before a
/// transport error can be published as the terminal wait result.
pub(crate) fn run_while_backend_available<B, T>(
    backend: &mut B,
    mut is_unavailable: impl FnMut(&mut B) -> bool,
    operation: impl FnOnce(&mut B) -> T,
) -> BackendChecked<T> {
    if is_unavailable(backend) {
        return BackendChecked::Unavailable;
    }
    let result = operation(backend);
    if is_unavailable(backend) {
        BackendChecked::Unavailable
    } else {
        BackendChecked::Available(result)
    }
}

/// A normal close can cancel an observation that was already in flight. Keep
/// the wait pending in that case so the driver's shutdown path publishes the
/// stable `browser_unavailable` outcome instead of the backend cancellation.
pub(crate) fn defer_result_during_shutdown(
    stop_requested: &AtomicBool,
    result: Option<WaitResult>,
) -> Option<WaitResult> {
    if stop_requested.load(Ordering::Acquire) {
        None
    } else {
        result
    }
}

/// What one observation concluded.
#[derive(Debug)]
pub(crate) enum Observation {
    /// The condition is not met yet.
    Waiting,
    /// The condition is met; the scan is deferred until the guards re-ran.
    Satisfied,
    /// The wait completed (invalidated or timed out) during the observation.
    Done(WaitResult),
}

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
    /// The raw scan of a satisfied observation, held until the driver has
    /// re-read the coordination signals and drained backend events, so a
    /// lease or handoff change (or a navigation) during the blocking
    /// observation still cancels the wait. Stored with the signal epoch it
    /// was observed under; it is released, and only then registered as
    /// semantic references, once a later epoch proves a refresh ran.
    deferred: Option<(serde_json::Value, u64)>,
    /// When the observation that satisfied the condition was judged; the
    /// reported elapsed time ends there, not at the later release.
    satisfied_at: Option<Instant>,
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
            satisfied_at: None,
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

    /// Hold the raw scan of a satisfied observation, made under
    /// `signal_epoch`, until the guards have been re-evaluated on fresher
    /// signals.
    pub(crate) fn defer(&mut self, scan: serde_json::Value, signal_epoch: u64) {
        self.deferred = Some((scan, signal_epoch));
    }

    /// The held scan, once a coordination refresh newer than the one it was
    /// observed under has run and the guards allowed it; the driver registers
    /// it and builds the outcome with [`Self::outcome`].
    pub(crate) fn take_deferred(&mut self, signal_epoch: u64) -> Option<serde_json::Value> {
        match self.deferred.take() {
            Some((scan, observed_epoch)) if observed_epoch != signal_epoch => Some(scan),
            other => {
                self.deferred = other;
                None
            }
        }
    }

    /// Whether the held scan has survived a later coordination refresh and
    /// is ready for the driver's final document-identity guard.
    pub(crate) fn deferred_ready(&self, signal_epoch: u64) -> bool {
        matches!(self.deferred, Some((_, observed_epoch)) if observed_epoch != signal_epoch)
    }

    /// The satisfied outcome from the registered nodes of the released scan.
    /// `elapsed_millis` ends when the condition was observed, not at the
    /// later release after the coordination refresh.
    pub(crate) fn outcome(
        &self,
        generation: u64,
        revision: u64,
        mut nodes: Vec<BrowserNode>,
        now: Instant,
    ) -> BrowserControlValue {
        nodes.truncate(WAIT_MAX_RESULTS as usize);
        BrowserControlValue::Wait {
            wait: WaitOutcome {
                state: self.state,
                generation,
                revision,
                nodes,
                elapsed_millis: self.elapsed_millis(self.satisfied_at.unwrap_or(now)),
                polls: self.polls,
            },
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

    /// Judge one page observation made without registering references. The
    /// scan's summary counts every match, beyond the capped nodes, so a
    /// visible match past the cap still satisfies `visible` and keeps
    /// `hidden` unsatisfied.
    pub(crate) fn observe(
        &mut self,
        generation: u64,
        nodes: &[BrowserNode],
        summary: Option<ScanSummary>,
        now: Instant,
    ) -> Observation {
        self.polls = self.polls.saturating_add(1);
        self.next_poll = now + WAIT_POLL_INTERVAL;
        self.last_match_count = summary.map_or(nodes.len(), |summary| summary.matched);
        self.consecutive_failures = 0;
        if generation != self.generation_at_start {
            // The page navigated while the query was in flight: these nodes
            // belong to another document.
            return Observation::Done(self.stopped(WaitStop::NavigationInvalidated, now));
        }
        // `now` is taken after the query returned: an observation that
        // overran the bound is a timeout, whatever it saw.
        if now >= self.deadline {
            return Observation::Done(self.timed_out(now));
        }
        // Decide on every match the scan saw; the released scan is registered
        // and capped by the driver.
        let satisfied = match summary {
            Some(summary) => match self.state {
                SelectorState::Present => summary.matched > 0,
                SelectorState::Visible => summary.visible > 0,
                SelectorState::Hidden => summary.visible == 0,
            },
            None => self.state.satisfied_by(nodes),
        };
        if satisfied {
            self.satisfied_at = Some(now);
            Observation::Satisfied
        } else {
            Observation::Waiting
        }
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

    /// A failed pre-release check on a held match. The condition was met in
    /// time, so the wait deadline no longer applies: a retryable failure is
    /// tried again on the next tick, up to [`MAX_TRANSIENT_FAILURES`] in a
    /// row, and every other failure, or a persistent one, is returned as is.
    pub(crate) fn release_check_failure(&mut self, failure: BrowserControlFailure) -> Option<WaitResult> {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        (!observation_failure_is_transient(&failure) || self.consecutive_failures >= MAX_TRANSIENT_FAILURES)
            .then_some(Err(failure))
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
            WaitStop::BrowserUnavailable => ("browser_unavailable", "the browser stopped while waiting"),
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
/// page is navigating), an observation that timed out, and a transport
/// disconnect that may precede process waitability are transient. Persistent
/// disconnects, a missing page session, and script errors still settle at the
/// bounded retry count.
fn observation_failure_is_transient(failure: &BrowserControlFailure) -> bool {
    if !matches!(failure.code.as_str(), "protocol_error" | "javascript_error") {
        return false;
    }
    let message = failure.message.to_ascii_lowercase();
    // "temporarily unavailable" is the Unix read-deadline wording
    // (`WouldBlock`) a bounded WebDriver observation surfaces as.
    [
        "context",
        "broken pipe",
        "connection closed",
        "connection refused",
        "connection reset",
        "missing header terminator",
        "target closed",
        "timed out",
        "timeout",
        "temporarily unavailable",
        "unexpected end of file",
        "unexpected eof",
        "unloaded",
        "navigat",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrowserControlAction;

    fn node(visible: bool) -> BrowserNode {
        BrowserNode {
            reference: String::new(),
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

    fn scan() -> serde_json::Value {
        serde_json::json!({ "nodes": [] })
    }

    fn done(observation: Observation) -> WaitResult {
        match observation {
            Observation::Done(result) => result,
            other => panic!("expected a completed wait, got {other:?}"),
        }
    }

    #[test]
    fn shutdown_defers_an_inflight_observation_result_for_terminal_settlement() {
        let stop_requested = AtomicBool::new(false);
        let failure = || BrowserControlFailure::new("protocol_error", "backend observation cancelled");

        assert!(
            defer_result_during_shutdown(&stop_requested, Some(Err(failure())))
                .expect("running driver keeps the result")
                .is_err()
        );
        stop_requested.store(true, Ordering::Release);
        assert!(defer_result_during_shutdown(&stop_requested, Some(Err(failure()))).is_none());
    }

    #[test]
    fn backend_liveness_guard_skips_work_after_an_observed_exit() {
        #[derive(Default)]
        struct Backend {
            stopped: bool,
            operations: u32,
        }

        let mut backend = Backend {
            stopped: true,
            operations: 0,
        };
        let result = run_while_backend_available(
            &mut backend,
            |backend| backend.stopped,
            |backend| backend.operations += 1,
        );

        assert_eq!(result, BackendChecked::Unavailable);
        assert_eq!(backend.operations, 0);
    }

    #[test]
    fn backend_liveness_guard_discards_a_result_when_exit_wins_the_observation_race() {
        #[derive(Default)]
        struct Backend {
            stopped: bool,
            operations: u32,
        }

        let mut backend = Backend::default();
        let result = run_while_backend_available(
            &mut backend,
            |backend| backend.stopped,
            |backend| {
                backend.operations += 1;
                backend.stopped = true;
                "transport error"
            },
        );

        assert_eq!(result, BackendChecked::Unavailable);
        assert_eq!(backend.operations, 1);
    }

    #[test]
    fn backend_liveness_guard_preserves_a_result_while_running() {
        let mut stopped = false;
        let result = run_while_backend_available(&mut stopped, |stopped| *stopped, |_| 7_u8);

        assert_eq!(result, BackendChecked::Available(7));
    }

    fn released(wait: &PendingWait, nodes: Vec<BrowserNode>, now: Instant) -> WaitOutcome {
        match wait.outcome(7, 1, nodes, now) {
            BrowserControlValue::Wait { wait } => wait,
            other => panic!("unexpected value {other:?}"),
        }
    }

    #[test]
    fn a_wait_settles_when_the_selector_reaches_the_requested_state() {
        let now = Instant::now();
        let mut wait = pending(SelectorState::Visible, 5_000, now);
        assert!(wait.poll_due(now));
        assert!(
            matches!(wait.observe(7, &[], None, now), Observation::Waiting),
            "no match keeps waiting"
        );
        assert!(
            !wait.poll_due(now + Duration::from_millis(50)),
            "observations are paced"
        );
        assert!(wait.poll_due(now + WAIT_POLL_INTERVAL));
        assert!(
            matches!(
                wait.observe(7, &[node(false)], None, now + Duration::from_millis(100)),
                Observation::Waiting
            ),
            "a hidden match does not satisfy visible"
        );
        assert!(matches!(
            wait.observe(7, &[node(true)], None, now + Duration::from_millis(1_500)),
            Observation::Satisfied
        ));
        // Released later, after the coordination refresh: the elapsed time
        // still ends at the observation that met the condition.
        let satisfied = released(&wait, vec![node(true)], now + Duration::from_millis(1_900));
        assert_eq!(satisfied.state, SelectorState::Visible);
        assert_eq!(satisfied.polls, 3);
        assert_eq!(satisfied.elapsed_millis, 1_500);
        assert_eq!(satisfied.nodes.len(), 1);

        let mut hidden = pending(SelectorState::Hidden, 5_000, now);
        assert!(matches!(
            hidden.observe(7, &[node(true)], None, now),
            Observation::Waiting
        ));
        assert!(
            matches!(hidden.observe(7, &[], None, now), Observation::Satisfied),
            "an empty match set is hidden"
        );

        let mut present = pending(SelectorState::Present, 5_000, now);
        assert!(matches!(
            present.observe(7, &[node(false)], None, now),
            Observation::Satisfied
        ));
        assert_eq!(released(&present, vec![node(false)], now).polls, 1);
    }

    #[test]
    fn timeouts_and_cancellations_are_typed_failures() {
        let now = Instant::now();
        let mut wait = pending(SelectorState::Present, 1_000, now);
        assert!(matches!(wait.observe(7, &[], None, now), Observation::Waiting));
        assert!(wait.tick(7, now + Duration::from_millis(999)).is_none());
        let timeout = wait
            .tick(7, now + Duration::from_secs(1))
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
        assert_eq!(
            wait.stopped(WaitStop::BrowserUnavailable, now)
                .expect_err("cancelled")
                .code,
            "browser_unavailable"
        );

        let transient = BrowserControlFailure::new("javascript_error", "Execution context was destroyed");
        assert!(
            wait.observe_failure(transient, now).is_none(),
            "transient failures are retried"
        );
        for (code, message) in [
            (
                "protocol_error",
                "websocket connection closed while waiting for Runtime.evaluate",
            ),
            ("javascript_error", "WebDriver HTTP I/O: Connection reset by peer"),
            (
                "javascript_error",
                "invalid WebDriver HTTP response: missing header terminator",
            ),
        ] {
            let mut transport_wait = pending(SelectorState::Present, 1_000, now);
            assert!(
                transport_wait
                    .observe_failure(BrowserControlFailure::new(code, message), now)
                    .is_none(),
                "a transport disconnect gets one more liveness pass: {message}"
            );
        }
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
        assert!(
            matches!(visible.observe(7, &nodes, None, now), Observation::Satisfied),
            "a visible match beyond the cap counts"
        );
        assert_eq!(
            released(&visible, nodes.clone(), now).nodes.len(),
            WAIT_MAX_RESULTS as usize
        );
        let mut hidden = pending(SelectorState::Hidden, 1_000, now);
        assert!(
            matches!(hidden.observe(7, &nodes, None, now), Observation::Waiting),
            "a visible match beyond the cap keeps hidden unsatisfied"
        );

        // With the scan's summary, matches beyond the returned cap decide too:
        // 300 matches of which only the 251st is visible.
        let beyond = ScanSummary {
            matched: 300,
            visible: 1,
        };
        let capped: Vec<BrowserNode> = (0..250).map(|_| node(false)).collect();
        let mut visible_beyond = pending(SelectorState::Visible, 1_000, now);
        assert!(
            matches!(
                visible_beyond.observe(7, &capped, Some(beyond), now),
                Observation::Satisfied
            ),
            "a visible match beyond the scan cap satisfies visible"
        );
        let mut hidden_beyond = pending(SelectorState::Hidden, 1_000, now);
        assert!(
            matches!(
                hidden_beyond.observe(7, &capped, Some(beyond), now),
                Observation::Waiting
            ),
            "a visible match beyond the scan cap keeps hidden unsatisfied"
        );
        let mut present_none = pending(SelectorState::Present, 1_000, now);
        assert!(matches!(
            present_none.observe(7, &[], Some(ScanSummary { matched: 0, visible: 0 }), now),
            Observation::Waiting
        ));
    }

    #[test]
    fn an_observation_from_another_page_generation_invalidates_the_wait() {
        let now = Instant::now();
        let mut pending = pending(SelectorState::Present, 1_000, now);
        let result = done(pending.observe(7 + 1, &[node(true)], None, now));
        assert_eq!(result.expect_err("invalidated").code, "wait_navigation_invalidated");
    }

    #[test]
    fn observations_are_budgeted_and_rechecked_against_the_deadline() {
        let now = Instant::now();
        let pending_wait = pending(SelectorState::Present, 1_000, now);
        assert_eq!(pending_wait.observation_budget(now), Duration::from_secs(1));
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
        let failure = done(overran.observe(7, &[node(true)], None, late)).expect_err("timed out");
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
        assert!(
            pending_wait
                .observe_failure(
                    BrowserControlFailure::new(
                        "javascript_error",
                        "WebDriver HTTP I/O: Resource temporarily unavailable (os error 11)"
                    ),
                    now
                )
                .is_none(),
            "a Unix read deadline on a bounded WebDriver observation is retried"
        );
        for (code, message) in [
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

        let mut disconnected = pending(SelectorState::Present, 10_000, now);
        for _ in 1..MAX_TRANSIENT_FAILURES {
            assert!(
                disconnected
                    .observe_failure(
                        BrowserControlFailure::new("protocol_error", "CDP connection closed"),
                        now
                    )
                    .is_none()
            );
        }
        let disconnected = disconnected
            .observe_failure(
                BrowserControlFailure::new("protocol_error", "CDP connection closed"),
                now,
            )
            .expect("bounded retry settled")
            .expect_err("disconnect stayed failed");
        assert_eq!(disconnected.code, "protocol_error");

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
    }

    #[test]
    fn a_failed_release_check_is_retried_without_the_wait_deadline() {
        let now = Instant::now();
        let mut held = pending(SelectorState::Present, 1_000, now);
        assert!(matches!(
            held.observe(7, &[node(true)], None, now),
            Observation::Satisfied
        ));
        held.defer(scan(), 3);
        let late = now + Duration::from_secs(3);
        for _ in 1..MAX_TRANSIENT_FAILURES {
            assert!(
                held.release_check_failure(BrowserControlFailure::new(
                    "javascript_error",
                    "WebDriver HTTP I/O: Resource temporarily unavailable (os error 11)"
                ))
                .is_none(),
                "a slow identity read past the deadline is retried, not timed out"
            );
        }
        assert!(
            held.tick(7, late).is_none(),
            "the held match still ignores the deadline"
        );
        assert_eq!(held.polls, 1, "release checks are not observations");
        let surfaced = held
            .release_check_failure(BrowserControlFailure::new("javascript_error", "evaluate timed out"))
            .expect("settled");
        assert_eq!(surfaced.expect_err("failed").code, "javascript_error");

        let mut permanent = pending(SelectorState::Present, 1_000, now);
        assert!(matches!(
            permanent.observe(7, &[node(true)], None, now),
            Observation::Satisfied
        ));
        permanent.defer(scan(), 3);
        let surfaced = permanent
            .release_check_failure(BrowserControlFailure::new("invalid_result", "no document identity"))
            .expect("settled");
        assert_eq!(surfaced.expect_err("failed").code, "invalid_result");
        assert!(RELEASE_CHECK_BUDGET > MIN_OBSERVATION_BUDGET);
    }

    #[test]
    fn a_satisfied_scan_is_released_only_after_a_later_signal_refresh() {
        let now = Instant::now();
        // The deferred scan survives the deadline passing meanwhile; a
        // navigation still invalidates it; and it is released only under a
        // later coordination-read epoch than it was observed under.
        let mut deferred = pending(SelectorState::Present, 1_000, now);
        assert!(matches!(
            deferred.observe(7, &[node(true)], None, now),
            Observation::Satisfied
        ));
        deferred.defer(scan(), 3);
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

        // A wait blocked by an in-flight navigation remembers it.
        let mut blocked = pending(SelectorState::Present, 1_000, now);
        assert!(!blocked.blocked_by_navigation());
        blocked.block_for_navigation();
        assert!(blocked.blocked_by_navigation());
    }
}
