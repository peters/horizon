use super::*;
use horizon_cloud::{
    WorkerSpec,
    runpod::volumes::{Spec, Tier, Volume},
};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

fn fixture() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    assert!(
        std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(root.path().join("identity"))
            .status()
            .unwrap()
            .success()
    );
    let public_key = fs::read_to_string(root.path().join("identity.pub")).unwrap();
    let worker: WorkerSpec = serde_json::from_value(json!({
        "operation_id":"owned-operation", "image_digest":format!("example/worker@sha256:{}", "a".repeat(64)),
        "profile":{"provider":"runpod","image":"example/worker","cpu":4,"memory_gb":8},
        "public_key":public_key.trim(),"registry_auth_id":null,"gpu_types":[],"cpu_flavors":["cpu3c"],"data_centers":[]
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
        "spec":worker,"worker":{"id":"worker1","name":worker.name(),"imageName":worker.image_digest,
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

fn settings() -> Settings {
    serde_json::from_value(json!({"runpod_key_file":"missing-key","ssh_identity_file":"unused",
        "docker_config":"unused","registry_pull_auth_id":null,"cpu_flavors":["cpu3c"],"gpu_types":[]}))
    .unwrap()
}

fn identity_settings(store: &Store) -> Settings {
    let mut settings = settings();
    settings.ssh_identity_file = store.root().join("identity");
    settings
}
fn confirmed(store: &Store) -> Intent {
    let mut intent = prepare(store, &settings(), 8, 16).unwrap();
    let mut saved = serde_json::to_value(&intent.replacement).unwrap();
    saved["phase"] = json!("completed");
    saved["creation"] = json!({"state":"bound","worker_id":"worker2"});
    intent.replacement = serde_json::from_value(saved).unwrap();
    let mut worker = intent.original().unwrap().worker.unwrap();
    worker.id = "worker2".into();
    worker.vcpu_count = Some(8);
    worker.memory_in_gb = Some(16);
    intent.observed = Some(worker);
    write(store, &intent).unwrap();
    intent
}
#[test]
fn local_commit_recovers_each_boundary_and_keeps_workspace_sessions() {
    for after_storage in [false, true] {
        let (root, store) = fixture();
        let mut original = store.load().unwrap().unwrap();
        original.last_self_stop = Some(crate::cloud_runtime::worker_contract::SelfStop {
            at: 123,
            reason: "Previous worker stopped".into(),
            agent: None,
            session: None,
        });
        store.save(&original).unwrap();
        let intent = confirmed(&store);
        let sessions = serde_json::to_value(&intent.original().unwrap().sessions).unwrap();
        assert!(
            commit(
                &store,
                &intent,
                &mut |at| if matches!(at, Boundary::Storage) == after_storage {
                    Err(Error::Invalid("crash"))
                } else {
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(store.load().is_err());
        drop(store);
        let store = Store::lock(root.path()).unwrap();
        let intent = read(&store).unwrap().unwrap();
        let state = commit(&store, &intent, &mut |_| Ok(())).unwrap();
        assert_eq!((state.profile.cpu, state.profile.memory_gb), (8, 16));
        assert_eq!(state.worker.unwrap().id, "worker2");
        assert_eq!(state.stage, Stage::Readiness);
        assert!(state.last_self_stop.is_none());
        assert!(state.source_ready);
        assert_eq!(serde_json::to_value(state.sessions).unwrap(), sessions);
        let storage = load(&store, state.spec.as_ref().unwrap()).unwrap().unwrap();
        assert!(matches!(storage.state, State::Bound { creation: None, .. }));
        assert!(!root.path().join(JOURNAL).exists());
    }
}

#[test]
fn retained_resize_rechecks_ssh_identity_before_provider_work() {
    for (phase, observed) in [
        ("prepared", false),
        ("terminating", false),
        ("creating", false),
        ("completed", false),
        ("completed", true),
    ] {
        let (root, store) = fixture();
        let mut intent = confirmed(&store);
        let mut replacement = serde_json::to_value(&intent.replacement).unwrap();
        replacement["phase"] = json!(phase);
        if phase != "completed" {
            replacement["creation"] = json!({"state":"prepared"});
        }
        intent.replacement = serde_json::from_value(replacement).unwrap();
        if !observed {
            intent.observed = None;
        }
        write(&store, &intent).unwrap();
        let settings = identity_settings(&store);
        fs::write(
            root.path().join("identity.pub"),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIF2kk2kaQHcd1MHINbQ4muiDkEONuV3co+7ug6QOawIB fixture",
        )
        .unwrap();
        let journal = fs::read(root.path().join(JOURNAL)).unwrap();
        let deployment = fs::read(root.path().join("deployment.json")).unwrap();
        let storage = fs::read(root.path().join("workspace-volume.json")).unwrap();
        drop(store);
        let error = resize_compute_with(
            root.path(),
            &settings,
            8,
            16,
            &Cancellation::default(),
            &|_| panic!("Provider work must not begin"),
            || panic!("A mismatched identity must not reconnect"),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "The SSH private identity must match the replacement worker"
        );
        assert_eq!(fs::read(root.path().join(JOURNAL)).unwrap(), journal);
        assert_eq!(fs::read(root.path().join("deployment.json")).unwrap(), deployment);
        assert_eq!(fs::read(root.path().join("workspace-volume.json")).unwrap(), storage);
    }
}

#[test]
fn missing_private_identity_with_matching_public_key_cannot_begin_or_resume_resize() {
    for retained in [false, true] {
        let (root, store) = fixture();
        let mut settings = identity_settings(&store);
        settings.runpod_key_file = root.path().join("synthetic-provider-key");
        fs::write(&settings.runpod_key_file, "synthetic-test-only-token").unwrap();
        fs::set_permissions(&settings.runpod_key_file, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_file(&settings.ssh_identity_file).unwrap();
        if retained {
            prepare(&store, &settings, 8, 16).unwrap();
        }
        let journal = fs::read(root.path().join(JOURNAL)).ok();
        let deployment = fs::read(root.path().join("deployment.json")).unwrap();
        let storage = fs::read(root.path().join("workspace-volume.json")).unwrap();
        drop(store);
        let error = resize_compute_with(
            root.path(),
            &settings,
            8,
            16,
            &Cancellation::default(),
            &|_| panic!("Provider work must not begin"),
            || panic!("A missing private identity must not reconnect"),
        )
        .unwrap_err();
        assert!(matches!(error, Error::Io(error) if error.kind() == std::io::ErrorKind::NotFound));
        assert_eq!(fs::read(root.path().join(JOURNAL)).ok(), journal);
        assert_eq!(fs::read(root.path().join("deployment.json")).unwrap(), deployment);
        assert_eq!(fs::read(root.path().join("workspace-volume.json")).unwrap(), storage);
    }
}
#[test]
fn original_or_result_drift_cannot_rebind_the_workspace() {
    for change in 0..4 {
        let (_root, store) = fixture();
        let mut intent = confirmed(&store);
        match change {
            0 => intent.observed.as_mut().unwrap().id = "foreign".into(),
            1 => intent.observed.as_mut().unwrap().network_volume.as_mut().unwrap().id = Some("foreign-volume".into()),
            2 => {
                let mut state = store.load_during_storage_growth().unwrap().unwrap();
                state.revision = "changed".into();
                store.save(&state).unwrap();
            }
            _ => intent.storage.spec.operation_id = "foreign".into(),
        }
        assert!(commit(&store, &intent, &mut |_| Ok(())).is_err());
        assert!(store.root().join(JOURNAL).exists());
    }
}
#[test]
fn stopped_gpu_unready_and_remote_device_clouds_cannot_begin() {
    for change in 0..4 {
        let (_root, store) = fixture();
        let mut state = store.load().unwrap().unwrap();
        match change {
            0 => state.stop_requested = true,
            1 => state.profile.gpu = true,
            2 => state.stage = Stage::Provision,
            _ => {
                state.profile.capabilities.browserstack = Some(horizon_cloud::BrowserStack {
                    provider: horizon_cloud::BrowserStack::default_provider(),
                    targets: ["ios_phone".into()].into(),
                    local_ports: [8080].into(),
                });
            }
        }
        store.save(&state).unwrap();
        assert!(prepare(&store, &settings(), 8, 16).is_err());
        assert!(!store.root().join(JOURNAL).exists());
    }
}
#[test]
fn pending_resize_blocks_migration_and_disk_growth() {
    let (root, store) = fixture();
    prepare(&store, &settings(), 8, 16).unwrap();
    assert!(store.load().is_err());
    drop(store);
    assert_eq!(
        super::super::pending_resize(root.path()).unwrap(),
        Some(super::super::ResizeTarget::Compute { cpu: 8, memory_gb: 16 })
    );
    let error = crate::cloud_runtime::state::migration::MigratedStore::migrate(
        root.path(),
        "session",
        "workspace",
        crate::cloud_runtime::allocation::ControllerId::generate(),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("Compute resize is pending"));
    assert!(!root.path().join("migration.json").exists());
    let error = super::super::growth::grow_storage(root.path(), &settings(), 40, &Cancellation::default()).unwrap_err();
    assert!(error.to_string().contains("Compute resize is pending"));
}

#[test]
fn provider_refusal_unfences_only_before_termination_was_authorized() {
    for phase in ["prepared", "terminating"] {
        let (root, store) = fixture();
        let mut intent = prepare(&store, &settings(), 8, 16).unwrap();
        let mut replacement = serde_json::to_value(&intent.replacement).unwrap();
        replacement["phase"] = json!(phase);
        intent.replacement = serde_json::from_value(replacement).unwrap();
        write(&store, &intent).unwrap();
        let journal = fs::read(root.path().join(JOURNAL)).unwrap();
        let deployment = fs::read(root.path().join("deployment.json")).unwrap();
        let storage = fs::read(root.path().join("workspace-volume.json")).unwrap();
        assert!(matches!(
            record_observation(
                &store,
                &mut intent,
                Err(horizon_cloud::CloudError::Invalid("The original worker is not running"))
            ),
            Err(Error::Provider(horizon_cloud::CloudError::Invalid(
                "The original worker is not running"
            )))
        ));
        if phase == "prepared" {
            assert!(!root.path().join(JOURNAL).exists());
            assert!(store.load().unwrap().is_some());
        } else {
            assert_eq!(fs::read(root.path().join(JOURNAL)).unwrap(), journal);
            assert!(store.load().is_err());
        }
        assert_eq!(fs::read(root.path().join("deployment.json")).unwrap(), deployment);
        assert_eq!(fs::read(root.path().join("workspace-volume.json")).unwrap(), storage);
    }
}

#[test]
fn reconnect_failure_after_commit_retries_the_same_size_without_replacing_again() {
    let (root, store) = fixture();
    confirmed(&store);
    let settings = identity_settings(&store);
    drop(store);
    let first = resize_compute_with(root.path(), &settings, 8, 16, &Cancellation::default(), &|_| {}, || {
        Err(Error::Invalid("synthetic readiness failure"))
    });
    assert!(first.unwrap_err().to_string().contains("synthetic readiness failure"));
    assert!(!root.path().join(JOURNAL).exists());
    let state = resize_compute_with(root.path(), &settings, 8, 16, &Cancellation::default(), &|_| {}, || {
        let state = Store::lock(root.path())?.load()?.unwrap();
        assert_eq!(state.stage, Stage::Readiness);
        Ok(state)
    })
    .unwrap();
    assert_eq!(state.worker.unwrap().id, "worker2");
    assert_eq!((state.profile.cpu, state.profile.memory_gb), (8, 16));
}

#[test]
fn a_stale_public_sidecar_cannot_authorize_a_changed_or_invalid_private_key() {
    for phase in ["new", "prepared", "observed"] {
        for malformed in [false, true] {
            let (root, store) = fixture();
            let mut settings = identity_settings(&store);
            settings.runpod_key_file = root.path().join("synthetic-provider-key");
            fs::write(&settings.runpod_key_file, "synthetic-test-only-token").unwrap();
            fs::set_permissions(&settings.runpod_key_file, fs::Permissions::from_mode(0o600)).unwrap();
            match phase {
                "prepared" => {
                    prepare(&store, &settings, 8, 16).unwrap();
                }
                "observed" => {
                    confirmed(&store);
                }
                _ => {}
            }
            if malformed {
                fs::write(&settings.ssh_identity_file, "not an SSH private key").unwrap();
            } else {
                let (other, _) = fixture();
                fs::copy(other.path().join("identity"), &settings.ssh_identity_file).unwrap();
            }
            let journal = fs::read(root.path().join(JOURNAL)).ok();
            let deployment = fs::read(root.path().join("deployment.json")).unwrap();
            let storage = fs::read(root.path().join("workspace-volume.json")).unwrap();
            drop(store);
            let error = resize_compute_with(
                root.path(),
                &settings,
                8,
                16,
                &Cancellation::default(),
                &|_| panic!("No provider work"),
                || panic!("No reconnect"),
            )
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                "The SSH private identity must match the replacement worker"
            );
            assert_eq!(fs::read(root.path().join(JOURNAL)).ok(), journal);
            assert_eq!(fs::read(root.path().join("deployment.json")).unwrap(), deployment);
            assert_eq!(fs::read(root.path().join("workspace-volume.json")).unwrap(), storage);
        }
    }
}
