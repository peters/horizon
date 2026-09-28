use super::*;
use horizon_cloud::{
    WorkerSpec,
    runpod::volumes::{Spec, Tier},
};
use serde_json::json;

fn fixture() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let worker: WorkerSpec = serde_json::from_value(json!({
        "operation_id":"owned-operation", "image_digest":format!("example/worker@sha256:{}", "a".repeat(64)),
        "profile":{"provider":"runpod","image":"example/worker","cpu":4,"memory_gb":8},
        "public_key":"fixture-key","registry_auth_id":null,"gpu_types":[],"cpu_flavors":["cpu3c"],"data_centers":[]
    }))
    .unwrap();
    let spec = Spec {
        operation_id: worker.operation_id.clone(),
        size: 20,
        data_center_id: "EU-TEST-1".into(),
        tier: Tier::Standard,
    };
    let volume = Volume {
        id: "volume1".into(),
        name: spec.name(),
        size: 20,
        data_center_id: spec.data_center_id.clone(),
        tier: Some(Tier::Standard),
    };
    let state: Deployment = serde_json::from_value(json!({
        "version":1,"cloud_id":worker.operation_id,"repository":"/synthetic","revision":"a",
        "profile":worker.profile,"stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
        "spec":worker,"worker":{"id":"worker1","name":"horizon-owned-operation","imageName":worker.image_digest,
            "vcpuCount":4,"memoryInGb":8,"containerDiskInGb":20,"volumeInGb":0,"volumeMountPath":"/workspace",
            "desiredStatus":"RUNNING","publicIp":"192.0.2.10","portMappings":{"22":2200},
            "networkVolume":{"id":"volume1","size":20,"dataCenterId":"EU-TEST-1"}},
        "source_ready":true,"sessions":[{"panel_id":"panel","agent":"shell","tmux":"session","branch":"main","worktree":"/workspace/repo"}]
    })).unwrap();
    store.save(&state).unwrap();
    save(
        &store,
        &Record {
            version: 1,
            worker,
            spec,
            state: State::Bound { volume, creation: None },
        },
    )
    .unwrap();
    (root, store)
}

fn confirmed(store: &Store) -> Intent {
    let mut intent = prepare(store, 40).unwrap();
    let mut value = serde_json::to_value(&intent.growth).unwrap();
    value["phase"] = json!("confirmed");
    intent.growth = serde_json::from_value(value).unwrap();
    let State::Bound { volume, .. } = &intent.storage.state else {
        panic!("bound")
    };
    let mut volume = volume.clone();
    volume.size = 40;
    intent.observed = Some(volume);
    write(store, &intent).unwrap();
    intent
}

#[test]
fn growth_intent_blocks_other_operations_and_different_targets() {
    let (_root, store) = fixture();
    let intent = prepare(&store, 40).unwrap();
    assert!(store.load().is_err());
    intent.verify(&store).unwrap();
    assert!(intent.next().is_err());
    assert!(read(&store).unwrap().is_some());
    assert!(prepare(&store, 60).is_err());
}

#[test]
fn every_local_commit_boundary_replays_without_losing_sessions_or_authority() {
    for fail_after_storage in [false, true] {
        let (root, store) = fixture();
        let intent = confirmed(&store);
        let old_sessions = serde_json::to_value(&intent.original().unwrap().sessions).unwrap();
        assert!(
            commit(&store, &intent, &mut |boundary| {
                if matches!(boundary, Boundary::Storage) == fail_after_storage {
                    return Err(Error::Invalid("synthetic crash"));
                }
                Ok(())
            })
            .is_err()
        );
        assert!(store.load().is_err());
        drop(store);
        let store = Store::lock(root.path()).unwrap();
        let intent = read(&store).unwrap().unwrap();
        let next = commit(&store, &intent, &mut |_| Ok(())).unwrap();
        assert_eq!(next.profile.storage.volume_gb, 40);
        assert_eq!(serde_json::to_value(&next.sessions).unwrap(), old_sessions);
        assert_eq!(
            next.worker.as_ref().unwrap().network_volume.as_ref().unwrap().size,
            Some(40)
        );
        let volume = load(&store, next.spec.as_ref().unwrap()).unwrap().unwrap();
        assert_eq!(volume.spec.size, 40);
        assert!(matches!(volume.state, State::Bound { creation: None, .. }));
        assert!(store.load().unwrap().is_some());
        assert!(read(&store).unwrap().is_none());
    }
}

