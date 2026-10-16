use super::*;
use crate::cloud_runtime::{local_network::endpoint::tests::prepared, state::service_admission::Identity};
use serde_json::json;

fn owner() -> OwnerScope {
    OwnerScope::new(
        "native-host",
        &uuid::Uuid::new_v4().to_string(),
        "workspace-a",
        "service-cloud",
    )
    .unwrap()
}
#[cfg(unix)]
fn settings() -> Settings {
    serde_json::from_value(json!({"runpod_key_file":"/synthetic/no-provider", "ssh_identity_file":"/synthetic/key", "docker_config":"/synthetic/no-docker", "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]})).unwrap()
}
#[cfg(unix)]
fn ready(store: &Store, settings: &Settings) -> (Recipient, crate::cloud_panel::CloudLaunch) {
    let state: Deployment = serde_json::from_value(json!({
        "version":1,"cloud_id":"service-cloud","repository":"/synthetic","revision":"a".repeat(40),
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
        "stage":"Ready","operation":{"state":"bound","worker_id":"worker-a"},"spec":{"operation_id":"op-a","image_digest":"sha256:synthetic","profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},"public_key":"synthetic","registry_auth_id":null,"gpu_types":[],"cpu_flavors":[],"data_centers":[]},
        "worker":{"id":"worker-a","name":"worker-a","imageName":"registry.example/worker","desiredStatus":"RUNNING","publicIp":"127.0.0.1","portMappings":{"22":22022}},
        "sessions":[],"source_ready":true
    })).unwrap();
    store.save(&state).unwrap();
    let launch = crate::cloud_panel::CloudLaunch {
        publishing_selection: None,
        deployment_started: true,
        id: state.cloud_id.clone(),
        revision: state.revision.clone(),
        profile_name: "synthetic".into(),
        profile: state.profile.clone(),
        placement: crate::cloud_panel::Placement::default(),
    };
    (Recipient::checked(store, settings, &launch).unwrap(), launch)
}

#[cfg(unix)]
#[test]
fn intent_is_durable_and_original_lifecycle_lock_held_before_any_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let settings = settings();
    let (recipient, _) = ready(&store, &settings);
    let requested = prepared("100.64.1.2:8123".parse().unwrap());
    let expected = requested.retained(owner(), "worker-a".into()).unwrap();
    let result = Admitted::start_with(
        store,
        &settings,
        expected.owner().clone(),
        recipient,
        requested,
        |_, _| {
            assert!(Store::lock_existing(root.path()).is_err());
            let state: Deployment =
                serde_json::from_slice(&std::fs::read(root.path().join("deployment.json")).unwrap()).unwrap();
            assert_eq!(state.service_admission, Some(expected.clone()));
            assert_eq!(expected.phase(), Phase::Armed {});
            Err(Error::Invalid("synthetic startup uncertainty"))
        },
    );
    assert!(result.is_err());
    let store = Store::lock_existing(root.path()).unwrap();
    assert_eq!(store.load().unwrap().unwrap().service_admission, Some(expected));
}

#[cfg(unix)]
#[test]
fn unresolved_attempt_is_never_recreated_from_metadata_or_replaced_by_another_owner() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let settings = settings();
    let (recipient, _) = ready(&store, &settings);
    let scope = owner();
    let prior = prepared("100.64.1.2:8123".parse().unwrap())
        .retained(scope.clone(), "worker-a".into())
        .unwrap();
    let mut state = store.load().unwrap().unwrap();
    state.service_admission = Some(prior.clone());
    store.save(&state).unwrap();
    assert!(
        Admitted::start_with(
            store,
            &settings,
            scope,
            recipient,
            prepared("100.64.1.2:8123".parse().unwrap()),
            |_, _| panic!("uncertain operation dispatched")
        )
        .is_err()
    );
    assert_eq!(
        Store::lock_existing(root.path())
            .unwrap()
            .load()
            .unwrap()
            .unwrap()
            .service_admission,
        Some(prior)
    );
}

#[cfg(unix)]
#[test]
fn changed_worker_route_image_and_identity_file_refuse_before_intent_or_dispatch() {
    for change in ["worker", "route", "image", "identity"] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut settings = settings();
        let (recipient, _) = ready(&store, &settings);
        let mut state = store.load().unwrap().unwrap();
        let worker = state.worker.as_mut().unwrap();
        match change {
            "worker" => worker.id = "successor".into(),
            "route" => worker
                .port_mappings
                .as_mut()
                .unwrap()
                .insert("22".into(), 22023)
                .map(|_| ())
                .unwrap(),
            "image" => worker.image_name = "registry.example/successor".into(),
            "identity" => settings.ssh_identity_file = "/synthetic/another-key".into(),
            _ => unreachable!(),
        }
        store.save(&state).unwrap();
        assert!(
            Admitted::start_with(
                store,
                &settings,
                owner(),
                recipient,
                prepared("100.64.1.2:8123".parse().unwrap()),
                |_, _| panic!("changed recipient dispatched")
            )
            .is_err()
        );
        assert!(
            Store::lock_existing(root.path())
                .unwrap()
                .load()
                .unwrap()
                .unwrap()
                .service_admission
                .is_none()
        );
    }
}

#[test]
fn exact_generation_transitions_refuse_stale_receipts_without_modifying_evidence() {
    let mut record = prepared("100.64.1.2:8123".parse().unwrap())
        .retained(owner(), "worker-a".into())
        .unwrap();
    let before = record.clone();
    let foreign: Identity =
        serde_json::from_value(json!({"operation_id":uuid::Uuid::new_v4(),"generation":uuid::Uuid::new_v4()})).unwrap();
    assert!(record.set_closed(foreign).is_err());
    assert_eq!(record, before);
    assert!(
        record
            .set_attached(uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), 1234)
            .is_err()
    );
    assert_eq!(record, before);
    let attempt = prepared("100.64.1.2:8123".parse().unwrap());
    let mut record = attempt.retained(owner(), "worker-a".into()).unwrap();
    record
        .set_attached(attempt.attempt().operation_id, attempt.attempt().generation, 1234)
        .unwrap();
    record.set_closed(record.attempt()).unwrap();
    record.set_closed(record.attempt()).unwrap();
    assert_eq!(record.phase(), Phase::Closed {});
    assert!(
        record
            .set_attached(attempt.attempt().operation_id, attempt.attempt().generation, 1234)
            .is_err()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn actual_legacy_ready_then_exact_owner_close_reaps_before_closed_and_releases_original_lock() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let settings = settings();
    let (recipient, _) = ready(&store, &settings);
    let owner = owner();
    let mut admitted = Admitted::start_with(
        store,
        &settings,
        owner.clone(),
        recipient,
        prepared("100.64.1.2:8123".parse().unwrap()),
        |prepared, _| Ok(super::super::attachment::tests::synthetic_attachment(prepared)),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !matches!(
        admitted.status().unwrap().transport.worker,
        crate::cloud_runtime::local_network::State::Active { .. }
    ) {
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(Store::lock_existing(root.path()).is_err());
    let receipt = admitted.status().unwrap().transport.gateway.receipt;
    let mut stale = receipt.clone();
    stale.generation = uuid::Uuid::new_v4();
    assert!(admitted.close(&owner, &stale).is_err());
    let mut foreign = owner.clone();
    foreign.workspace_id = "another-workspace".into();
    assert!(admitted.close(&foreign, &receipt).is_err());
    assert!(!admitted.status().unwrap().stopping);
    let original_bytes = std::fs::read(root.path().join("deployment.json")).unwrap();
    assert!(matches!(
        admitted
            .store
            .snapshot(&owner)
            .unwrap()
            .service_admission
            .unwrap()
            .phase(),
        Phase::Attached { .. }
    ));
    for (scope, attempted) in [(&owner, &stale), (&foreign, &receipt)] {
        assert!(admitted.finish(scope, attempted).is_err());
        let status = admitted.status().unwrap();
        assert!(matches!(status.transport.gateway.state, super::super::State::Active));
        assert!(matches!(
            status.transport.worker,
            crate::cloud_runtime::local_network::State::Active { .. }
        ));
        assert!(!status.stopping);
        assert!(Store::lock_existing(root.path()).is_err());
        assert_eq!(
            std::fs::read(root.path().join("deployment.json")).unwrap(),
            original_bytes
        );
    }
    admitted.finish(&owner, &receipt).unwrap();
    assert!(admitted.status().is_none());
    assert_eq!(
        admitted
            .store
            .snapshot(&owner)
            .unwrap()
            .service_admission
            .unwrap()
            .phase(),
        Phase::Closed {}
    );
    drop(admitted);
    let store = Store::lock_existing(root.path()).unwrap();
    assert_eq!(
        store.load().unwrap().unwrap().service_admission.unwrap().phase(),
        Phase::Closed {}
    );
}

#[cfg(target_os = "linux")]
#[test]
fn worker_or_native_scope_drift_closes_live_source_without_following_successor_or_erasing_attempt() {
    for changed in ["worker", "owner", "launch"] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let settings = settings();
        let (recipient, mut launch) = ready(&store, &settings);
        let owner = owner();
        let admitted = Admitted::start_with(
            store,
            &settings,
            owner.clone(),
            recipient,
            prepared("100.64.1.2:8123".parse().unwrap()),
            |prepared, _| Ok(super::super::attachment::tests::synthetic_attachment(prepared)),
        )
        .unwrap();
        if changed == "worker" {
            // External record drift is not an operation on the live custody API.
            // Its original watcher must close without following the successor.
            let mut state = admitted.store.snapshot(&owner).unwrap();
            state.worker.as_mut().unwrap().id = "successor".into();
            std::fs::write(
                root.path().join("deployment.json"),
                serde_json::to_vec_pretty(&state).unwrap(),
            )
            .unwrap();
        } else if changed == "launch" {
            launch.publishing_selection = Some(crate::cloud_runtime::publishing::Selection::Recipe {
                image: "registry.example/another-intent".into(),
            });
            assert!(admitted.observe(&owner, &settings, &launch).is_err());
        } else {
            let mut next = owner.clone();
            next.session_id = uuid::Uuid::new_v4();
            assert!(admitted.observe(&next, &settings, &launch).is_err());
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !matches!(
            admitted.status().unwrap().transport.gateway.state,
            super::super::State::Revoked { .. }
        ) {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            admitted.status().unwrap().transport.gateway.state,
            super::super::State::Revoked { .. }
        ));
        drop(admitted);
        let state = Store::lock_existing(root.path()).unwrap().load().unwrap().unwrap();
        assert!(matches!(
            state.service_admission.unwrap().phase(),
            Phase::Attached { .. }
        ));
    }
}

#[cfg(unix)]
#[test]
fn full_original_launch_snapshot_is_retained_including_placement_and_publishing_intent() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let settings = settings();
    let (recipient, launch) = ready(&store, &settings);
    assert!(Recipient::checked(&store, &settings, &launch).is_ok_and(|current| current == recipient));
    for change in ["placement", "profile_name", "publishing_intent", "started"] {
        let mut changed = launch.clone();
        match change {
            "placement" => changed.placement.region = Some("another-region".into()),
            "profile_name" => changed.profile_name = "another-original-profile".into(),
            "publishing_intent" => {
                changed.publishing_selection = Some(crate::cloud_runtime::publishing::Selection::Recipe {
                    image: "registry.example/another-intent".into(),
                });
            }
            "started" => changed.deployment_started = false,
            _ => unreachable!(),
        }
        assert!(Recipient::checked(&store, &settings, &changed).is_ok_and(|current| current != recipient));
    }
}

#[cfg(unix)]
#[test]
fn a_closed_original_attempt_allows_a_fresh_explicit_owner_but_retains_new_uncertain_intent() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let settings = settings();
    let (recipient, _) = ready(&store, &settings);
    let former = owner();
    let prior = prepared("100.64.1.2:8123".parse().unwrap());
    let mut record = prior.retained(former.clone(), "worker-a".into()).unwrap();
    record.set_closed(record.attempt()).unwrap();
    let mut state = store.load().unwrap().unwrap();
    state.service_admission = Some(record);
    store.save(&state).unwrap();
    let next = owner();
    assert_ne!(former, next);
    let called = std::cell::Cell::new(false);
    let result = Admitted::start_with(
        store,
        &settings,
        next.clone(),
        recipient,
        prepared("100.64.1.2:8123".parse().unwrap()),
        |_, _| {
            called.set(true);
            Err(Error::Invalid("bounded injected startup refusal"))
        },
    );
    assert!(result.is_err());
    assert!(called.get());
    let state = Store::lock_existing(root.path()).unwrap().load().unwrap().unwrap();
    let retained = state.service_admission.unwrap();
    assert_eq!(retained.owner(), &next);
    assert_eq!(retained.phase(), Phase::Armed {});
}

