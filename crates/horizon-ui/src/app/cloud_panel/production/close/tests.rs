use super::*;
use crate::app::test_support::test_app;
use crate::test_egui::DiscardTextures;
use horizon_core::cloud_panel::{CloudGroup, CloudLaunch};
use horizon_core::cloud_runtime::state::Deployment;
#[cfg(unix)]
use horizon_core::cloud_runtime::{CreateState, state::Store};

fn deployment() -> Deployment {
    serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "fixture", "repository": "/synthetic", "revision": "a".repeat(40),
        "profile": {"provider": "runpod", "image": "registry.example/worker", "cpu": 4, "memory_gb": 8},
        "stage": "Ready", "operation": {"state": "bound", "worker_id": "worker"}, "sessions": []
    }))
    .unwrap()
}

fn add_cloud(app: &mut HorizonApp, root: &std::path::Path, state: Option<Deployment>) {
    let workspace = app.board.create_workspace("Fixture");
    let mut group = CloudGroup::new(
        101,
        "Fixture".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        root.into(),
        [0.0, 0.0],
    );
    group.remote = Some(CloudLaunch {
        id: "fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: deployment().profile,
        placement: horizon_core::cloud_panel::Placement::default(),
        deployment_started: state.is_some(),
    });
    app.cloud_prototype.groups.0.push(group);
    app.cloud_prototype.root = Some(root.into());
    app.cloud_prototype.production.runtimes.insert(
        101,
        Runtime {
            stage: state.as_ref().map(|s| s.stage),
            state,
            ..Default::default()
        },
    );
}

#[test]
fn closing_requires_known_idle_resources() {
    let mut runtime = Runtime::default();
    assert!(matches!(close_action(&runtime, false), Ok(Action::Remove)));
    assert!(close_action(&runtime, true).is_err());
    runtime.state = Some(deployment());
    assert!(matches!(close_action(&runtime, true), Ok(Action::Delete)));
    let (_, rx) = std::sync::mpsc::channel();
    runtime.receiver = Some(rx);
    runtime.stage = Some(Stage::Provision);
    assert!(close_action(&runtime, true).is_err());
    runtime.stage = Some(Stage::Ready);
    assert!(
        matches!(close_action(&runtime, true), Ok(Action::Delete)),
        "ready discovery does not block closing"
    );
    runtime.receiver = None;
    runtime.state_unavailable = true;
    assert!(close_action(&runtime, true).is_err());
}

#[test]
#[cfg(unix)] // Durable cloud records require Unix directory durability.
fn failed_image_push_closes_without_worker_deletion_and_rechecks_storage() {
    let (temp, mut app) = test_app();
    let mut state = deployment();
    state.stage = Stage::Push;
    state.operation = CreateState::Prepared;
    state.spec = None;
    let path = temp.path().join("fixture");
    Store::lock(&path).unwrap().save(&state).unwrap();
    add_cloud(&mut app, temp.path(), Some(state));
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&101).unwrap();
    runtime.error = Some("error from registry: denied".into());
    assert_eq!(close_action(runtime, true), Ok(Action::Remove));

    std::fs::write(path.join("workspace-volume.required"), "").unwrap();
    app.remove_deleted_cloud(101, &egui::Context::default());
    assert_eq!(app.cloud_prototype.groups.0.len(), 1, "storage must remain tracked");
    std::fs::remove_file(path.join("workspace-volume.required")).unwrap();
    app.remove_deleted_cloud(101, &egui::Context::default());
    assert!(app.cloud_prototype.groups.0.is_empty());
    assert!(!app.cloud_prototype.production.runtimes.contains_key(&101));
    assert!(
        !temp.path().join("settings.json").exists(),
        "removal needs no provider credentials"
    );
}

#[test]
fn close_confirmation_is_modal_and_escape_preserves_cloud() {
    let (temp, mut app) = test_app();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    app.request_cloud_close(101);
    assert!(app.host_dialog_open());
    let ctx = egui::Context::default();
    for _ in 0..2 {
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                app.render_cloud_close_confirmation(ui.ctx());
            })
            .discard_textures();
    }
    let _ = ctx
        .run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                }],
                ..Default::default()
            },
            |ui| app.render_cloud_close_confirmation(ui.ctx()),
        )
        .discard_textures();
    assert!(!app.cloud_close_confirmation_open());
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    assert!(app.cloud_prototype.production.runtimes[&101].receiver.is_none());
    let _ = ctx
        .run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: true,
                    modifiers: egui::Modifiers::default(),
                }],
                ..Default::default()
            },
            |ui| {
                app.filter_held_navigation_keys(ui.ctx());
                assert!(
                    !ui.input(|input| input.key_pressed(egui::Key::Escape)),
                    "held Escape must not escape to the canvas"
                );
            },
        )
        .discard_textures();
}

