//! Backend-neutral bookkeeping for a selector wait observed inside the
//! driver loop: one audited action that polls the page at a bounded cadence
//! and settles on the selector state, the deadline, or a cancellation.

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
/// page is navigating) and an observation that timed out are transient; a
/// closed backend, a missing page session, or a script error are not.
fn observation_failure_is_transient(failure: &BrowserControlFailure) -> bool {
    if !matches!(failure.code.as_str(), "protocol_error" | "javascript_error") {
        return false;
    }
    let message = failure.message.to_ascii_lowercase();
    // "temporarily unavailable" is the Unix read-deadline wording
    // (`WouldBlock`) a bounded WebDriver observation surfaces as.
    [
        "context",
        "timed out",
        "timeout",
        "temporarily unavailable",
        "unloaded",
        "navigat",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

#[cfg(test)]
mod tests;
