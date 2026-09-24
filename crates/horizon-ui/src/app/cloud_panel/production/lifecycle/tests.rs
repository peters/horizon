use super::*;
use crate::app::test_support::test_app;
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};

fn failed_setup_runtime() -> Runtime {
    let config = CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 4\n    memory_gb: 8\n    capabilities:\n      browserstack:\n        provider: account\n").unwrap();
    let state = serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "fixture", "repository": "/synthetic", "revision": "a".repeat(40),
        "profile": config.profiles["dev"], "stage": "Readiness",
        "operation": cloud_runtime::CreateState::Bound { worker_id: "fixture-worker".into() },
        "spec": null, "worker": null, "sessions": [], "browserstack_released": false
    }))
    .unwrap();
    Runtime {
        stage: Some(Stage::Readiness),
        state: Some(state),
        error: Some("Private runtime upload failed".into()),
        ..Default::default()
    }
}

#[test]
fn release_after_failed_setup_has_its_own_completion_channel() {
    let mut runtime = failed_setup_runtime();
    let (old_sender, old_receiver) = channel();
    drop(old_receiver);
    runtime.sender = Some(old_sender);
    assert!(runtime.can_release_remote_devices());
    let (sender, receiver) = channel();
    runtime.remote_release = Some(receiver);
    assert!(!runtime.can_release_remote_devices());
    assert!(runtime.needs_repaint());
    runtime.poll_remote_release();
    assert!(runtime.remote_release.is_some());
    let mut released = runtime.state.clone().unwrap();
    released.browserstack_released = true;
    sender.send(Ok(released)).unwrap();
    runtime.poll_remote_release();
    assert!(runtime.remote_release.is_none());
    assert!(runtime.state.as_ref().unwrap().browserstack_released);
    assert_eq!(runtime.stage, Some(Stage::Readiness));
    assert!(runtime.error.is_some(), "cleanup does not erase the setup failure");
    assert!(!runtime.can_release_remote_devices());
}

#[test]
fn failed_or_lost_release_retains_the_pending_cleanup_state() {
    for disconnected in [false, true] {
        let mut runtime = failed_setup_runtime();
        let (sender, receiver) = channel();
        runtime.remote_release = Some(receiver);
        if disconnected {
            drop(sender);
        } else {
            sender
                .send(Err(cloud_runtime::Error::Invalid("Release uncertain")))
                .unwrap();
        }
        runtime.poll_remote_release();
        assert!(runtime.remote_release.is_none());
        assert!(!runtime.state.as_ref().unwrap().browserstack_released);
        assert_eq!(runtime.stage, Some(Stage::Readiness));
        assert_eq!(runtime.error.as_deref(), Some("Private runtime upload failed"));
        assert!(runtime.remote_release_error.is_some());
        assert!(runtime.can_release_remote_devices());
    }
}

#[test]
fn successful_release_retry_clears_only_its_own_error() {
    for setup_error in [None, Some("Unresolved deployment failure".into())] {
        let mut runtime = failed_setup_runtime();
        runtime.error.clone_from(&setup_error);
        let (sender, receiver) = channel();
        runtime.remote_release = Some(receiver);
        sender
            .send(Err(cloud_runtime::Error::Invalid("Release uncertain")))
            .unwrap();
        runtime.poll_remote_release();
        assert!(runtime.remote_release_error.is_some());
        assert_eq!(runtime.error, setup_error);

        let mut released = runtime.state.clone().unwrap();
        released.browserstack_released = true;
        let (sender, receiver) = channel();
        runtime.remote_release = Some(receiver);
        sender.send(Ok(released)).unwrap();
        runtime.poll_remote_release();
        assert!(runtime.remote_release_error.is_none());
        assert_eq!(runtime.error, setup_error);
        assert!(runtime.state.as_ref().unwrap().browserstack_released);
    }
}

#[test]
fn lifecycle_cleanup_clears_a_previous_release_error() {
    for stopped in [false, true] {
        let (_temp, mut app) = test_app();
        let ctx = egui::Context::default();
        app.prepare_production_clouds(&ctx);
        let mut runtime = failed_setup_runtime();
        runtime.remote_release_error = Some("Release uncertain".into());
        let mut state = runtime.state.clone().unwrap();
        state.browserstack_released = true;
        let (sender, receiver) = channel();
        runtime.receiver = Some(receiver);
        app.cloud_prototype.production.runtimes.insert(1, runtime);
        sender
            .send(if stopped {
                Event::Stopped(Box::new(state))
            } else {
                Event::Snapshot(Box::new(state))
            })
            .unwrap();
        app.prepare_production_clouds(&ctx);
        let runtime = &app.cloud_prototype.production.runtimes[&1];
        assert!(runtime.remote_release_error.is_none());
        assert!(runtime.state.as_ref().unwrap().browserstack_released);
        if !stopped {
            assert_eq!(runtime.error.as_deref(), Some("Private runtime upload failed"));
        }
    }
}

