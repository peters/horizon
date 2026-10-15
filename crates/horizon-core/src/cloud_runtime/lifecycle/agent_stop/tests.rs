//! How Horizon takes an agent's stop request from a worker that cannot stop itself.
use super::*;
use std::{cell::RefCell, time::Duration};

const STOP: &str = r#"{"at":1792017958386,"reason":"PR 12 merged","session":"agent-a","agent":"claude"}"#;

fn stop() -> AgentStop {
    AgentStop {
        at: 1_792_017_958_386,
        reason: "PR 12 merged".into(),
        session: "agent-a".into(),
        agent: "claude".into(),
        worker_id: "42".into(),
        worker: None,
    }
}

/// A ready Hetzner cloud on worker `42`, as `adjust` leaves it.
fn ready(root: &Path, adjust: impl FnOnce(&mut Deployment)) {
    let mut state: Deployment = serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "stop-test", "repository": "/fixture", "revision": "a".repeat(40),
        "profile": {"provider": "hetzner", "image": "registry.example/worker", "cpu": 2, "memory_gb": 4},
        "stage": "Ready", "operation": {"state": "bound", "worker_id": "42"}, "spec": null,
        "worker": {"id": "42", "name": "horizon-cloud-stop-test", "imageName": "registry.example/worker@sha256:1",
                   "desiredStatus": "RUNNING", "publicIp": "192.0.2.10", "portMappings": {"22": 22}},
        "sessions": [], "stop_requested": false
    }))
    .unwrap();
    adjust(&mut state);
    Store::lock(root).unwrap().save(&state).unwrap();
}

fn stored(root: &Path) -> Deployment {
    Store::lock(root).unwrap().load().unwrap().unwrap()
}

/// A stop that records the cloud as stopped, as Stop does.
fn stops(store: &Store) -> Result<()> {
    let mut state = store.load()?.unwrap();
    state.stage = Stage::Stopped;
    state.stop_requested = true;
    store.save(&state)
}

/// The stop's provider work settled: nothing it sent may have taken effect.
fn settled(_: &Error) -> bool {
    false
}

fn never(_: &Store) -> Result<()> {
    panic!("the cloud is not stopped")
}

#[test]
fn only_a_ready_hetzner_or_docker_host_cloud_is_read() {
    let unread = |_: &Worker, _: &str| -> Result<String> { panic!("the worker is not read") };
    let adjustments: [fn(&mut Deployment); 4] = [
        |state| state.stage = Stage::Stopped,
        |state| state.stop_requested = true,
        |state| state.profile.provider = "runpod".into(),
        |state| state.operation = CreateState::Prepared,
    ];
    for adjust in adjustments {
        let root = tempfile::tempdir().unwrap();
        ready(root.path(), adjust);
        assert_eq!(read_with(root.path(), unread).unwrap(), Requested::Nothing);
    }
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(read_with(empty.path(), unread).unwrap(), Requested::Nothing);
    let root = tempfile::tempdir().unwrap();
    ready(root.path(), |state| state.profile.provider = "docker-host".into());
    assert!(takes_stop_requests(&stored(root.path())));
}

#[test]
fn the_worker_answer_is_read_as_no_request_a_waiting_or_refused_one_or_a_stop() {
    for (answer, requested) in [
        ("", Requested::Nothing),
        ("null", Requested::Nothing),
        (
            r#"{"waiting":"An agent session produced output"}"#,
            Requested::Waiting("An agent session produced output".into()),
        ),
        (
            r#"{"refused":"The stop request lapsed"}"#,
            Requested::Refused("The stop request lapsed".into()),
        ),
        (STOP, Requested::Stop(stop())),
    ] {
        let root = tempfile::tempdir().unwrap();
        ready(root.path(), |_| {});
        let run = |worker: &Worker, command: &str| {
            assert_eq!((worker.id.as_str(), command), ("42", TAKE));
            Ok(format!("{answer}\n"))
        };
        assert_eq!(read_with(root.path(), run).unwrap(), requested);
        assert_eq!(stored(root.path()).stage, Stage::Ready, "a read stops nothing");
    }
    let root = tempfile::tempdir().unwrap();
    ready(root.path(), |_| {});
    let malformed = |_: &Worker, _: &str| Ok(r#"{"at":"soon"}"#.to_owned());
    assert!(read_with(root.path(), malformed).is_err());
}

/// A worker that still gives `STOP` for a take and records every other command.
fn worker(commands: &RefCell<Vec<String>>) -> impl Fn(&Worker, &str) -> Result<String> + '_ {
    move |_: &Worker, command: &str| {
        if command == TAKE {
            return Ok(STOP.to_owned());
        }
        commands.borrow_mut().push(command.to_owned());
        Ok(String::new())
    }
}

fn attribution(root: &Path) -> Option<u64> {
    stored(root).last_self_stop.map(|stop| stop.at)
}