#[test]
fn switching_sessions_discards_all_previous_close_intent() {
    let (_temp, mut app) = test_app();
    app.cloud_prototype.initialized = true;
    app.cloud_prototype.production.session_id = Some("previous-session".into());
    app.cloud_prototype.production.close.confirming = Some(101);
    app.cloud_prototype.production.close.deleting.insert(102);
    app.restore_cloud_state(&egui::Context::default());
    assert!(app.cloud_prototype.production.close.confirming.is_none());
    assert!(app.cloud_prototype.production.close.deleting.is_empty());
}

#[test]
#[cfg(unix)] // Durable cloud records require Unix directory durability.
fn close_waits_for_completion_and_rechecks_durable_cleanup() {
    let (temp, mut app) = test_app();
    let state = deployment();
    add_cloud(&mut app, temp.path(), Some(state.clone()));
    Store::lock(&temp.path().join("fixture")).unwrap().save(&state).unwrap();
    app.cloud_prototype.production.close.deleting.insert(101);
    let (tx, rx) = std::sync::mpsc::channel();
    app.cloud_prototype.production.runtimes.get_mut(&101).unwrap().receiver = Some(rx);
    let ctx = egui::Context::default();
    app.finish_closing_clouds(&ctx);
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    drop(tx);
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&101).unwrap();
    runtime.receiver = None;
    runtime.stage = Some(Stage::Deleted);
    app.finish_closing_clouds(&ctx);
    assert_eq!(
        app.cloud_prototype.groups.0.len(),
        1,
        "stale success cannot discard a bound worker"
    );

    let mut deleted = state;
    deleted.stage = Stage::Deleted;
    deleted.operation = CreateState::Terminated {
        worker_id: "worker".into(),
    };
    Store::lock(&temp.path().join("fixture"))
        .unwrap()
        .save(&deleted)
        .unwrap();
    app.cloud_prototype.production.close.deleting.insert(101);
    app.finish_closing_clouds(&ctx);
    assert!(app.cloud_prototype.groups.0.is_empty());
    assert!(!app.cloud_prototype.production.runtimes.contains_key(&101));
}

#[test]
fn failed_deletion_retains_cloud_and_error_for_retry() {
    let (temp, mut app) = test_app();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&101).unwrap();
    runtime.stage = Some(Stage::DeleteStorage);
    runtime.error = Some("Storage cleanup failed".into());
    app.cloud_prototype.production.close.deleting.insert(101);
    app.finish_closing_clouds(&egui::Context::default());
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    assert_eq!(
        app.cloud_prototype.production.runtimes[&101].error.as_deref(),
        Some("Storage cleanup failed")
    );
    assert!(app.cloud_prototype.production.close.deleting.is_empty());
}

fn add_member(app: &mut HorizonApp) -> horizon_core::PanelId {
    let workspace = app.board.workspaces.last().unwrap().id;
    let panel = app
        .board
        .create_panel(
            horizon_core::PanelOptions {
                kind: horizon_core::PanelKind::Editor,
                position: Some([14.0, 120.0]),
                size: Some([120.0, 100.0]),
                ..Default::default()
            },
            workspace,
        )
        .unwrap();
    app.cloud_prototype.groups.0[0].attach(&mut app.board, panel);
    panel
}

#[test]
fn a_closing_cloud_hides_its_panels_and_shows_them_again_if_its_deletion_fails() {
    let (temp, mut app) = test_app();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    let panel = add_member(&mut app);
    let already_hidden = add_member(&mut app);
    app.board.panel_mut(already_hidden).unwrap().visible = false;
    assert!(app.board.panel(panel).unwrap().visible);

    let (_sender, receiver) = std::sync::mpsc::channel();
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&101).unwrap();
    runtime.receiver = Some(receiver);
    runtime.stage = Some(Stage::DeleteWorker);
    app.cloud_prototype.production.close.deleting.insert(101);
    let ctx = egui::Context::default();
    app.finish_closing_clouds(&ctx);
    assert!(app.cloud_prototype.production.close.closing(101));
    assert!(
        !app.board.panel(panel).unwrap().visible,
        "the panels end with the cloud and give way to its disposal"
    );
    assert!(
        app.board.is_hidden_for_disposal(panel),
        "saved state keeps them as they were while the disposal is shown"
    );

    let runtime = app.cloud_prototype.production.runtimes.get_mut(&101).unwrap();
    runtime.receiver = None;
    runtime.error = Some("Storage cleanup failed".into());
    app.finish_closing_clouds(&ctx);
    assert!(!app.cloud_prototype.production.close.closing(101));
    assert!(
        app.board.panel(panel).unwrap().visible,
        "a deletion that could not finish leaves the cloud as it was"
    );
    assert!(!app.board.is_hidden_for_disposal(panel));
    assert!(
        !app.board.panel(already_hidden).unwrap().visible,
        "a panel that was hidden before stays hidden"
    );
}

