use super::*;

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
}
