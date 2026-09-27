//! How Horizon stops an idle Hetzner cloud from the idle record its worker keeps.
use super::{provider, server, spec, volume};
use crate::cloud_runtime::{
    Error, Stage,
    deployment::hetzner::{Allowed, Compute, Journal, JournalFile as _, idle::IdleCheck, idle::check_with},
    state::{Deployment, Store},
};
use horizon_cloud::{Cancellation, CreateState, Credential, hetzner::Hetzner};
use serde_json::json;
use std::{path::Path, time::Duration};

const BOUND: &str = "42";

/// A running Hetzner cloud with a ten-minute idle period, as `adjust` leaves it.
fn running(root: &Path, adjust: impl FnOnce(&mut Deployment)) {
    let mut spec = spec();
    spec.profile.idle_stop_minutes = Some(10);
    let worker =
        horizon_cloud::hetzner::cloud::worker(&server("running", Some("192.0.2.10")), &spec, &volume()).unwrap();
    let mut state: Deployment = serde_json::from_value(json!({
        "version": 1, "cloud_id": spec.operation_id, "repository": "/fixture", "revision": "a".repeat(40),
        "profile": spec.profile, "stage": "Ready", "operation": {"state": "bound", "worker_id": BOUND},
        "spec": spec, "worker": worker, "sessions": [], "stop_requested": false
    }))
    .unwrap();
    adjust(&mut state);
    Store::lock(root).unwrap().save(&state).unwrap();
    Journal {
        location: Some("hel1".into()),
        volume: CreateState::Bound { worker_id: "9".into() },
        key: Some("ssh-ed25519 AAAA".into()),
        released: None,
        deleting: false,
    }
    .save(root)
    .unwrap();
}

fn record(idle_seconds: u64) -> String {
    json!({"idle_seconds": idle_seconds, "idle_stop_seconds": 600}).to_string()
}

/// A Hetzner client that must never be asked for anything.
fn untouched() -> crate::cloud_runtime::Result<Compute> {
    panic!("no provider request is made")
}

fn compute(address: std::net::SocketAddr) -> Compute {
    Compute {
        client: Hetzner::loopback(Credential::new("secret-test-key".into()).unwrap(), address).unwrap(),
        settings: serde_json::from_value(
            json!({"token_file": "/unused", "server_types": ["cx33"], "locations": ["hel1"]}),
        )
        .unwrap(),
        allowed: Allowed {
            locations: vec!["hel1".into()],
            server_types: vec!["cx33".into()],
        },
        registries: None,
    }
}

fn stored(root: &Path) -> Deployment {
    Store::lock(root).unwrap().load().unwrap().unwrap()
}

#[test]
fn only_a_running_hetzner_cloud_with_an_idle_period_is_read() {
    let unread =
        |_: &horizon_cloud::Worker| -> crate::cloud_runtime::Result<String> { panic!("the record is not read") };
    let adjustments: [fn(&mut Deployment); 5] = [
        |state| state.profile.idle_stop_minutes = None,
        |state| state.stage = Stage::Stopped,
        |state| state.stop_requested = true,
        |state| state.profile.provider = "runpod".into(),
        |state| state.operation = CreateState::Prepared,
    ];
    for adjust in adjustments {
        let root = tempfile::tempdir().unwrap();
        running(root.path(), adjust);
        let checked = check_with(root.path(), &Cancellation::default(), unread, untouched).unwrap();
        assert_eq!(checked, IdleCheck::NotWatched);
    }
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(
        check_with(empty.path(), &Cancellation::default(), unread, untouched).unwrap(),
        IdleCheck::NotWatched
    );
}

#[test]
fn a_cloud_idle_for_less_than_its_period_keeps_running() {
    let root = tempfile::tempdir().unwrap();
    running(root.path(), |_| {});
    let read = |worker: &horizon_cloud::Worker| {
        assert_eq!(worker.id, BOUND);
        Ok(format!("{}\n", record(599)))
    };
    let checked = check_with(root.path(), &Cancellation::default(), read, untouched).unwrap();
    assert_eq!(
        checked,
        IdleCheck::Active {
            idle: Duration::from_secs(599),
            limit: Duration::from_secs(600)
        }
    );
    assert_eq!(stored(root.path()).stage, Stage::Ready);
}