#[test]
fn release_waits_for_setup_and_requires_a_bound_worker() {
    let mut runtime = failed_setup_runtime();
    let (_sender, receiver) = channel();
    runtime.receiver = Some(receiver);
    assert!(!runtime.can_release_remote_devices());
    runtime.stage = Some(Stage::Ready);
    assert!(
        runtime.can_release_remote_devices(),
        "ready discovery is not a deployment operation"
    );
    runtime.receiver = None;
    runtime.state.as_mut().unwrap().operation = cloud_runtime::CreateState::Requested;
    assert!(!runtime.can_release_remote_devices());
}

#[test]
#[cfg(unix)]
fn cloud_removal_requires_readable_unlocked_and_safe_durable_state() {
    let (temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let workspace = app.board.create_workspace("cloud fixture");
    let mut group = CloudGroup::new(
        1,
        "Fixture".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        temp.path().into(),
        [0.0, 0.0],
    );
    let config = CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 4\n    memory_gb: 8\n").unwrap();
    let profile = config.profiles["dev"].clone();
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: profile.clone(),
    });
    app.cloud_prototype.groups.0.push(group);
    app.cloud_prototype.root = Some(temp.path().into());
    let root = temp.path().join("fixture");

    app.remove_deleted_cloud(1, &ctx);
    assert_eq!(
        app.cloud_prototype.groups.0.len(),
        1,
        "missing deployed state must retain identity"
    );
    std::fs::write(root.join("deployment.json"), b"{broken").unwrap();
    app.remove_deleted_cloud(1, &ctx);
    assert_eq!(
        app.cloud_prototype.groups.0.len(),
        1,
        "corrupt state must retain identity"
    );
    let store = Store::lock(&root).unwrap();
    let mut state = cloud_runtime::state::Deployment {
        version: 1,
        cloud_id: "fixture".into(),
        repository: temp.path().into(),
        revision: "a".repeat(40),
        profile,
        stage: Stage::Provision,
        operation: cloud_runtime::CreateState::Requested,
        spec: None,
        registry_generation: None,
        worker: None,
        sessions: Vec::new(),
        source_ready: false,
        ready_after_seconds: None,
        ready_history: horizon_core::cloud_runtime::state::ReadyHistory::Unobserved,
        stop_requested: false,
        browserstack_released: false,
        browserstack_targets: std::collections::BTreeSet::new(),
    };
    store.save(&state).unwrap();
    app.remove_deleted_cloud(1, &ctx);
    assert_eq!(
        app.cloud_prototype.groups.0.len(),
        1,
        "competing controller must retain identity"
    );
    drop(store);
    app.remove_deleted_cloud(1, &ctx);
    assert_eq!(
        app.cloud_prototype.groups.0.len(),
        1,
        "uncertain allocation must retain identity"
    );
    let store = Store::lock(&root).unwrap();
    state.operation = cloud_runtime::CreateState::Prepared;
    store.save(&state).unwrap();
    drop(store);
    app.remove_deleted_cloud(1, &ctx);
    assert!(
        app.cloud_prototype.groups.0.is_empty(),
        "unallocated cloud can be removed"
    );
}

#[test]
fn restoring_an_absolute_cloud_identity_never_touches_its_target() {
    let (temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let workspace = app.board.create_workspace("cloud fixture");
    let mut group = CloudGroup::new(
        1,
        "Invalid persisted cloud".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        temp.path().into(),
        [0.0, 0.0],
    );
    let config = CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 4\n    memory_gb: 8\n").unwrap();
    let outside = temp.path().join("must-not-be-created");
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: outside.to_str().unwrap().into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: config.profiles["dev"].clone(),
    });
    app.board.cloud_groups.0.push(group);
    app.cloud_prototype.initialized = false;
    app.restore_cloud_state(&ctx);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert!(runtime.state_unavailable);
    assert!(runtime.error.as_ref().unwrap().contains("Invalid cloud identity"));
    assert!(!outside.exists());
    app.start_production_deployment(1, &ctx);
    app.remove_deleted_cloud(1, &ctx);
    assert!(!outside.exists());
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
}
