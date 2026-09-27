//! How Horizon records a stop: an interrupted stop is finished rather than taken
//! for a released server. Deletion itself is tested in `horizon_cloud::hetzner::cloud`.
use super::{provider, server, spec};
use crate::cloud_runtime::{
    Stage,
    deployment::hetzner::{
        Allowed, Compute, Journal, JournalFile as _,
        lifecycle::{reconcile_with, stop_with},
        retained,
    },
    state::{Deployment, Store},
};
use horizon_cloud::{Cancellation, CreateState, Credential, hetzner::Hetzner};
use serde_json::json;

fn gone() -> (u16, String) {
    (
        404,
        json!({"error": {"code": "not_found", "message": "not found"}}).to_string(),
    )
}

/// Runs `act` on a cloud whose record is `operation` and `journal`, stopping,
/// against `responses`; returns the saved deployment, whether anything is
/// retained and the requests served.
fn act_on(
    operation: &CreateState,
    journal: &Journal,
    responses: Vec<(u16, String)>,
    act: impl FnOnce(&Compute, &Store, &mut Deployment),
) -> (Deployment, bool, Vec<String>) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let spec = spec();
    let mut state: Deployment = serde_json::from_value(json!({
        "version": 1, "cloud_id": spec.operation_id, "repository": "/fixture", "revision": "a".repeat(40),
        "profile": spec.profile, "stage": "Stopping", "operation": operation, "spec": spec,
        "worker": null, "sessions": [], "stop_requested": true
    }))
    .unwrap();
    store.save(&state).unwrap();
    journal.save(root.path()).unwrap();
    let (address, requests, task) = provider::serve(responses);
    let compute = Compute {
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
    };
    act(&compute, &store, &mut state);
    task.join().unwrap();
    let requests = requests.lock().unwrap().clone();
    (store.load().unwrap().unwrap(), retained(root.path()).unwrap(), requests)
}

fn journal(volume: CreateState) -> Journal {
    Journal {
        location: Some("hel1".into()),
        volume,
        key: Some("ssh-ed25519 AAAA".into()),
        released: None,
        deleting: false,
        vacating: false,
    }
}

#[test]
fn an_interrupted_stop_is_neither_reported_stopped_nor_resumed_until_its_server_is_gone() {
    let server = serde_json::to_value(server("off", None)).unwrap();
    let bound = CreateState::Bound { worker_id: "42".into() };
    let mut released = journal(CreateState::Bound { worker_id: "9".into() });
    released.released = Some("42".into());
    let still_there = (200, json!({"server": server}).to_string());
    let (state, _, requests) = act_on(&bound, &released, vec![still_there.clone()], |compute, store, state| {
        // A stop that crashed after recording only its release.
        (state.stage, state.stop_requested) = (Stage::Ready, false);
        let report = reconcile_with(
            compute,
            &|| Ok(compute.allowed.clone()),
            store,
            state,
            &Cancellation::default(),
        )
        .unwrap();
        assert!(report.worker.is_none(), "no stopped worker while the server exists");
    });
    assert_eq!(requests.len(), 1);
    assert!(
        state.stop_requested && state.stage == Stage::Stopping,
        "the stop intent is restored"
    );
    // Stopping again deletes the server the first stop released, after checking it is off.
    let deleted = vec![
        still_there.clone(),
        still_there,
        (200, json!({"action": {"id": 3, "status": "success"}}).to_string()),
        gone(),
    ];
    let (state, kept, requests) = act_on(&bound, &released, deleted, |compute, store, state| {
        stop_with(compute, store, state, &Cancellation::default()).unwrap();
    });
    assert!(
        requests
            .iter()
            .any(|request| request.starts_with("DELETE ") && request.contains("/servers/42 "))
    );
    assert_eq!((state.stage, state.operation), (Stage::Stopped, bound));
    assert!(kept, "the workspace volume stays");
}

#[test]
fn a_stop_records_the_release_before_it_shuts_the_server_down() {
    let running = (
        200,
        json!({"server": server("running", Some("192.0.2.10"))}).to_string(),
    );
    let refused = (
        503,
        json!({"error": {"code": "unavailable", "message": "unavailable"}}).to_string(),
    );
    let bound = CreateState::Bound { worker_id: "42".into() };
    let volume = journal(CreateState::Bound { worker_id: "9".into() });
    let responses = vec![running.clone(), running.clone(), running, refused];
    let (state, kept, _) = act_on(&bound, &volume, responses, |compute, store, state| {
        assert!(stop_with(compute, store, state, &Cancellation::default()).is_err());
        assert_eq!(Journal::load(store.root()).unwrap().released.as_deref(), Some("42"));
    });
    // A failed shutdown leaves an unfinished stop, which stopping again completes.
    assert_eq!((state.stage, state.operation), (Stage::Stopping, bound));
    assert!(kept);
}

#[test]
fn a_stop_interrupted_after_its_delete_is_finished_by_check_without_a_recorded_worker() {
    let bound = CreateState::Bound { worker_id: "42".into() };
    let mut released = journal(CreateState::Bound { worker_id: "9".into() });
    released.released = Some("42".into());
    let (state, kept, _) = act_on(&bound, &released, vec![gone()], |compute, store, state| {
        assert!(state.worker.is_none());
        // Ownership alone settles a released server: the placement is never consulted.
        let moved = || {
            Err(horizon_cloud::CloudError::Invalid(
                "The cloud's location is no longer allowed",
            ))
        };
        reconcile_with(compute, &moved, store, state, &Cancellation::default()).unwrap();
    });
    assert!(
        state.stop_requested && state.stage == Stage::Stopped,
        "resume is now possible"
    );
    assert!(kept, "the workspace volume stays");
}
