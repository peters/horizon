//! The power wait on simulated time: the production schedule and bound run exactly, so
//! the deadline arithmetic (the final-poll reserve, slow requests eating the bound, a
//! transition that completes at the very end) is proven rather than skipped.
use super::*;
use crate::cloud_run::{
    interactive_worker_start::{InteractiveWorkerStart, InteractiveWorkerStartProvider},
    interactive_worker_stop::{InteractiveWorkerStop, InteractiveWorkerStopProvider},
};

const BOUND: Duration = Duration::from_secs(300);
const RESERVE: Duration = Duration::from_secs(5);

fn secs(values: &[u64]) -> Vec<Duration> {
    values.iter().map(|value| Duration::from_secs(*value)).collect()
}

/// The polls a wait makes on an instant control plane: one per schedule step until the
/// last sleep is cut short so a final poll lands inside the reserve.
fn expected_sleeps() -> Vec<Duration> {
    let mut sleeps = secs(&[0, 1, 2, 4, 8, 15]);
    sleeps.extend(secs(&[30; 8]));
    sleeps.push(Duration::from_secs(25));
    sleeps
}

fn states(before: usize, then: &str, s: &Scenario) -> Vec<Option<AzureVmView>> {
    let mut states = vec![Some(vm("deallocated", &s.tags))];
    states.extend((0..before).map(|_| Some(vm("starting", &s.tags))));
    states.push(Some(vm(then, &s.tags)));
    states
}

#[test]
fn the_wait_follows_the_schedule_and_polls_once_more_inside_the_reserve() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    // Stuck in `starting` for the whole bound: every scheduled poll happens, the last
    // one inside the reserve, and nothing is claimed.
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "203.0.113.9")),
        states(40, "starting", &s),
    );
    let begun = s.clock.instant();
    assert_eq!(client.start_worker(&worker), Err(AzureError::StartUnverified));
    assert_eq!(
        s.clock.sleeps(),
        expected_sleeps(),
        "the production schedule, last sleep cut to the reserve"
    );
    let elapsed = s.clock.instant().saturating_duration_since(begun);
    assert!(
        elapsed >= BOUND.saturating_sub(RESERVE) && elapsed <= BOUND,
        "the last poll lands inside the reserve: {elapsed:?}"
    );
    let polls = s
        .plane
        .calls()
        .iter()
        .filter(|call| matches!(call, Call::GetVm(_)))
        .count();
    assert_eq!(
        polls,
        1 + expected_sleeps().len(),
        "one observation and one poll per sleep; an unverified start is never re-observed"
    );
}

#[test]
fn a_transition_that_completes_at_the_very_end_of_the_bound_is_still_observed() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    let final_poll = expected_sleeps().len() - 1;
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "203.0.113.9")),
        states(final_poll, "running", &s),
    );
    let started = client.start_worker(&worker).expect("start");
    assert!(matches!(started, InteractiveWorkerStart::Started(_)), "{started:?}");
    assert_eq!(s.clock.sleeps(), expected_sleeps());
    // One step later is one step too late: the final poll saw `starting`, and the wait
    // never polls past its deadline.
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "203.0.113.9")),
        states(final_poll + 1, "running", &s),
    );
    assert_eq!(client.start_worker(&worker), Err(AzureError::StartUnverified));
    assert_eq!(
        s.plane.mutations(),
        vec![Call::Start(s.group.clone())],
        "posted once, never re-posted"
    );
}

#[test]
fn slow_requests_spend_the_bound_and_never_get_a_budget_past_the_deadline() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    // Every bounded lookup takes 100 s: the first poll spends 200 s, the next sleep is
    // one second, the following group lookup is handed the 99 s left and uses them all,
    // and the wait ends without another request (the fake asserts every budget it gets).
    s.plane.lock().request_takes = Duration::from_secs(100);
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "203.0.113.9")),
        states(1, "running", &s),
    );
    assert_eq!(client.start_worker(&worker), Err(AzureError::StartUnverified));
    assert_eq!(s.clock.sleeps(), secs(&[0, 1]));
    let vm_polls = s
        .plane
        .calls()
        .iter()
        .filter(|call| matches!(call, Call::GetVm(_)))
        .count();
    assert_eq!(vm_polls, 2, "the observation and the one poll that fit in the bound");
    // Each bounded lookup was handed exactly what was left: the group lookup at 0 s,
    // the VM lookup at 100 s, the group lookup at 201 s; nothing after 300 s.
    assert_eq!(s.plane.budgets(), secs(&[300, 200, 99]));
}

#[test]
fn every_poll_is_handed_exactly_what_is_left_of_the_bound() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    s.plane.lock().request_takes = Duration::from_millis(500);
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "203.0.113.9")),
        states(40, "starting", &s),
    );
    assert_eq!(client.start_worker(&worker), Err(AzureError::StartUnverified));
    // Two lookups per poll, each half a second: the budgets walk down from the bound by
    // the sleeps and the request times alone, and the wait never asks past the bound.
    let mut expected = Vec::new();
    let mut left = BOUND;
    for sleep in s.clock.sleeps() {
        left = left.saturating_sub(sleep);
        expected.push(left);
        left = left.saturating_sub(Duration::from_millis(500));
        expected.push(left);
        left = left.saturating_sub(Duration::from_millis(500));
    }
    assert_eq!(s.plane.budgets(), expected);
    assert!(expected.iter().all(|budget| !budget.is_zero() && *budget <= BOUND));
    assert!(
        *expected.last().expect("polls") <= RESERVE,
        "the last poll runs inside the reserve"
    );
}

#[test]
fn the_stop_wait_runs_on_the_same_schedule() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    let final_poll = expected_sleeps().len() - 1;
    let mut states = vec![Some(vm("running", &s.tags))];
    states.extend((0..final_poll).map(|_| Some(vm("deallocating", &s.tags))));
    states.push(Some(vm("deallocated", &s.tags)));
    s.plane
        .script(Some(owned(&s)), Some(deployment("Succeeded", "203.0.113.9")), states);
    let begun = s.clock.instant();
    assert_eq!(client.stop_worker(&worker), Ok(InteractiveWorkerStop::Stopped));
    assert_eq!(s.clock.sleeps(), expected_sleeps());
    assert!(s.clock.instant().saturating_duration_since(begun) <= BOUND);
}