#[test]
fn a_stop_for_the_agent_stops_the_cloud_and_names_the_agent() {
    let root = tempfile::tempdir().unwrap();
    ready(root.path(), |_| {});
    let commands = RefCell::new(Vec::new());
    let state = stop_with(
        root.path(),
        &stop(),
        worker(&commands),
        stops,
        settled,
        || false,
        Duration::ZERO,
    )
    .unwrap();
    assert_eq!(state.stage, Stage::Stopped);
    let recorded = stored(root.path()).last_self_stop.unwrap();
    assert_eq!(
        (
            recorded.at,
            recorded.reason.as_str(),
            recorded.agent.as_deref(),
            recorded.session.as_deref()
        ),
        (stop().at, "PR 12 merged", Some("claude"), Some("agent-a"))
    );
    assert!(commands.borrow().is_empty(), "a stop that happened is not taken back");
}

#[test]
fn a_cloud_another_horizon_already_stopped_for_the_request_is_that_stop() {
    let root = tempfile::tempdir().unwrap();
    ready(root.path(), |_| {});
    let commands = RefCell::new(Vec::new());
    stop_with(
        root.path(),
        &stop(),
        worker(&commands),
        stops,
        settled,
        || false,
        Duration::ZERO,
    )
    .unwrap();
    let unread = |_: &Worker, _: &str| -> Result<String> { panic!("the worker is not asked") };
    let again = stop_with(root.path(), &stop(), unread, never, settled, || false, Duration::ZERO).unwrap();
    assert_eq!(again.stage, Stage::Stopped);
}

#[test]
fn a_stop_that_fails_or_a_cloud_that_changed_is_taken_back() {
    let untake = format!("horizon-worker-stopd --untake {}", stop().at);
    let commands = RefCell::new(Vec::new());
    // The stop fails before the provider is asked.
    let root = tempfile::tempdir().unwrap();
    ready(root.path(), |_| {});
    let failing = |_: &Store| Err(Error::Invalid("provider refused"));
    assert!(
        stop_with(
            root.path(),
            &stop(),
            worker(&commands),
            failing,
            settled,
            || false,
            Duration::ZERO
        )
        .is_err()
    );
    assert_eq!(*commands.borrow(), std::slice::from_ref(&untake));
    assert_eq!(stored(root.path()).stage, Stage::Ready);
    assert_eq!(
        attribution(root.path()),
        None,
        "a stop that did not happen names no agent"
    );
    // The cloud runs another worker now: that worker took no request.
    commands.borrow_mut().clear();
    let mut other = stop();
    other.worker_id = "41".into();
    assert!(
        stop_with(
            root.path(),
            &other,
            worker(&commands),
            never,
            settled,
            || false,
            Duration::ZERO
        )
        .is_err()
    );
    assert!(commands.borrow().is_empty());
    // The cloud is no longer ready on the worker that asked: nothing stops, and that
    // worker drops the request.
    let root = tempfile::tempdir().unwrap();
    ready(root.path(), |state| state.stage = Stage::Provision);
    assert!(
        stop_with(
            root.path(),
            &stop(),
            worker(&commands),
            never,
            settled,
            || false,
            Duration::ZERO
        )
        .is_err()
    );
    assert_eq!(*commands.borrow(), [untake]);
}

#[test]
fn a_cloud_another_operation_holds_is_waited_for_before_the_request_goes_back() {
    // Held longer than the stop waits: the request goes back to the worker.
    let root = tempfile::tempdir().unwrap();
    ready(root.path(), |_| {});
    let mut request = stop();
    request.worker = stored(root.path()).worker.map(Box::new);
    let held = Store::lock(root.path()).unwrap();
    let commands = RefCell::new(Vec::new());
    assert!(matches!(
        stop_with(
            root.path(),
            &request,
            worker(&commands),
            never,
            settled,
            || false,
            Duration::ZERO
        ),
        Err(Error::Busy)
    ));
    assert_eq!(
        *commands.borrow(),
        [format!("horizon-worker-stopd --untake {}", stop().at)]
    );
    commands.borrow_mut().clear();
    // Released soon after, as when another Horizon's stop for the same request ends: its
    // outcome decides.
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        drop(held);
    });
    let state = stop_with(
        root.path(),
        &request,
        worker(&commands),
        stops,
        settled,
        || false,
        Duration::from_secs(10),
    )
    .unwrap();
    releaser.join().unwrap();
    assert_eq!(state.stage, Stage::Stopped);
}

#[test]
fn a_record_that_cannot_be_saved_gives_the_request_back() {
    let root = tempfile::tempdir().unwrap();
    ready(root.path(), |_| {});
    let commands = RefCell::new(Vec::new());
    // The definite refusal's rollback cannot be saved: the request still goes back first.
    let refused_unsaved = |store: &Store| {
        std::fs::remove_dir_all(store.root()).ok();
        Err(Error::Invalid("Unauthorized"))
    };
    assert!(
        stop_with(
            root.path(),
            &stop(),
            worker(&commands),
            refused_unsaved,
            settled,
            || false,
            Duration::ZERO
        )
        .is_err()
    );
    assert_eq!(
        *commands.borrow(),
        [format!("horizon-worker-stopd --untake {}", stop().at)]
    );
}

