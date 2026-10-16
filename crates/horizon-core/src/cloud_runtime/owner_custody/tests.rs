use super::*;
use crate::cloud_runtime::state::{
    exposure,
    service_admission::{Binding as SourceBinding, Checked, Phase},
};
#[cfg(target_os = "linux")]
use crate::cloud_runtime::{
    Cancellation,
    local_network::{
        State as WorkerState,
        endpoint::{Admitted, State as GatewayState},
    },
};
use horizon_cloud_protocol::exposure::{ConnectionLimits, ExposureMode, ExposureRequest};
use serde_json::json;
#[cfg(target_os = "linux")]
use std::{
    io::Write,
    time::{Duration, Instant},
};
use std::{sync::Barrier, thread};
use uuid::Uuid;

fn fixture() -> (tempfile::TempDir, OriginalCloud, Settings, CloudLaunch) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let image = format!("registry.example/worker@sha256:{}", "a".repeat(64));
    let state: Deployment = serde_json::from_value(json!({
        "version":1,"cloud_id":"service-cloud","repository":"/synthetic","revision":"a".repeat(40),
        "profile":{"provider":"hetzner","image":"registry.example/worker","cpu":4,"memory_gb":8},
        "stage":"Ready","operation":{"state":"bound","worker_id":"worker-a"},
        "spec":{"operation_id":"op-a","image_digest":image,"profile":{"provider":"hetzner","image":"registry.example/worker","cpu":4,"memory_gb":8},"public_key":"synthetic","registry_auth_id":null,"gpu_types":[],"cpu_flavors":[],"data_centers":[]},
        "worker":{"id":"worker-a","name":"worker-a","imageName":image,"desiredStatus":"RUNNING","publicIp":"192.0.2.5","portMappings":{"22":22022}},
        "sessions":[{"panel_id":"preserved-panel","agent":"codex","tmux":"preserved-session","branch":"preserved-branch","worktree":"/synthetic/unchanged"}],"source_ready":true
    })).unwrap();
    store.save(&state).unwrap();
    let launch = CloudLaunch {
        publishing_selection: None,
        deployment_started: true,
        id: state.cloud_id.clone(),
        revision: state.revision.clone(),
        profile_name: "synthetic".into(),
        profile: state.profile.clone(),
        placement: crate::cloud_panel::Placement::default(),
    };
    let settings: Settings = serde_json::from_value(json!({"runpod_key_file":"/synthetic/no-provider", "ssh_identity_file":"/synthetic/key", "docker_config":"/synthetic/no-docker", "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]})).unwrap();
    let scope = OwnerScope::new(
        "native-host",
        &Uuid::new_v4().to_string(),
        "workspace-a",
        "service-cloud",
    )
    .unwrap();
    let original = OriginalCloud::capture(store, &settings, scope, &launch).unwrap();
    (root, original, settings, launch)
}
fn admission(original: &OriginalCloud) -> Record {
    Record::armed(
        original.owner().clone(),
        "worker-a".into(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Checked {
            host: "device.example.ts.net".into(),
            address: "100.64.1.2:8123".parse().unwrap(),
            binding: SourceBinding::Tailnet {
                desktop: "desktop".into(),
                peer: "device".into(),
                name: "device.example.ts.net".into(),
            },
        },
    )
    .unwrap()
}
fn exposure(original: &OriginalCloud, settings: &Settings, launch: &CloudLaunch) -> exposure::Record {
    let state = original.snapshot(original.owner()).unwrap();
    exposure::Record::starting(
        exposure::Binding {
            owner: original.owner().clone(),
            launch: launch.clone(),
            spec: state.spec.unwrap(),
            worker_id: "worker-a".into(),
            pinned_ssh_args: original
                .recipient(original.owner(), settings, launch)
                .unwrap()
                .pinned_args()
                .to_vec(),
            provider_credential: original.root().join("synthetic-provider-reference").into(),
        },
        Uuid::new_v4(),
        exposure::Service {
            id: Uuid::new_v4(),
            policy: ExposureRequest {
                mode: ExposureMode::Public,
                target_port: 8123,
                sources: vec![],
                limits: ConnectionLimits::default(),
            },
            route: exposure::Route::Gate {},
        },
        Uuid::new_v4(),
        1_000,
    )
    .unwrap()
}

#[test]
fn concurrent_original_field_saves_preserve_sibling_intents_and_every_other_field() {
    let (root, original, settings, launch) = fixture();
    let first = admission(&original);
    let exposed = exposure(&original, &settings, &launch);
    let before = original.snapshot(original.owner()).unwrap();
    let barrier = std::sync::Arc::new(Barrier::new(2));
    let sharing = original.clone();
    let held = std::sync::Arc::clone(&barrier);
    let requested = first.clone();
    let writer = thread::spawn(move || {
        held.wait();
        sharing.replace_admission(sharing.owner(), None, requested).unwrap();
    });
    barrier.wait();
    original
        .replace_exposure(original.owner(), None, exposed.clone())
        .unwrap();
    writer.join().unwrap();
    let mut attached = first.clone();
    let identity = attached.attempt();
    let ids = serde_json::to_value(identity).unwrap();
    attached
        .set_attached(
            serde_json::from_value(ids["operation_id"].clone()).unwrap(),
            serde_json::from_value(ids["generation"].clone()).unwrap(),
            32001,
        )
        .unwrap();
    original
        .replace_admission(original.owner(), Some(&first), attached.clone())
        .unwrap();
    let mut observed = exposed.clone();
    let custody = Uuid::new_v4();
    observed.observed(observed.epoch(), custody, 1_001).unwrap();
    let service = observed.intents()[0].service.id;
    let operation = observed.intents()[0].operation;
    observed
        .active(observed.epoch(), custody, operation, 20000, 1_002)
        .unwrap();
    original
        .replace_exposure(original.owner(), Some(&exposed), observed.clone())
        .unwrap();
    let mut closed = attached.clone();
    closed.set_closed(closed.attempt()).unwrap();
    original
        .replace_admission(original.owner(), Some(&attached), closed.clone())
        .unwrap();
    let mut renewed = observed.clone();
    renewed
        .renewing(renewed.epoch(), custody, service, Uuid::new_v4(), 1_003)
        .unwrap();
    original
        .replace_exposure(original.owner(), Some(&observed), renewed.clone())
        .unwrap();
    let after = original.snapshot(original.owner()).unwrap();
    assert_eq!(after.service_admission, Some(closed));
    assert_eq!(after.exposure, Some(renewed));
    let mut unchanged = serde_json::to_value(after).unwrap();
    let mut previous = serde_json::to_value(before).unwrap();
    for value in [&mut unchanged, &mut previous] {
        for field in ["version", "service_admission", "exposure"] {
            value.as_object_mut().unwrap().remove(field);
        }
    }
    assert_eq!(unchanged, previous);
    assert!(matches!(
        Store::lock_existing(root.path()),
        Err(super::super::Error::Busy)
    ));
}

#[test]
fn stale_field_foreign_scope_missing_record_and_worker_drift_never_save() {
    let (root, original, settings, launch) = fixture();
    let requested = admission(&original);
    original
        .replace_admission(original.owner(), None, requested.clone())
        .unwrap();
    let path = root.path().join("deployment.json");
    let bytes = std::fs::read(&path).unwrap();
    assert!(matches!(
        original.replace_admission(original.owner(), None, requested.clone()),
        Err(Error::Conflict)
    ));
    let mut foreign = original.owner().clone();
    foreign.session_id = Uuid::new_v4();
    assert!(matches!(original.snapshot(&foreign), Err(Error::Scope)));
    assert!(matches!(
        original.replace_admission(&foreign, None, requested.clone()),
        Err(Error::Scope)
    ));
    let mut other = exposure(&original, &settings, &launch);
    let mut wire = serde_json::to_value(&other).unwrap();
    wire["binding"]["owner"]["session_id"] = json!(foreign.session_id);
    other = serde_json::from_value(wire).unwrap();
    assert!(matches!(
        original.replace_exposure(original.owner(), None, other),
        Err(Error::Binding)
    ));
    let mut changed_settings = settings.clone();
    changed_settings.ssh_identity_file = root.path().join("another-identity-reference");
    assert!(matches!(
        original.checked_snapshot(original.owner(), &changed_settings, &launch),
        Err(Error::Binding)
    ));
    let mut changed_launch = launch.clone();
    changed_launch.profile_name = "other-native-launch".into();
    assert!(matches!(
        original.checked_snapshot(original.owner(), &settings, &changed_launch),
        Err(Error::Binding)
    ));
    let coherent = original.checked_snapshot(original.owner(), &settings, &launch).unwrap();
    assert_eq!(coherent.state.service_admission, Some(requested.clone()));
    assert_eq!(coherent.selection, super::super::tailnet::Selection::default());
    let selection = root.path().join("tailnet.json");
    std::fs::write(&selection, b"{invalid").unwrap();
    assert!(matches!(
        original.checked_snapshot(original.owner(), &settings, &launch),
        Err(Error::Selection)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(selection).unwrap();
    assert!(coherent.recipient == original.recipient(original.owner(), &settings, &launch).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(original.snapshot(original.owner()), Err(Error::Missing)));
    assert!(matches!(
        original.replace_admission(original.owner(), None, requested.clone()),
        Err(Error::Missing)
    ));
    assert!(!path.exists());
    let mut drift: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    drift["worker"]["id"] = json!("successor");
    let changed = serde_json::to_vec(&drift).unwrap();
    std::fs::write(&path, &changed).unwrap();
    assert!(matches!(original.snapshot(original.owner()), Err(Error::Binding)));
    assert!(matches!(
        original.replace_admission(original.owner(), None, requested),
        Err(Error::Binding)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), changed);
}

#[cfg(target_os = "linux")]
#[test]
fn old_session_cancellation_closes_actual_source_during_blocked_transaction_and_keeps_original_lock() {
    let (root, original, settings, launch) = fixture();
    let cancel = Cancellation::default();
    let admitted = Admitted::synthetic_shared(
        original.clone(),
        &settings,
        original.owner().clone(),
        &launch,
        cancel.clone(),
    );
    let until = Instant::now() + Duration::from_secs(3);
    while !matches!(admitted.status().unwrap().transport.worker, WorkerState::Active { .. }) {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(10));
    }
    let held = admitted.live_cloud();
    let path = root.path().join("deployment.json");
    let backup = root.path().join("original-record");
    let fifo = root.path().join("blocked-record");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::copy(&path, &backup).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    std::fs::rename(&fifo, &path).unwrap();
    let reading = original.clone();
    let reader = thread::spawn(move || reading.snapshot(reading.owner()));
    let mut release = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    // The original reader retains the FIFO inode; future reads see the exact original record.
    std::fs::rename(&backup, &path).unwrap();
    assert!(!cancel.is_cancelled());
    assert!(matches!(
        admitted.status().unwrap().transport.gateway.state,
        GatewayState::Active
    ));
    let mut successor = original.owner().clone();
    successor.session_id = Uuid::new_v4();
    assert!(matches!(original.snapshot(&successor), Err(Error::Scope)));
    assert!(admitted.observe(&successor, &settings, &launch).is_err());
    assert!(cancel.is_cancelled());
    assert!(matches!(
        admitted.status().unwrap().transport.gateway.state,
        GatewayState::Revoked { .. }
    ));
    assert!(matches!(
        Store::lock_existing(root.path()),
        Err(super::super::Error::Busy)
    ));
    release.write_all(&bytes).unwrap();
    drop(release);
    assert!(reader.join().unwrap().is_ok());
    drop(admitted);
    drop(original);
    assert!(matches!(
        Store::lock_existing(root.path()),
        Err(super::super::Error::Busy)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    drop(held);
    let store = Store::lock_existing(root.path()).unwrap();
    assert!(matches!(
        store.load().unwrap().unwrap().service_admission.unwrap().phase(),
        Phase::Attached { .. }
    ));
}
