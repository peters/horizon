use super::super::super::park::tests::{member, ready_cloud};
use super::super::super::{Runtime, Stage};
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
    assert!(clouds[0].get("idle").is_none(), "nothing offers a bulk stop: {answer}");

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
        .answer_cloud_list(&request(&stranger, "park", Some("fixture")), &ctx)
        .unwrap_err();
    assert!(error.starts_with("cloud_list_unknown_cloud"), "{error}");
}

#[test]
fn each_listed_hourly_rate_names_its_currency() {
    let (_temp, mut app, actor) = cloud_with_agent();
    let ctx = egui::Context::default();
    let listed = |app: &mut HorizonApp| {
        let answer = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
        (
            answer["clouds"][0]["hourly_rate"].clone(),
            answer["clouds"][0]["currency"].clone(),
        )
    };
    let state = runtime(&mut app).state.as_mut().unwrap();
    state.worker.as_mut().unwrap().cost_per_hr = None;
    assert_eq!(listed(&mut app), (json!(null), json!(null)), "no rate, no currency");
    let state = runtime(&mut app).state.as_mut().unwrap();
    state.worker.as_mut().unwrap().cost_per_hr = Some(0.69);
    assert_eq!(listed(&mut app), (json!(0.69), json!("USD")));
    let state = runtime(&mut app).state.as_mut().unwrap();
    state.profile.provider = "hetzner".into();
    state.worker.as_mut().unwrap().cost_per_hr = Some(0.024);
    assert_eq!(listed(&mut app), (json!(0.024), json!("EUR")));
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
    runtime(&mut app).pending_session_attachments.insert("one".into());
    let error = app.answer_cloud_list(&park, &ctx).unwrap_err();
    assert!(
        error.starts_with("cloud_list_not_ready"),
        "a session still attaches: {error}"
    );

    runtime(&mut app).pending_session_attachments.clear();
    app.board.focus(member(&app, "one"));
    let error = app.answer_cloud_list(&park, &ctx).unwrap_err();
    assert!(error.starts_with("cloud_list_in_view"), "{error}");

    app.board.focused = None;
    // A request that meets another operation first is dropped, not kept for later.
    assert_eq!(app.answer_cloud_list(&park, &ctx).unwrap()["park"], "parking");
    runtime(&mut app).stage = Some(Stage::Stopping);
    app.sync_cloud_parking();
    runtime(&mut app).stage = Some(Stage::Ready);
    app.sync_cloud_parking();
    assert!(
        !runtime(&mut app).parking.is_parked(),
        "the old request does not park it"
    );

    assert_eq!(app.answer_cloud_list(&park, &ctx).unwrap()["park"], "parking");
    app.sync_cloud_parking();
    assert!(runtime(&mut app).parking.is_parked());
    assert_eq!(app.answer_cloud_list(&park, &ctx).unwrap()["park"], "parked");
    // A parked cloud that comes into view attaches again, so a park is refused.
    app.board.focus(member(&app, "one"));
    let error = app.answer_cloud_list(&park, &ctx).unwrap_err();
    assert!(error.starts_with("cloud_list_in_view"), "{error}");
}

#[test]
fn a_stop_from_an_older_agent_tool_is_refused() {
    let (_temp, mut app, actor) = cloud_with_agent();
    let ctx = egui::Context::default();
    let error = app
        .answer_cloud_list(&request(&actor, "stop", Some("fixture")), &ctx)
        .unwrap_err();
    assert!(
        error.starts_with("cloud_list_invalid_request: stop was removed"),
        "{error}"
    );
    assert_eq!(runtime(&mut app).operation, None);
}

#[test]
fn an_expired_request_or_an_ambiguous_cloud_changes_nothing() {
    let (_temp, mut app, actor) = cloud_with_agent();
    let ctx = egui::Context::default();
    let mut park = request(&actor, "park", Some("fixture"));
    park.deadline_at_millis = manifest::now_millis() - 1;
    let error = app.answer_cloud_list(&park, &ctx).unwrap_err();
    assert!(error.starts_with("cloud_list_expired"), "{error}");
    let mut list = request(&actor, "list", None);
    list.deadline_at_millis = manifest::now_millis() - 1;
    assert!(app.answer_cloud_list(&list, &ctx).is_ok(), "a read is still answered");

    let mut copy = app.cloud_prototype.groups.0[0].clone();
    copy.issue = 2;
    copy.workspace = "elsewhere".into();
    app.cloud_prototype.groups.0.push(copy);
    let error = app
        .answer_cloud_list(&request(&actor, "park", Some("fixture")), &ctx)
        .unwrap_err();
    assert!(error.starts_with("cloud_list_ambiguous_cloud"), "{error}");
    assert!(!runtime(&mut app).parking.is_parked());
}