#[test]
fn a_request_that_no_longer_holds_on_the_worker_stops_nothing() {
    for answer in [
        r#"{"refused":"The stop request lapsed before the worker stopped."}"#,
        "null",
    ] {
        let root = tempfile::tempdir().unwrap();
        ready(root.path(), |_| {});
        let run = |_: &Worker, command: &str| {
            assert_eq!(command, TAKE);
            Ok(answer.to_owned())
        };
        assert!(stop_with(root.path(), &stop(), run, never, settled, || false, Duration::ZERO).is_err());
        assert_eq!(stored(root.path()).stage, Stage::Ready);
        assert_eq!(attribution(root.path()), None);
    }
}

#[test]
fn a_stop_the_provider_refused_outright_names_no_agent_and_gives_the_request_back() {
    let root = tempfile::tempdir().unwrap();
    ready(root.path(), |_| {});
    let commands = RefCell::new(Vec::new());
    // As a Hetzner stop leaves its record: stopping, to be retried, with nothing sent
    // that took effect.
    let refused = |store: &Store| {
        let mut state = store.load()?.unwrap();
        state.stage = Stage::Stopping;
        state.stop_requested = true;
        store.save(&state)?;
        Err(Error::Invalid("Unauthorized"))
    };
    assert!(
        stop_with(
            root.path(),
            &stop(),
            worker(&commands),
            refused,
            settled,
            || false,
            Duration::ZERO
        )
        .is_err()
    );
    assert_eq!(attribution(root.path()), None);
    assert_eq!(
        *commands.borrow(),
        [format!("horizon-worker-stopd --untake {}", stop().at)]
    );
}

#[test]
fn a_stop_whose_outcome_is_uncertain_keeps_the_agent_and_the_request() {
    let root = tempfile::tempdir().unwrap();
    ready(root.path(), |_| {});
    let commands = RefCell::new(Vec::new());
    // The provider may have released the server, but its answer was lost.
    let uncertain = |store: &Store| {
        let mut state = store.load()?.unwrap();
        state.stage = Stage::Stopping;
        state.stop_requested = true;
        store.save(&state)?;
        Err(Error::Invalid("the answer was lost"))
    };
    assert!(
        stop_with(
            root.path(),
            &stop(),
            worker(&commands),
            uncertain,
            |_| true,
            || false,
            Duration::ZERO
        )
        .is_err()
    );
    assert_eq!(attribution(root.path()), Some(stop().at));
    assert!(
        commands.borrow().is_empty(),
        "a stop that may have happened is not taken back"
    );
}

#[test]
fn a_cancelled_stop_is_uncertain_only_once_a_provider_request_was_sent() {
    use crate::cloud_runtime::mutation::State::{Pending, Settled};
    let cancelled = Error::Provider(horizon_cloud::CloudError::Cancelled);
    let refused = Error::Invalid("Unauthorized");
    // Pending at the end: the request may have taken effect, whatever the error.
    assert!(uncertain_outcome(Some(Pending), true, &refused));
    // Cancelled after a request was sent, though rolled back as refused.
    assert!(uncertain_outcome(Some(Settled), true, &cancelled));
    // Cancelled before anything was sent, or refused outright.
    assert!(!uncertain_outcome(None, false, &cancelled));
    assert!(!uncertain_outcome(Some(Settled), true, &refused));
}

#[test]
fn another_horizon_still_stopping_for_the_request_is_not_reported_stopped() {
    let root = tempfile::tempdir().unwrap();
    ready(root.path(), |state| {
        state.stage = Stage::Stopping;
        state.stop_requested = true;
        state.last_self_stop = Some(crate::cloud_runtime::SelfStop {
            at: stop().at,
            reason: "PR 12 merged".into(),
            agent: Some("claude".into()),
            session: Some("agent-a".into()),
        });
    });
    let unread = |_: &Worker, _: &str| -> Result<String> { panic!("the request stays with that stop") };
    assert!(stop_with(root.path(), &stop(), unread, never, settled, || false, Duration::ZERO).is_err());
}

#[test]
fn a_stop_cancelled_while_it_waits_for_the_cloud_gives_the_request_back() {
    let root = tempfile::tempdir().unwrap();
    ready(root.path(), |_| {});
    let mut request = stop();
    request.worker = stored(root.path()).worker.map(Box::new);
    let _held = Store::lock(root.path()).unwrap();
    let commands = RefCell::new(Vec::new());
    let result = stop_with(
        root.path(),
        &request,
        worker(&commands),
        never,
        settled,
        || true,
        Duration::from_secs(30),
    );
    assert!(matches!(
        result,
        Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
    ));
    assert_eq!(
        *commands.borrow(),
        [format!("horizon-worker-stopd --untake {}", stop().at)]
    );
}
