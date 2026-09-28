//! The Hetzner half of an image rebuild: the server is released as a stop releases
//! it, but the deployment keeps the rebuild's `Replace` stage.
use super::{
    deletion_points::{act_on, gone, journal},
    server,
};
use crate::cloud_runtime::{
    Stage,
    deployment::hetzner::{
        Journal, JournalFile as _,
        rebuild::{release_with, released, reopen},
    },
};
use horizon_cloud::{Cancellation, CreateState};
use serde_json::json;

#[test]
fn a_rebuild_releases_the_server_without_recording_a_stop() {
    let off = (
        200,
        json!({"server": serde_json::to_value(server("off", None)).unwrap()}).to_string(),
    );
    let bound = CreateState::Bound { worker_id: "42".into() };
    let mut recorded = journal(CreateState::Bound { worker_id: "9".into() });
    // A retry after an interruption that recorded only the release.
    recorded.released = Some("42".into());
    let responses = vec![
        off.clone(),
        off,
        (200, json!({"action": {"id": 3, "status": "success"}}).to_string()),
        gone(),
    ];
    let (state, kept, requests) = act_on(&bound, &recorded, responses, |compute, store, state| {
        (state.stage, state.stop_requested) = (Stage::Replace, false);
        assert!(released(store, state).unwrap(), "the release was recorded");
        release_with(compute, store, state, &Cancellation::default()).unwrap();
        assert_eq!(Journal::load(store.root()).unwrap().released.as_deref(), Some("42"));
    });
    assert!(
        requests
            .iter()
            .any(|request| request.starts_with("DELETE ") && request.contains("/servers/42 "))
    );
    assert_eq!(
        (state.stage, state.stop_requested, state.operation),
        (Stage::Replace, false, bound),
        "the rebuild's stage is kept and no stop is requested"
    );
    assert!(kept, "the workspace volume stays");
}

#[test]
fn only_the_released_server_is_reopened_and_its_release_is_cleared() {
    let bound = CreateState::Bound { worker_id: "42".into() };
    let untouched = journal(CreateState::Bound { worker_id: "9".into() });
    act_on(&bound, &untouched, Vec::new(), |_, store, state| {
        assert!(
            !released(store, state).unwrap(),
            "no release recorded: the server is untouched"
        );
        assert!(
            reopen(store, state).is_err(),
            "a server that was never released keeps its fence"
        );
    });
    let mut recorded = journal(CreateState::Bound { worker_id: "9".into() });
    recorded.released = Some("42".into());
    recorded.unused = true;
    let (state, kept, _) = act_on(&bound, &recorded, Vec::new(), |_, store, state| {
        (state.stage, state.stop_requested) = (Stage::Readiness, true);
        reopen(store, state).unwrap();
        let journal = Journal::load(store.root()).unwrap();
        assert!(journal.released.is_none());
        assert!(
            !journal.unused,
            "a volume a server held is never deleted to move the cloud"
        );
    });
    assert_eq!(state.operation, CreateState::Prepared);
    assert!(state.worker.is_none() && !state.stop_requested);
    assert!(kept);
}
