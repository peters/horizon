use super::*;
use crate::app::test_support::{raw_input, run_app_frame, run_app_frame_with_input, test_app, test_app_with_startup};
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};
use horizon_core::{PanelId, PanelOptions, RuntimeState, StartupDecision, WorkspaceId};

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
fn a_worker_found_stopped_offers_resume_instead_of_an_error() {
    for (stage, error) in [(Stage::Stopped, false), (Stage::Readiness, true)] {
        let mut runtime = failed_setup_runtime();
        runtime.error = None;
        let mut state = runtime.state.clone().unwrap();
        state.stage = stage;
        let (sender, receiver) = channel();
        runtime.recovery_receiver = Some(receiver);
        sender
            .send(Ok(cloud_runtime::lifecycle::ReconciledDeployment {
                state,
                report: serde_json::from_value(serde_json::json!({
                    "operation_id": "fixture",
                    "outcome": {"status": "inactive", "worker_id": "fixture-worker"}
                }))
                .unwrap(),
            }))
            .unwrap();
        runtime.poll_recovery();
        assert_eq!(runtime.stage, Some(stage));
        assert_eq!(runtime.error.is_some(), error, "{stage:?}");
    }
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
        image_replacement: None,
        session_restart: None,
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

/// An app whose production cloud list is live for its session, after startup frames.
fn live_cloud_app() -> (tempfile::TempDir, egui::Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    for _ in 0..2 {
        run_app_frame(&ctx, &mut app);
    }
    assert!(app.cloud_state_is_live());
    app.cloud_prototype.root = Some(temp.path().into());
    (temp, ctx, app)
}

/// A cloud whose worker was never allocated, so it can be removed.
fn add_unallocated_cloud(app: &mut HorizonApp, issue: u32, workspace: WorkspaceId) {
    let config = CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 4\n    memory_gb: 8\n").unwrap();
    let mut group = CloudGroup::new(
        issue,
        format!("Cloud {issue}"),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        std::path::PathBuf::new(),
        [0.0, 0.0],
    );
    group.remote = Some(CloudLaunch {
        deployment_started: false,
        id: format!("fixture{issue}"),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: config.profiles["dev"].clone(),
    });
    app.cloud_prototype.groups.0.push(group);
}

fn add_editor(app: &mut HorizonApp, workspace: WorkspaceId) -> PanelId {
    app.board
        .create_panel(
            PanelOptions {
                kind: horizon_core::PanelKind::Editor,
                ..PanelOptions::default()
            },
            workspace,
        )
        .unwrap()
}

#[test]
#[cfg_attr(windows, ignore = "cloud state stores need Unix directory durability")]
fn removing_the_only_cloud_removes_its_workspace_on_the_next_frame() {
    for survivors in [1, 2] {
        for with_member in [false, true] {
            let (_temp, ctx, mut app) = live_cloud_app();
            let mut most_recent = None;
            let mut survivor_workspace = None;
            for index in 0..survivors {
                let workspace = app.board.create_workspace(&format!("Local {index}"));
                most_recent = Some(add_editor(&mut app, workspace));
                survivor_workspace = Some(workspace);
            }
            let cloud = app.board.create_workspace("Cloud");
            add_unallocated_cloud(&mut app, 1, cloud);
            let member = with_member.then(|| {
                let member = add_editor(&mut app, cloud);
                app.cloud_prototype.groups.0[0].attach(&mut app.board, member);
                member
            });
            run_app_frame(&ctx, &mut app);
            match member {
                Some(member) => app.board.focus(member),
                None => app.board.focus_workspace(cloud),
            }
            run_app_frame(&ctx, &mut app);
            assert!(
                app.board.workspace(cloud).is_some(),
                "an empty cloud keeps its workspace"
            );

            app.remove_deleted_cloud(1, &ctx);
            assert!(app.cloud_prototype.groups.0.is_empty());
            assert!(member.is_none_or(|member| app.board.panel(member).is_none()));
            run_app_frame(&ctx, &mut app);

            let case = format!("survivors: {survivors}, member: {with_member}");
            assert!(app.board.workspace(cloud).is_none(), "{case}");
            assert_eq!(app.board.focused, most_recent, "{case}");
            assert_eq!(app.board.active_workspace, survivor_workspace, "{case}");
        }
    }
}

#[test]
#[cfg_attr(windows, ignore = "cloud state stores need Unix directory durability")]
fn removing_a_cloud_keeps_a_workspace_that_is_still_in_use() {
    for keeper in ["another cloud", "an ordinary panel", "a hidden panel"] {
        let (_temp, ctx, mut app) = live_cloud_app();
        let cloud = app.board.create_workspace("Cloud");
        add_unallocated_cloud(&mut app, 1, cloud);
        let ordinary = if keeper == "another cloud" {
            add_unallocated_cloud(&mut app, 2, cloud);
            None
        } else {
            let panel = add_editor(&mut app, cloud);
            if keeper == "a hidden panel" {
                assert!(app.board.set_panel_visible(panel, false));
            }
            Some(panel)
        };
        run_app_frame(&ctx, &mut app);

        app.remove_deleted_cloud(1, &ctx);
        assert!(app.cloud_prototype.groups.0.iter().all(|group| group.issue != 1));
        for _ in 0..2 {
            run_app_frame(&ctx, &mut app);
        }
        assert!(app.board.workspace(cloud).is_some(), "{keeper} keeps the workspace");
        if let Some(panel) = ordinary {
            assert!(app.board.panel(panel).is_some());
            app.close_panel(panel);
            assert!(
                app.board.workspace(cloud).is_none(),
                "without a cloud it closes with its last panel"
            );
        }
    }
}

#[test]
#[cfg_attr(windows, ignore = "cloud state stores need Unix directory durability")]
fn cancelling_a_creation_releases_the_workspace_of_its_removed_cloud() {
    let (temp, ctx, mut app) = live_cloud_app();
    let frame = |app: &mut HorizonApp, escape: bool| {
        let mut input = raw_input([1400.0, 900.0], None);
        if escape {
            input.events.push(egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: Some(egui::Key::Escape),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        run_app_frame_with_input(&ctx, app, input);
    };
    let local = app.board.create_workspace("Local");
    let remaining = add_editor(&mut app, local);
    let cloud = app.board.create_workspace("Cloud");
    app.board.workspace_mut(cloud).unwrap().cwd = Some(temp.path().into());
    add_unallocated_cloud(&mut app, 1, cloud);
    frame(&mut app, false);
    app.open_workspace_cloud(&ctx, cloud);
    app.remove_deleted_cloud(1, &ctx);
    for _ in 0..3 {
        frame(&mut app, false);
    }
    assert!(app.cloud_prototype.production.creating);
    assert!(
        app.board.workspace(cloud).is_some(),
        "the open creation keeps its target"
    );

    frame(&mut app, true);
    assert!(!app.cloud_prototype.production.creating);
    assert!(
        app.cloud_prototype.creation_holds.is_empty(),
        "the frame that ends the creation releases its hold"
    );
    frame(&mut app, false);
    assert!(app.board.workspace(cloud).is_none());
    assert_eq!(app.board.focused, Some(remaining));
    assert_eq!(app.board.active_workspace, Some(local));
}

/// A cloud whose worker was never requested, so deletion needs no provider request.
fn seed_unallocated_cloud(
    app: &mut crate::app::HorizonApp,
    root: &std::path::Path,
) -> (tempfile::NamedTempFile, cloud_runtime::state::Deployment) {
    let workspace = app.board.create_workspace("cloud fixture");
    let mut group = CloudGroup::new(
        1,
        "Fixture".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        root.into(),
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
    app.cloud_prototype.root = Some(root.into());
    let mut key = tempfile::NamedTempFile::new_in(root).unwrap();
    std::io::Write::write_all(&mut key, b"synthetic-test-key").unwrap();
    let settings = serde_json::json!({
        "runpod_key_file": key.path(), "ssh_identity_file": root.join("ssh"),
        "docker_config": root.join("docker"), "registry_pull_auth_id": null,
        "cpu_flavors": [], "gpu_types": []
    });
    std::fs::write(root.join("settings.json"), settings.to_string()).unwrap();
    let state = serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "fixture", "repository": root, "revision": "a".repeat(40),
        "profile": profile, "stage": "Provision", "operation": {"state": "prepared"},
        "spec": null, "worker": null, "sessions": []
    }))
    .unwrap();
    Store::lock(&root.join("fixture")).unwrap().save(&state).unwrap();
    (key, state)
}

/// Starts a deletion after a failed deployment attempt and waits for its result.
fn delete_after_a_stale_attempt(app: &mut crate::app::HorizonApp, ctx: &egui::Context) {
    use std::time::{Duration, Instant};
    let runtime = app.cloud_prototype.production.runtimes.entry(1).or_default();
    runtime.stage = Some(Stage::Provision);
    runtime.error = Some("Earlier deployment failure".into());
    runtime.remote_release_error = Some("Earlier hosted-device release failure".into());
    runtime.progress.stage(
        Stage::Provision,
        Instant::now().checked_sub(Duration::from_secs(60)).unwrap(),
    );
    app.change_production_worker(1, Action::Delete, ctx);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    // No hosted devices to release, so the stand-in step never offers Cancel.
    assert!(
        runtime
            .stage
            .is_some_and(|stage| Stage::DELETION.contains(&stage) && stage != Stage::ReleaseDevices)
    );
    assert!(runtime.error.is_none());
    assert!(runtime.remote_release_error.is_none());
    assert_eq!(runtime.progress.stage_label(Stage::Provision), "Provision worker");
    assert!(runtime.needs_repaint(), "the elapsed time stays live");
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.cloud_prototype.production.runtimes[&1].receiver.is_some() {
        assert!(Instant::now() < deadline, "deletion must finish");
        std::thread::sleep(Duration::from_millis(10));
        app.prepare_production_clouds(ctx);
    }
}

#[test]
#[cfg_attr(
    windows,
    ignore = "cloud control requires durable directory updates, which are Unix-only"
)]
fn deletion_replaces_a_stale_attempt_and_reports_its_steps_and_total_time() {
    let (temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.prepare_production_clouds(&ctx);
    let (_key, mut state) = seed_unallocated_cloud(&mut app, temp.path());
    let root = temp.path().join("fixture");

    delete_after_a_stale_attempt(&mut app, &ctx);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert_eq!(
        runtime.stage,
        Some(Stage::Provision),
        "a failed deletion shows the saved stage"
    );
    assert!(runtime.error.as_ref().unwrap().contains("No worker was requested"));
    assert!(runtime.progress.ended_in(Stage::Deleted).is_none());
    assert!(
        runtime.progress.is_deletion(),
        "the card keeps the deletion steps beside the error"
    );

    state.spec = Some(
        serde_json::from_value(serde_json::json!({
            "operation_id": "fixture", "image_digest": format!("example/worker@sha256:{}", "a".repeat(64)),
            "profile": state.profile, "public_key": "unused-fixture-key", "registry_auth_id": null,
            "gpu_types": [], "cpu_flavors": ["cpu3c"], "data_centers": []
        }))
        .unwrap(),
    );
    Store::lock(&root).unwrap().save(&state).unwrap();
    delete_after_a_stale_attempt(&mut app, &ctx);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert_eq!(runtime.stage, Some(Stage::Deleted));
    assert_eq!(runtime.error.as_deref(), Some(super::super::DELETED_RESOURCES_MESSAGE));
    assert!(runtime.cancel.is_none());
    assert!(!runtime.needs_repaint());
    assert!(runtime.progress.ended_in(Stage::Deleted).is_some());
    assert!(
        runtime
            .progress
            .stage_label(Stage::DeleteStorage)
            .starts_with("Delete workspace storage · 0m")
    );
    // Only core's events enter the timeline: the stand-in first step and the skipped
    // worker step show no duration.
    assert_eq!(
        runtime.progress.stage_label(Stage::ReleaseDevices),
        "Release hosted devices"
    );
    assert_eq!(runtime.progress.stage_label(Stage::DeleteWorker), "Delete worker");
    assert_eq!(
        Store::lock(&root).unwrap().load().unwrap().unwrap().stage,
        Stage::Deleted
    );
    app.remove_deleted_cloud(1, &ctx);
    assert!(app.cloud_prototype.groups.0.is_empty());
}

#[test]
fn deletion_offers_cancel_only_while_hosted_devices_can_still_be_released() {
    let state =
        |operation: serde_json::Value, browserstack: bool, released: bool| -> cloud_runtime::state::Deployment {
            let mut capabilities = serde_json::json!({});
            if browserstack {
                capabilities["browserstack"] = serde_json::json!({"targets": []});
            }
            serde_json::from_value(serde_json::json!({
                "version": 1, "cloud_id": "fixture", "repository": "/synthetic", "revision": "a",
                "profile": {"provider": "runpod", "image": "registry.example/worker", "cpu": 4, "memory_gb": 8,
                    "capabilities": capabilities},
                "stage": "Ready", "operation": operation, "spec": null, "worker": null, "sessions": [],
                "browserstack_released": released
            }))
            .unwrap()
        };
    let bound = serde_json::json!({"state": "bound", "worker_id": "worker1"});
    let prepared = serde_json::json!({"state": "prepared"});
    for (record, first) in [
        (Some(state(bound.clone(), true, false)), Stage::ReleaseDevices),
        (Some(state(bound.clone(), true, true)), Stage::DeleteWorker),
        (Some(state(bound, false, false)), Stage::DeleteWorker),
        (Some(state(prepared.clone(), true, false)), Stage::DeleteStorage),
        (Some(state(prepared, false, false)), Stage::DeleteStorage),
        (None, Stage::DeleteWorker),
    ] {
        assert_eq!(super::first_deletion_step(record.as_ref()), first);
    }
}

#[test]
fn another_operation_after_a_failed_deletion_drops_its_steps() {
    for (action, name) in [(Action::Stop, "Stop"), (Action::Resume, "Resume")] {
        let mut runtime = Runtime::default();
        runtime.progress.begin_deletion();
        runtime.progress.stage(Stage::DeleteWorker, std::time::Instant::now());
        runtime.progress.finish(std::time::Instant::now());
        assert!(runtime.progress.is_deletion());
        super::begin_operation(&mut runtime, action);
        assert!(!runtime.progress.is_deletion(), "{name} shows its own progress");
        assert_eq!(runtime.stage, Some(Stage::Provision));
    }
    let mut runtime = Runtime::default();
    super::begin_operation(&mut runtime, Action::Delete);
    assert!(runtime.progress.is_deletion());
}

#[test]
fn deletion_time_ends_when_the_worker_finished_not_when_the_ui_caught_up() {
    use std::time::{Duration, Instant};
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.prepare_production_clouds(&ctx);
    let started = Instant::now().checked_sub(Duration::from_secs(60)).unwrap();
    let mut runtime = Runtime::default();
    runtime.progress.begin_deletion();
    runtime.progress.stage(Stage::DeleteWorker, started);
    let (sender, receiver) = channel();
    runtime.receiver = Some(receiver);
    app.cloud_prototype.production.runtimes.insert(1, runtime);
    // The deletion finished 12 s in; the UI only processes it now, 48 s later.
    sender.send(Event::Deleted(started + Duration::from_secs(12))).unwrap();
    app.prepare_production_clouds(&ctx);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert_eq!(runtime.stage, Some(Stage::Deleted));
    assert_eq!(runtime.progress.ended_in(Stage::Deleted), Some(Duration::from_secs(12)));
}
