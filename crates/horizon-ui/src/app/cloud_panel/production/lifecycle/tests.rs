use super::*;
use crate::app::test_support::test_app;
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};

#[test]
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
        worker: None,
        sessions: Vec::new(),
        source_ready: false,
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
