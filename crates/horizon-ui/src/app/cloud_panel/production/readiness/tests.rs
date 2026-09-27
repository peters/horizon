use super::*;
use crate::app::cloud_panel::production::Runtime;
use crate::test_egui::DiscardTextures;
use horizon_core::cloud_panel::CloudLaunch;

fn group() -> CloudGroup {
    let mut group = CloudGroup::new(
        101,
        "Fixture".into(),
        "workspace".into(),
        "/synthetic".into(),
        [0.0, 0.0],
    );
    group.remote = Some(
        serde_json::from_value::<CloudLaunch>(serde_json::json!({
            "id":"fixture", "revision":"a", "profile_name":"development",
            "profile":{"provider":"runpod","image":"example.invalid/worker","cpu":4,"memory_gb":8}
        }))
        .unwrap(),
    );
    group
}

fn runtime(stage: Stage) -> Runtime {
    Runtime {
        stage: Some(stage),
        state: Some(
            serde_json::from_value(serde_json::json!({
                "version":1, "cloud_id":"fixture", "repository":"/synthetic", "revision":"a",
                "profile":{"provider":"runpod","image":"example.invalid/worker","cpu":4,"memory_gb":8},
                "stage":stage,"operation":{"state":"bound","worker_id":"worker1"},
                "spec":null,"worker":null,"sessions":[]
            }))
            .unwrap(),
        ),
        ..Runtime::default()
    }
}

#[test]
fn panel_creation_waits_for_a_ready_runtime_and_current_state() {
    let group = group();
    let mut production = Production::default();
    assert!(!production.accepts_panels(&group));
    production.runtimes.insert(group.issue, runtime(Stage::Provision));
    assert!(!production.accepts_panels(&group));
    production.runtimes.insert(group.issue, runtime(Stage::Ready));
    assert!(production.accepts_panels(&group));
    let current = production.runtimes.get_mut(&group.issue).unwrap();
    current.stage = Some(Stage::DeleteWorker);
    assert!(
        !production.accepts_panels(&group),
        "an old Ready snapshot cannot reopen the picker"
    );
    let current = production.runtimes.get_mut(&group.issue).unwrap();
    current.stage = Some(Stage::Ready);
    current.state_unavailable = true;
    assert!(!production.accepts_panels(&group));
    let current = production.runtimes.get_mut(&group.issue).unwrap();
    current.state_unavailable = false;
    current.state = None;
    assert!(!production.accepts_panels(&group));
}

#[test]
fn local_cloud_fixtures_do_not_require_a_remote_runtime() {
    let mut group = group();
    group.remote = None;
    assert!(Production::default().accepts_panels(&group));
}

#[test]
fn an_open_picker_closes_when_its_cloud_stops_being_ready() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    let workspace = app.board.ensure_workspace();
    let mut group = group();
    group
        .workspace
        .clone_from(&app.board.workspace(workspace).unwrap().local_id);
    let position = group.position;
    app.cloud_prototype
        .production
        .runtimes
        .insert(group.issue, runtime(Stage::Provision));
    app.cloud_prototype.groups.0.push(group);
    app.pending_preset_pick = Some((Some(workspace), position, std::time::Instant::now()));
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| app.render_preset_picker(ui.ctx()))
        .discard_textures();
    assert!(app.pending_preset_pick.is_none());
    assert!(app.preset_target_accepts_cloud_panels(None, position));
}
