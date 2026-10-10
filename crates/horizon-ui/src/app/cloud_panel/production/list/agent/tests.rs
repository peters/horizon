use super::super::super::park::tests::{member, ready_cloud};
use super::super::super::{Runtime, Stage, lifecycle::Action};
use super::*;
use horizon_core::{PanelKind, PanelOptions, browser_actor};

fn runtime(app: &mut HorizonApp) -> &mut Runtime {
    app.cloud_prototype.production.runtimes.get_mut(&1).unwrap()
}

/// A ready cloud with an agent panel beside it in its workspace, and that agent's actor.
fn cloud_with_agent() -> (tempfile::TempDir, HorizonApp, String) {
    let (temp, mut app) = ready_cloud();
    // The sender is dropped: the watch reports nothing in these tests.
    runtime(&mut app).receiver = Some(std::sync::mpsc::channel().1);
    let workspace = app.board.panel(member(&app, "one")).unwrap().workspace_id;
    let agent = app
        .board
        .create_panel(
            PanelOptions {
                command: Some("/bin/sh".into()),
                args: vec!["-c".into(), "exit 0".into()],
                kind: PanelKind::Codex,
                ..PanelOptions::default()
            },
            workspace,
        )
        .unwrap();
    app.board.focused = None;
    let actor = browser_actor(&app.board.panel(agent).unwrap().local_id);
    (temp, app, actor)
}

fn request(actor: &str, operation: &str, cloud: Option<&str>) -> UsageRequest {
    let mut list = json!({ "operation": operation });
    if let Some(cloud) = cloud {
        list["cloud"] = cloud.into();
    }
    serde_json::from_value(json!({
        "request_id": "cloud-list", "actor": actor, "host_instance": manifest::host_instance(),
        "deadline_at_millis": i64::MAX, "claimed": true, "cloud_list": list,
    }))
    .unwrap()
}

#[test]
fn an_agent_lists_only_the_clouds_of_its_workspace() {
    let (_temp, mut app, actor) = cloud_with_agent();
    let ctx = egui::Context::default();
    let answer = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
    let clouds = answer["clouds"].as_array().unwrap();
    assert_eq!(clouds.len(), 1, "{answer}");
    assert_eq!(clouds[0]["cloud"], "fixture");
    assert_eq!(clouds[0]["name"], "Cloud");
    assert_eq!(clouds[0]["group"], "cloud");
    assert_eq!(clouds[0]["idle"], true);

    let error = app
        .answer_cloud_list(&request("horizon:nobody", "list", None), &ctx)
        .unwrap_err();
    assert!(error.starts_with("cloud_list_unavailable"), "{error}");
    // An agent in another workspace does not see the cloud.
    let other = app.board.create_workspace("other");
    let stranger = app
        .board
        .create_panel(
            PanelOptions {
                command: Some("/bin/sh".into()),
                kind: PanelKind::Codex,
                ..PanelOptions::default()
            },
            other,
        )
        .unwrap();
    let stranger = browser_actor(&app.board.panel(stranger).unwrap().local_id);
    let answer = app.answer_cloud_list(&request(&stranger, "list", None), &ctx).unwrap();
    assert_eq!(answer["clouds"], json!([]));
    let error = app
        .answer_cloud_list(&request(&stranger, "stop", Some("fixture")), &ctx)
        .unwrap_err();
    assert!(error.starts_with("cloud_list_unknown_cloud"), "{error}");
}

#[test]
fn attach_brings_the_cloud_into_view_with_its_first_member_focused() {
    let (_temp, mut app, actor) = cloud_with_agent();
    let ctx = egui::Context::default();
    let answer = app
        .answer_cloud_list(&request(&actor, "attach", Some("fixture")), &ctx)
        .unwrap();
    assert_eq!(answer["attach"], "in_view");
    assert_eq!(app.board.focused, Some(member(&app, "one")));
}

#[test]
fn park_parks_an_attached_cloud_only_while_it_is_out_of_view() {
    let (_temp, mut app, actor) = cloud_with_agent();
    let ctx = egui::Context::default();
    let park = request(&actor, "park", Some("fixture"));
    let error = app.answer_cloud_list(&park, &ctx).unwrap_err();
    assert!(error.starts_with("cloud_list_not_ready"), "{error}");

    runtime(&mut app).parking.attach_for_test();
    let error = app.answer_cloud_list(&park, &ctx).unwrap_err();
    assert!(
        error.starts_with("cloud_list_not_ready"),
        "terminals still attach: {error}"
    );

    runtime(&mut app).needs_attach = false;
    runtime(&mut app).stage = Some(Stage::Stopping);
    let error = app.answer_cloud_list(&park, &ctx).unwrap_err();
    assert!(
        error.starts_with("cloud_list_not_ready"),
        "another operation runs: {error}"
    );

    runtime(&mut app).stage = Some(Stage::Ready);
    app.board.focus(member(&app, "one"));
    let error = app.answer_cloud_list(&park, &ctx).unwrap_err();
    assert!(error.starts_with("cloud_list_in_view"), "{error}");

    app.board.focused = None;
    assert_eq!(app.answer_cloud_list(&park, &ctx).unwrap()["park"], "parking");
    app.sync_cloud_parking();
    assert!(runtime(&mut app).parking.is_parked());
    assert_eq!(app.answer_cloud_list(&park, &ctx).unwrap()["park"], "parked");
}

#[test]
fn stop_stops_only_an_idle_cloud() {
    let (_temp, mut app, actor) = cloud_with_agent();
    let ctx = egui::Context::default();
    let stop = request(&actor, "stop", Some("fixture"));
    runtime(&mut app).stage = Some(Stage::Provision);
    let error = app.answer_cloud_list(&stop, &ctx).unwrap_err();
    assert!(error.starts_with("cloud_list_not_idle"), "{error}");
    assert_eq!(runtime(&mut app).operation, None);

    runtime(&mut app).stage = Some(Stage::Ready);
    assert_eq!(app.answer_cloud_list(&stop, &ctx).unwrap()["stop"], "stopping");
    assert_eq!(runtime(&mut app).operation, Some(Action::Stop));
}

#[test]
fn an_expired_request_or_an_ambiguous_cloud_changes_nothing() {
    let (_temp, mut app, actor) = cloud_with_agent();
    let ctx = egui::Context::default();
    let mut stop = request(&actor, "stop", Some("fixture"));
    stop.deadline_at_millis = manifest::now_millis() - 1;
    let error = app.answer_cloud_list(&stop, &ctx).unwrap_err();
    assert!(error.starts_with("cloud_list_expired"), "{error}");
    let mut list = request(&actor, "list", None);
    list.deadline_at_millis = manifest::now_millis() - 1;
    assert!(app.answer_cloud_list(&list, &ctx).is_ok(), "a read is still answered");

    let mut copy = app.cloud_prototype.groups.0[0].clone();
    copy.issue = 2;
    copy.workspace = "elsewhere".into();
    app.cloud_prototype.groups.0.push(copy);
    let error = app
        .answer_cloud_list(&request(&actor, "stop", Some("fixture")), &ctx)
        .unwrap_err();
    assert!(error.starts_with("cloud_list_ambiguous_cloud"), "{error}");
    assert_eq!(runtime(&mut app).operation, None);
}