/// A cloud with one panel whose deletion is running; keep the sender alive for as long as it runs.
fn closing_cloud_with_panel(
    app: &mut HorizonApp,
    root: &std::path::Path,
) -> (horizon_core::PanelId, std::sync::mpsc::Sender<super::super::Event>) {
    add_cloud(app, root, Some(deployment()));
    let panel = add_member(app);
    let (sender, receiver) = std::sync::mpsc::channel();
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&101).unwrap();
    runtime.receiver = Some(receiver);
    runtime.stage = Some(Stage::DeleteWorker);
    app.cloud_prototype.production.close.deleting.insert(101);
    (panel, sender)
}

#[test]
fn expanding_a_collapsed_cloud_while_it_closes_keeps_its_panels_hidden() {
    let (temp, mut app) = test_app();
    let (panel, _sender) = closing_cloud_with_panel(&mut app, temp.path());
    app.cloud_prototype.groups.0[0].set_collapsed(&mut app.board, true);
    let ctx = egui::Context::default();
    app.finish_closing_clouds(&ctx);
    assert!(!app.board.panel(panel).unwrap().visible);

    app.cloud_prototype.groups.0[0].set_collapsed(&mut app.board, false);
    assert!(
        app.board.panel(panel).unwrap().visible,
        "expanding shows its members again"
    );
    // The frame hides them again right before panels render, after the header action.
    app.hide_closing_cloud_panels();
    assert!(
        !app.board.panel(panel).unwrap().visible,
        "the disposal takes them back out of sight"
    );

    let runtime = app.cloud_prototype.production.runtimes.get_mut(&101).unwrap();
    runtime.receiver = None;
    runtime.error = Some("Storage cleanup failed".into());
    app.finish_closing_clouds(&ctx);
    assert!(
        app.board.panel(panel).unwrap().visible,
        "a failed deletion returns them"
    );
}

#[test]
fn a_cloud_collapsed_during_a_failed_close_shows_its_panels_when_it_expands() {
    let (temp, mut app) = test_app();
    let (panel, _sender) = closing_cloud_with_panel(&mut app, temp.path());
    let ctx = egui::Context::default();
    app.finish_closing_clouds(&ctx);
    assert!(!app.board.panel(panel).unwrap().visible);
    app.cloud_prototype.groups.0[0].set_collapsed(&mut app.board, true);

    let runtime = app.cloud_prototype.production.runtimes.get_mut(&101).unwrap();
    runtime.receiver = None;
    runtime.error = Some("Storage cleanup failed".into());
    app.finish_closing_clouds(&ctx);
    assert!(!app.board.panel(panel).unwrap().visible, "still collapsed");
    app.cloud_prototype.groups.0[0].set_collapsed(&mut app.board, false);
    assert!(
        app.board.panel(panel).unwrap().visible,
        "expanding must not leave the panel stranded"
    );
}

#[test]
#[cfg(unix)] // Durable cloud records require Unix directory durability.
fn a_declined_removal_returns_the_panels_of_the_cloud() {
    let (temp, mut app) = test_app();
    Store::lock(&temp.path().join("fixture"))
        .unwrap()
        .save(&deployment())
        .unwrap();
    let (panel, _sender) = closing_cloud_with_panel(&mut app, temp.path());
    let ctx = egui::Context::default();
    app.finish_closing_clouds(&ctx);
    assert!(!app.board.panel(panel).unwrap().visible);

    let runtime = app.cloud_prototype.production.runtimes.get_mut(&101).unwrap();
    runtime.receiver = None;
    runtime.stage = Some(Stage::Deleted);
    app.finish_closing_clouds(&ctx);
    assert_eq!(
        app.cloud_prototype.groups.0.len(),
        1,
        "the bound worker keeps the cloud"
    );
    assert!(app.board.panel(panel).unwrap().visible, "and its panels stay usable");
    assert!(!app.board.is_hidden_for_disposal(panel));
}

#[test]
#[cfg(unix)] // Durable cloud records require Unix directory durability.
fn a_finished_close_leaves_no_disposal_marker_behind() {
    let (temp, mut app) = test_app();
    let mut deleted = deployment();
    deleted.stage = Stage::Deleted;
    deleted.operation = CreateState::Terminated {
        worker_id: "worker".into(),
    };
    Store::lock(&temp.path().join("fixture"))
        .unwrap()
        .save(&deleted)
        .unwrap();
    let (panel, _sender) = closing_cloud_with_panel(&mut app, temp.path());
    let ctx = egui::Context::default();
    app.finish_closing_clouds(&ctx);
    assert!(app.board.is_hidden_for_disposal(panel));

    let runtime = app.cloud_prototype.production.runtimes.get_mut(&101).unwrap();
    runtime.receiver = None;
    runtime.stage = Some(Stage::Deleted);
    app.finish_closing_clouds(&ctx);
    assert!(app.cloud_prototype.groups.0.is_empty(), "the cloud closed");
    assert!(
        !app.board.is_hidden_for_disposal(panel),
        "a closed cloud's panels leave nothing behind"
    );
}