#[test]
fn a_cloud_idle_for_its_whole_period_is_stopped_and_keeps_its_volume() {
    let root = tempfile::tempdir().unwrap();
    running(root.path(), |_| {});
    let off = (200, json!({"server": server("off", None)}).to_string());
    let (address, requests, task) = provider::serve(vec![
        off.clone(),
        off.clone(),
        off,
        (200, json!({"action": {"id": 3, "status": "success"}}).to_string()),
        (
            404,
            json!({"error": {"code": "not_found", "message": "not found"}}).to_string(),
        ),
    ]);
    let checked = check_with(
        root.path(),
        &Cancellation::default(),
        |_| Ok(record(600)),
        || Ok(compute(address)),
    )
    .unwrap();
    task.join().unwrap();
    assert_eq!(
        checked,
        IdleCheck::Stopped {
            idle: Duration::from_secs(600)
        }
    );
    let state = stored(root.path());
    assert!(state.stop_requested && state.stage == Stage::Stopped);
    assert_eq!(Journal::load(root.path()).unwrap().released.as_deref(), Some(BOUND));
    let requests = requests.lock().unwrap().clone();
    assert!(
        requests
            .iter()
            .any(|request| request.starts_with("DELETE ") && request.contains("/servers/42 "))
    );
    assert!(
        !requests.iter().any(|request| request.contains("/volumes")),
        "the volume is kept"
    );
}

#[test]
fn an_unusable_record_is_an_error_and_stops_nothing() {
    for (output, expected) in [
        ("{\"idle_seconds\":900}", "The worker's idle record is malformed"),
        (
            "{\"idle_seconds\":900,\"idle_stop_seconds\":600,\"extra\":1}",
            "The worker's idle record is malformed",
        ),
        ("not json", "The worker's idle record is malformed"),
        (
            "{\"idle_seconds\":900,\"idle_stop_seconds\":1800}",
            "The worker's idle period is not this cloud's; redeploy it to apply the profile's idle_stop_minutes",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        running(root.path(), |_| {});
        let error = check_with(root.path(), &Cancellation::default(), |_| Ok(output.into()), untouched).unwrap_err();
        assert_eq!(error.to_string(), expected, "{output}");
        assert_eq!(stored(root.path()).stage, Stage::Ready);
    }
    // A record that cannot be read at all, such as a stale one, is the reader's error.
    let root = tempfile::tempdir().unwrap();
    running(root.path(), |_| {});
    let unreadable = |_: &horizon_cloud::Worker| Err(Error::Command("Reading the worker's idle record"));
    assert!(check_with(root.path(), &Cancellation::default(), unreadable, untouched).is_err());
}

#[test]
fn a_cloud_that_changed_while_its_record_was_read_is_left_alone() {
    let changes: [fn(&mut Deployment); 3] = [
        // Stopped by the person meanwhile.
        |state| (state.stage, state.stop_requested) = (Stage::Stopped, true),
        // Resumed onto another server.
        |state| state.operation = CreateState::Bound { worker_id: "43".into() },
        |state| state.worker.as_mut().unwrap().public_ip = Some([192, 0, 2, 11].into()),
    ];
    for change in changes {
        let root = tempfile::tempdir().unwrap();
        running(root.path(), |_| {});
        let read = |_: &horizon_cloud::Worker| {
            let store = Store::lock(root.path()).unwrap();
            let mut state = store.load().unwrap().unwrap();
            change(&mut state);
            store.save(&state).unwrap();
            Ok(record(3600))
        };
        let checked = check_with(root.path(), &Cancellation::default(), read, untouched).unwrap();
        assert_eq!(checked, IdleCheck::NotWatched);
    }
}

#[test]
fn a_busy_cloud_is_reported_rather_than_waited_for() {
    let root = tempfile::tempdir().unwrap();
    running(root.path(), |_| {});
    let _held = Store::lock(root.path()).unwrap();
    let unread =
        |_: &horizon_cloud::Worker| -> crate::cloud_runtime::Result<String> { panic!("the record is not read") };
    assert!(matches!(
        check_with(root.path(), &Cancellation::default(), unread, untouched),
        Err(Error::Busy)
    ));
}