#[test]
fn publication_status_uses_only_the_clouds_retained_runtime_metadata() {
    use horizon_core::cloud_runtime::{github::publish::publication::Account, publishing::Publication};
    let (_temp, mut app, actor) = cloud_with_agent();
    let state = runtime(&mut app).state.as_mut().unwrap();
    let publication = Publication {
        profile: "cpu".into(),
        requested_image: String::new(),
        destination: serde_json::from_value(json!({
            "repository":"ghcr.io/synthetic-user/fixture",
            "publisher":{"kind":"git_hub","login":"synthetic-user"}
        }))
        .unwrap(),
        account: Some(Account {
            id: 17,
            login: "synthetic-user".into(),
        }),
    };
    let expected = serde_json::to_value(&publication).unwrap();
    let destination = publication.destination.clone();
    state.profile.image.clone_from(&destination.repository);
    state.profile.build = Some(serde_json::from_value(json!({"context":".","dockerfile":"Dockerfile"})).unwrap());
    state.version = 5;
    if let Some(spec) = &mut state.spec {
        spec.profile = state.profile.clone();
    }
    state.publication = Some(Box::new(publication));
    let ctx = egui::Context::default();
    let listed = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
    assert_eq!(listed["clouds"][0]["worker_image"], destination.repository);
    assert_eq!(listed["clouds"][0]["image_publication"], expected);
    assert_eq!(
        listed["clouds"][0]["image_publication"]["destination"]["repository"],
        destination.repository
    );
    let encoded = serde_json::to_string(&listed).unwrap();
    assert!(!encoded.contains("secret_file"));
    assert!(!encoded.contains("access_token"));
    runtime(&mut app).state.as_mut().unwrap().cloud_id = "different-cloud".into();
    let listed = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
    assert!(listed["clouds"][0]["worker_image"].is_null());
    assert!(listed["clouds"][0]["image_publication"].is_null());
    runtime(&mut app).state = None;
    let listed = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
    assert!(listed["clouds"][0]["image_publication"].is_null());
}

#[test]
fn an_unresolved_automatic_image_is_null_and_a_retained_registry_grant_stays_opaque() {
    let (_temp, mut app, actor) = cloud_with_agent();
    let state = runtime(&mut app).state.as_mut().unwrap();
    state.profile.image.clear();
    state.profile.build = Some(serde_json::from_value(json!({"context":".","dockerfile":"Dockerfile"})).unwrap());
    if let Some(spec) = &mut state.spec {
        spec.profile = state.profile.clone();
    }
    let ctx = egui::Context::default();
    let listed = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
    assert!(listed["clouds"][0]["worker_image"].is_null());
    assert!(listed["clouds"][0]["image_publication"].is_null());

    let binding = "a".repeat(64);
    let expected = json!({
        "profile":"cpu", "requested_image":"",
        "destination":{"repository":"registry.example/team/worker",
            "publisher":{"kind":"registry","binding":binding}}
    });
    let state = runtime(&mut app).state.as_mut().unwrap();
    state.version = 5;
    state.profile.image = "registry.example/team/worker".into();
    if let Some(spec) = &mut state.spec {
        spec.profile = state.profile.clone();
    }
    state.publication = Some(Box::new(serde_json::from_value(expected.clone()).unwrap()));
    let listed = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
    assert_eq!(listed["clouds"][0]["worker_image"], "registry.example/team/worker");
    assert_eq!(listed["clouds"][0]["image_publication"], expected);
    let encoded = serde_json::to_string(&listed).unwrap();
    assert!(!encoded.contains("secret_file"));
    assert!(!encoded.contains("access_token"));
}