#[test]
fn changed_records_and_uncertain_provider_results_never_commit() {
    let (_root, store) = fixture();
    let mut intent = confirmed(&store);
    let original = intent.observed.clone();
    intent.observed.as_mut().unwrap().id = "foreign".into();
    assert!(commit(&store, &intent, &mut |_| Ok(())).is_err());
    intent.observed = original;
    let mut changed = store.load_during_storage_growth().unwrap().unwrap();
    changed.revision = "different".into();
    store.save(&changed).unwrap();
    assert!(commit(&store, &intent, &mut |_| Ok(())).is_err());
    assert!(read(&store).unwrap().is_some());
}

#[test]
fn invalid_attachment_and_profile_records_never_create_an_intent() {
    for field in ["id", "size", "location", "missing", "profile", "cloud_id"] {
        let (_root, store) = fixture();
        let mut state = store.load().unwrap().unwrap();
        let worker = state.worker.as_mut().unwrap();
        match field {
            "id" => worker.network_volume.as_mut().unwrap().id = Some("foreign".into()),
            "size" => worker.network_volume.as_mut().unwrap().size = Some(30),
            "location" => worker.network_volume.as_mut().unwrap().data_center_id = Some("elsewhere".into()),
            "missing" => worker.network_volume = None,
            "profile" => state.profile.cpu += 1,
            "cloud_id" => state.cloud_id = "different".into(),
            _ => unreachable!(),
        }
        store.save(&state).unwrap();
        assert!(prepare(&store, 40).is_err(), "{field}");
        assert!(read(&store).unwrap().is_none(), "{field}");
        assert!(store.load().is_ok());
    }
}

#[test]
fn shrinking_and_unready_clouds_cannot_begin() {
    let (_root, store) = fixture();
    assert!(prepare(&store, 20).is_err());
    assert!(prepare(&store, 10).is_err());
    assert!(read(&store).unwrap().is_none());
    let mut state = store.load().unwrap().unwrap();
    state.stop_requested = true;
    store.save(&state).unwrap();
    assert!(prepare(&store, 40).is_err());
}

#[test]
fn reconciled_mounts_may_omit_capacity_but_cannot_report_a_different_one() {
    for capacity in [None, Some(21)] {
        let (_root, store) = fixture();
        let mut state = store.load().unwrap().unwrap();
        state.worker.as_mut().unwrap().network_volume.as_mut().unwrap().size = capacity;
        store.save(&state).unwrap();
        if capacity.is_none() {
            let intent = confirmed(&store);
            let next = commit(&store, &intent, &mut |_| Ok(())).unwrap();
            assert_eq!(next.worker.unwrap().network_volume.unwrap().size, Some(40));
        } else {
            assert!(prepare(&store, 40).is_err());
            assert!(read(&store).unwrap().is_none());
        }
    }
}

#[test]
fn confirmed_local_recovery_does_not_need_provider_credentials() {
    let (root, store) = fixture();
    confirmed(&store);
    drop(store);
    let settings: Settings = serde_json::from_value(json!({
        "runpod_key_file":root.path().join("missing-key"),
        "ssh_identity_file":"unused", "docker_config":"unused", "registry_pull_auth_id":null,
        "cpu_flavors":[], "gpu_types":[]
    }))
    .unwrap();
    let state = grow_storage(root.path(), &settings, 40, &Cancellation::default()).unwrap();
    assert_eq!(state.profile.storage.volume_gb, 40);
    assert!(!root.path().join(JOURNAL).exists());
}

#[cfg(unix)]
#[test]
fn migration_cannot_capture_an_incomplete_growth() {
    let (root, store) = fixture();
    prepare(&store, 40).unwrap();
    let before = fs::read(root.path().join("deployment.json")).unwrap();
    drop(store);
    let error = crate::cloud_runtime::state::migration::MigratedStore::migrate(
        root.path(),
        "synthetic-session",
        "synthetic-workspace",
        crate::cloud_runtime::allocation::ControllerId::generate(),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("disk growth is pending"));
    assert!(!root.path().join("migration.json").exists());
    assert_eq!(fs::read(root.path().join("deployment.json")).unwrap(), before);
}