#[cfg(unix)]
#[test]
fn an_already_cancelled_native_job_never_saves_intent_or_dispatches() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let settings = settings();
    let (recipient, _) = ready(&store, &settings);
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let dispatched = std::cell::Cell::new(false);
    let result = Admitted::start_cancelled_with(
        store,
        &settings,
        owner(),
        recipient,
        prepared("100.64.1.2:8123".parse().unwrap()),
        cancellation,
        |_, _| {
            dispatched.set(true);
            Err(Error::Invalid("unexpected cancelled dispatch"))
        },
    );
    assert!(result.is_err());
    assert!(!dispatched.get());
    assert!(
        Store::lock_existing(root.path())
            .unwrap()
            .load()
            .unwrap()
            .unwrap()
            .service_admission
            .is_none()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn shared_native_cancellation_closes_source_without_waiting_for_the_owner_loop() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let settings = settings();
    let (recipient, _) = ready(&store, &settings);
    let scope = owner();
    let cancellation = Cancellation::default();
    let admitted = Admitted::start_cancelled_with(
        store,
        &settings,
        scope.clone(),
        recipient,
        prepared("100.64.1.2:8123".parse().unwrap()),
        cancellation.clone(),
        |prepared, _| Ok(super::super::attachment::tests::synthetic_attachment(prepared)),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !matches!(
        admitted.status().unwrap().transport.worker,
        crate::cloud_runtime::local_network::State::Active { .. }
    ) {
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    cancellation.cancel();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !matches!(
        admitted.status().unwrap().transport.gateway.state,
        super::super::State::Revoked { .. }
    ) {
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(Store::lock_existing(root.path()).is_err());
    drop(admitted);
    let state = Store::lock_existing(root.path()).unwrap().load().unwrap().unwrap();
    assert!(matches!(
        state.service_admission.unwrap().phase(),
        Phase::Attached { .. }
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn cancelled_source_closes_during_delayed_record_read_without_releasing_original_custody() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let (recipient, _) = ready(&store, &settings());
    let requested = prepared("100.64.1.2:8123".parse().unwrap());
    let mut record = requested.retained(owner(), "worker-a".into()).unwrap();
    let mut state = store.load().unwrap().unwrap();
    state.service_admission = Some(record.clone());
    store.save(&state).unwrap();
    let attachment = Arc::new(super::super::attachment::tests::synthetic_attachment(requested));
    let receipt = attachment.status().gateway.receipt;
    record
        .set_attached(receipt.operation_id, receipt.generation, receipt.proxy_port)
        .unwrap();
    state.service_admission = Some(record.clone());
    store.save(&state).unwrap();
    let scope = record.owner().clone();
    let store = OriginalCloud::capture(store, &settings(), scope.clone(), recipient.launch()).unwrap();
    let cancellation = Cancellation::default();
    let mut admitted = Admitted {
        attachment: Some(Arc::clone(&attachment)),
        watcher: None,
        cancellation_watcher: Some(
            watch_cancellation(Arc::clone(&attachment), receipt.clone(), cancellation.clone()).unwrap(),
        ),
        stop: cancellation.clone(),
        store: store.clone(),
        record: record.clone(),
        receipt,
        recipient: recipient.clone(),
    };
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    admitted.watcher = Some(
        watch_record(recipient, record.clone(), cancellation.clone(), move || {
            entered_tx.send(()).unwrap();
            release_rx
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| Error::Invalid("bounded deployment read delay expired"))?;
            store.snapshot(&scope).map(Some).map_err(Into::into)
        })
        .unwrap(),
    );
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    cancellation.cancel();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !matches!(attachment.status().gateway.state, super::super::State::Revoked { .. }) {
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(Store::lock_existing(root.path()).is_err());
    drop(attachment);
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let dropping = thread::spawn(move || {
        drop(admitted);
        finished_tx.send(()).unwrap();
    });
    assert!(matches!(
        finished_rx.recv_timeout(Duration::from_millis(100)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(Store::lock_existing(root.path()).is_err());
    release_tx.send(()).unwrap();
    finished_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    dropping.join().unwrap();
    let state = Store::lock_existing(root.path()).unwrap().load().unwrap().unwrap();
    assert_eq!(state.service_admission, Some(record));
    assert!(matches!(
        state.service_admission.unwrap().phase(),
        Phase::Attached { .. }
    ));
}

#[cfg(all(target_os = "linux", feature = "cloud-exposure"))]
impl Admitted {
    pub(in crate::cloud_runtime) fn synthetic_shared(
        store: OriginalCloud,
        settings: &Settings,
        owner: OwnerScope,
        launch: &crate::cloud_panel::CloudLaunch,
        cancellation: Cancellation,
    ) -> Self {
        let recipient = store.recipient(&owner, settings, launch).unwrap();
        Self::start_shared_cancelled_with(
            store,
            settings,
            owner,
            recipient,
            prepared("100.64.1.2:8123".parse().unwrap()),
            cancellation,
            |prepared, _| Ok(super::super::attachment::tests::synthetic_attachment(prepared)),
        )
        .unwrap()
    }
}