#[test]
fn the_list_names_the_sessions_a_caller_can_address() {
    let (_temp, mut app, actor) = cloud_with_agent();
    let ctx = egui::Context::default();
    let state = runtime(&mut app).state.as_mut().unwrap();
    state.sessions = vec![horizon_core::cloud_runtime::state::Session::new(
        "panel-1",
        "claude",
        "/workspace/agents/panel-1".into(),
    )];
    let listed = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
    assert_eq!(
        listed["clouds"][0]["sessions"],
        json!([{
            "panel_id": "panel-1",
            "agent": "claude",
            "branch": "",
            "worktree": "/workspace/agents/panel-1",
        }])
    );
    let encoded = serde_json::to_string(&listed).unwrap();
    assert!(!encoded.contains("secret") && !encoded.contains("token"), "{encoded}");
}

#[test]
fn a_session_this_window_recorded_is_listed_without_reading_the_record() {
    // A live worker saves its session to the record, which a worker thread writes; the
    // deployment thread's snapshot holds none, so a list answer taken from it named no
    // session and a caller had no panel ID to address. The window keeps the session it
    // added, and the answer reads that instead of the record: a list request must not read
    // a file on the UI thread.
    let (_temp, mut app, actor) = cloud_with_agent();
    let ctx = egui::Context::default();
    let listed = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
    assert_eq!(listed["clouds"][0]["sessions"], json!([]), "nothing is recorded yet");

    let session =
        horizon_core::cloud_runtime::state::Session::new("panel-live", "claude", "/workspace/checkout".into());
    runtime(&mut app).sessions.remember(&session);

    let listed = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
    assert_eq!(
        listed["clouds"][0]["sessions"],
        json!([{
            "panel_id": "panel-live",
            "agent": "claude",
            "branch": "",
            "worktree": "/workspace/checkout",
        }]),
        "the session this window recorded is named: {listed}"
    );
}

#[test]
fn a_restored_session_and_one_added_after_it_are_both_listed_once() {
    // A cloud restored after a restart carries the session its saved record held. A panel
    // that attaches later adds a second one, and the deployment thread's snapshot still
    // names only the first. Both are sessions the worker runs, so both are listed: a caller
    // needs the panel ID of the one it wants to address.
    let (_temp, mut app, actor) = cloud_with_agent();
    let ctx = egui::Context::default();
    runtime(&mut app).state.as_mut().unwrap().sessions = vec![horizon_core::cloud_runtime::state::Session::new(
        "panel-restored",
        "claude",
        "/workspace/agents/panel-restored".into(),
    )];

    let listed = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
    assert_eq!(
        listed["clouds"][0]["sessions"],
        json!([{
            "panel_id": "panel-restored",
            "agent": "claude",
            "branch": "",
            "worktree": "/workspace/agents/panel-restored",
        }]),
        "the restored session alone is listed before a panel attaches: {listed}"
    );

    let session =
        horizon_core::cloud_runtime::state::Session::new("panel-live", "claude", "/workspace/checkout".into());
    runtime(&mut app).sessions.remember(&session);

    let listed = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
    let sessions = listed["clouds"][0]["sessions"].as_array().unwrap().clone();
    assert_eq!(
        sessions
            .iter()
            .map(|session| session["panel_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["panel-restored", "panel-live"],
        "both sessions are named, in the order the state then this window holds them: {listed}"
    );
}

#[test]
fn a_session_both_sources_hold_is_listed_once() {
    // The window remembers the session it added, and a later snapshot of the record carries
    // it too. It is one session on the worker, so it is one entry: a caller that counted
    // entries as sessions would address the same terminal twice.
    let (_temp, mut app, actor) = cloud_with_agent();
    let ctx = egui::Context::default();
    let session =
        horizon_core::cloud_runtime::state::Session::new("panel-live", "claude", "/workspace/checkout".into());
    runtime(&mut app).sessions.remember(&session);
    runtime(&mut app).state.as_mut().unwrap().sessions = vec![session];

    let listed = app.answer_cloud_list(&request(&actor, "list", None), &ctx).unwrap();
    assert_eq!(
        listed["clouds"][0]["sessions"],
        json!([{
            "panel_id": "panel-live",
            "agent": "claude",
            "branch": "",
            "worktree": "/workspace/checkout",
        }]),
        "one session is one entry: {listed}"
    );
}
