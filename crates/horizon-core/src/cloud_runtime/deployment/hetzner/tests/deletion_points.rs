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

pub(super) fn gone() -> (u16, String) {
    (
        404,
        json!({"error": {"code": "not_found", "message": "not found"}}).to_string(),
    )
}

/// Runs `act` on a cloud whose record is `operation` and `journal`, stopping,
/// against `responses`; returns the saved deployment, whether anything is
/// retained and the requests served.
pub(super) fn act_on(
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

pub(super) fn journal(volume: CreateState) -> Journal {
    Journal {
        location: Some("hel1".into()),
        volume,
        key: Some("ssh-ed25519 AAAA".into()),
        released: None,
        deleting: false,
        unused: false,
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

#[test]
fn stop_boundary_refusal_prevents_mutation_and_settlement_follows_saved_stop() {
    use crate::cloud_runtime::deployment::hetzner::lifecycle::stop_with_observer;
    use crate::cloud_runtime::{Error, mutation::State as Mutation};
    let bound = CreateState::Bound { worker_id: "42".into() };
    let mut released = journal(CreateState::Bound { worker_id: "9".into() });
    released.released = Some("42".into());
    let off = (200, json!({"server": server("off", None)}).to_string());
    // The read-only checks run; the refused boundary keeps the delete from being sent.
    let (state, _, requests) = act_on(
        &bound,
        &released,
        vec![off.clone(), off.clone()],
        |compute, store, state| {
            let observe = |phase| {
                assert_eq!(phase, Mutation::Pending);
                Err(Error::Invalid("Synthetic receipt failure"))
            };
            assert!(stop_with_observer(compute, store, state, &Cancellation::default(), &observe).is_err());
        },
    );
    assert_eq!(state.stage, Stage::Stopping);
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.starts_with("GET ")));
    let responses = vec![
        off.clone(),
        off,
        (200, json!({"action":{"id":3,"status":"success"}}).to_string()),
        gone(),
    ];
    let seen = std::cell::RefCell::new(Vec::new());
    let (state, _, _) = act_on(&bound, &released, responses, |compute, store, state| {
        let observe = |phase| {
            if phase == Mutation::Settled {
                assert_eq!(store.load()?.unwrap().stage, Stage::Stopped);
            }
            let previous = seen.borrow().last().copied().unwrap_or(Mutation::Settled);
            seen.borrow_mut().push(phase);
            Ok(previous)
        };
        stop_with_observer(compute, store, state, &Cancellation::default(), &observe).unwrap();
    });
    assert_eq!(state.stage, Stage::Stopped);
    assert_eq!(*seen.borrow(), [Mutation::Pending, Mutation::Settled]);
}

#[test]
fn stop_save_failure_preserves_evidence_and_never_reaches_the_provider() {
    use crate::cloud_runtime::deployment::hetzner::lifecycle::stop_with_observer;
    for retry in [false, true] {
        let bound = CreateState::Bound { worker_id: "42".into() };
        let mut released = journal(CreateState::Bound { worker_id: "9".into() });
        released.released = Some("42".into());
        let seen = std::cell::RefCell::new(Vec::new());
        let (_, _, requests) = act_on(&bound, &released, vec![], |compute, store, state| {
            if !retry {
                state.stage = Stage::Ready;
                state.stop_requested = false;
            }
            let file = store.root().join("deployment.json");
            let original = std::fs::read(&file).unwrap();
            std::fs::remove_file(&file).unwrap();
            std::fs::create_dir(&file).unwrap();
            let observe = |phase| {
                let previous = seen
                    .borrow()
                    .last()
                    .copied()
                    .unwrap_or(crate::cloud_runtime::mutation::State::Settled);
                seen.borrow_mut().push(phase);
                Ok(previous)
            };
            assert!(stop_with_observer(compute, store, state, &Cancellation::default(), &observe).is_err());
            std::fs::remove_dir(&file).unwrap();
            std::fs::write(&file, original).unwrap();
        });
        assert!(seen.borrow().is_empty());
        assert!(requests.is_empty());
    }
}

#[test]
fn provisioning_observer_can_refuse_empty_volume_deletion_after_ownership_checks() {
    use crate::cloud_runtime::{Error, mutation::State as Mutation};
    let mut free = super::volume();
    free.server = None;
    let mut unused = journal(CreateState::Bound { worker_id: "9".into() });
    unused.unused = true;
    let (_, _, requests) = act_on(
        &CreateState::Prepared,
        &unused,
        vec![(200, json!({"volume": free}).to_string())],
        |compute, store, state| {
            let seen = std::cell::Cell::new(0);
            let error = crate::cloud_runtime::deployment::hetzner::provision(
                compute,
                store,
                state,
                &spec(),
                &Cancellation::default(),
                &|_| {},
                &|phase| {
                    assert_eq!(phase, Mutation::Pending);
                    seen.set(seen.get() + 1);
                    Err(Error::Invalid("observer refused"))
                },
            )
            .unwrap_err();
            assert!(matches!(error, Error::Invalid("observer refused")));
            assert_eq!(seen.get(), 1);
            assert!(Journal::load(store.root()).unwrap().unused);
        },
    );
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /volumes/9"));
}

#[test]
fn a_stop_records_a_pending_mutation_only_before_its_requests() {
    use crate::cloud_runtime::deployment::hetzner::lifecycle::stop_with_observer;
    use crate::cloud_runtime::mutation::State as Mutation;
    let bound = CreateState::Bound { worker_id: "42".into() };
    let volume = journal(CreateState::Bound { worker_id: "9".into() });
    let running = (
        200,
        json!({"server": server("running", Some("192.0.2.10"))}).to_string(),
    );
    let unavailable = (
        503,
        json!({"error": {"code": "unavailable", "message": "unavailable"}}).to_string(),
    );
    let refused = (
        422,
        json!({"error": {"code": "invalid_input", "message": "refused"}}).to_string(),
    );
    // (responses, the evidence the observer sees)
    let cases = [
        // The status read fails before any request: nothing is pending.
        (vec![running.clone(), unavailable], vec![]),
        // The only request sent is refused outright: it settles again.
        (
            vec![running.clone(), running.clone(), running.clone(), refused],
            vec![Mutation::Pending, Mutation::Settled],
        ),
        // The shutdown is accepted, then waiting for its action fails: it may have
        // applied, so it stays pending.
        (
            vec![
                running.clone(),
                running.clone(),
                running,
                (200, json!({"action": {"id": 5, "status": "running"}}).to_string()),
                (
                    401,
                    json!({"error": {"code": "unauthorized", "message": "unauthorized"}}).to_string(),
                ),
            ],
            vec![Mutation::Pending],
        ),
    ];
    for (responses, expected) in cases {
        let seen = std::cell::RefCell::new(Vec::new());
        let (state, _, _) = act_on(&bound, &volume, responses, |compute, store, state| {
            (state.stage, state.stop_requested) = (Stage::Ready, false);
            let observe = |phase| {
                let previous = seen.borrow().last().copied().unwrap_or(Mutation::Settled);
                seen.borrow_mut().push(phase);
                Ok(previous)
            };
            assert!(stop_with_observer(compute, store, state, &Cancellation::default(), &observe).is_err());
        });
        assert_eq!(*seen.borrow(), expected);
        assert_eq!(state.stage, Stage::Stopping, "the stop stays recorded, to be retried");
    }
}
