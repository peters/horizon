use super::*;

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
