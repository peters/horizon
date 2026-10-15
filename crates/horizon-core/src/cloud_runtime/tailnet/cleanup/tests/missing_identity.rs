use super::*;
use crate::cloud_runtime::{state::ReadyHistory, tailnet::DeviceName, timeline::Timeline};

fn prepared() -> (tempfile::TempDir, state::Store, state::Deployment) {
    let (directory, store, mut state) = fixture();
    state.operation = CreateState::Prepared;
    state.tailnet_enrollment = EnrollmentLedger::default();
    state.version = state.record_version();
    store.save(&state).unwrap();
    super::super::super::Selection {
        tailnet: Some("work".into()),
    }
    .commit(store.root())
    .unwrap();
    (directory, store, state)
}

fn assert_unknown(store: &state::Store, state: &mut state::Deployment) {
    store.save(state).unwrap();
    let before = std::fs::read(store.root().join("deployment.json")).unwrap();
    assert!(
        require_complete(store, state)
            .unwrap_err()
            .to_string()
            .contains("not recorded")
    );
    assert!(
        run(store, state, &|_| {}, |_, _| panic!(
            "missing identity must never reach Node API"
        ))
        .unwrap_err()
        .to_string()
        .contains("not recorded")
    );
    assert_eq!(std::fs::read(store.root().join("deployment.json")).unwrap(), before);
    assert!(state.tailnet_enrollment.is_empty());
}

#[test]
fn prepared_does_not_erase_prior_readiness_enrollment_or_resume_history() {
    for history in 0..7 {
        let (_directory, store, mut state) = prepared();
        assert_eq!(
            state.ready_history,
            ReadyHistory::Unobserved,
            "legacy default is not proof of freshness"
        );
        match history {
            0 => state.ready_history = ReadyHistory::Observed,
            1 => state.ready_after_seconds = Some(0),
            2 => state.source_ready = true,
            3 => {
                state.tailnet_device = Some(DeviceName {
                    name: "synthetic-worker".into(),
                    observed: false,
                });
            }
            4 => {
                state.timeline = Some(Timeline {
                    reconnected: true,
                    ..Timeline::default()
                });
            }
            5 => {
                state.timeline = Some(Timeline {
                    resume_requested: Some(0),
                    ..Timeline::default()
                });
            }
            _ => state.stop_requested = true,
        }
        assert_unknown(&store, &mut state);
    }
}

#[test]
fn a_never_provisioned_selected_prepared_cloud_needs_no_node_cleanup() {
    let (_directory, store, mut state) = prepared();
    state.timeline = Some(Timeline::default());
    store.save(&state).unwrap();
    let before = std::fs::read(store.root().join("deployment.json")).unwrap();
    require_complete(&store, &state).unwrap();
    run(&store, &mut state, &|_| {}, |_, _| panic!("no node was enrolled")).unwrap();
    assert_eq!(std::fs::read(store.root().join("deployment.json")).unwrap(), before);
}

#[test]
fn prior_ready_or_resume_history_without_a_selected_tailnet_stays_quiet() {
    let (_directory, store, mut state) = prepared();
    state.ready_history = ReadyHistory::Observed;
    state.source_ready = true;
    state.timeline = Some(Timeline::resume_requested(std::time::SystemTime::UNIX_EPOCH));
    super::super::super::Selection::default().commit(store.root()).unwrap();
    store.save(&state).unwrap();
    let before = std::fs::read(store.root().join("deployment.json")).unwrap();
    require_complete(&store, &state).unwrap();
    run(&store, &mut state, &|_| {}, |_, _| {
        panic!("None supplies no enrolled node")
    })
    .unwrap();
    assert_eq!(std::fs::read(store.root().join("deployment.json")).unwrap(), before);
}

#[test]
fn deleted_volume_journals_alone_do_not_claim_a_prepared_cloud_enrolled_a_node() {
    let (_directory, store, mut state) = prepared();
    state.profile.provider = provider_id(Kind::Hetzner);
    state.spec.as_mut().unwrap().profile = state.profile.clone();
    store.save(&state).unwrap();
    let journal = horizon_cloud::hetzner::cloud::Journal {
        location: Some("hel1".into()),
        deleting: true,
        volume: CreateState::Terminated { worker_id: "9".into() },
        ..Default::default()
    };
    std::fs::write(store.root().join("hetzner.json"), serde_json::to_vec(&journal).unwrap()).unwrap();
    assert!(crate::cloud_runtime::providers::deletion_confirmed(&store, &state).unwrap());
    require_complete(&store, &state).unwrap();
    run(&store, &mut state, &|_| {}, |_, _| {
        panic!("a volume is not an enrolled worker")
    })
    .unwrap();
}
