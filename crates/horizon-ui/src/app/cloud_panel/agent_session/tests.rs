use super::*;
use horizon_core::{
    Board, PanelKind, PanelOptions, PanelState, RuntimeState, WorkspaceState,
    browser::manifest,
    browser_actor,
    cloud_panel::{CloudConfig, CloudGroup, CloudLaunch},
    cloud_runtime::state::Deployment,
};

/// A ready cloud in its own workspace, its record naming one agent session, and an agent panel
/// beside it.
fn cloud_with_agent() -> (tempfile::TempDir, HorizonApp, String) {
    let (temp, mut app) = crate::app::test_support::test_app();
    let profile = CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker\n    cpu: 4\n    memory_gb: 8\n",
    )
    .unwrap()
    .profiles["dev"]
        .clone();
    let mut group = CloudGroup::new(1, "Cloud".into(), "workspace".into(), temp.path().into(), [0.0, 0.0]);
    group.remote = Some(CloudLaunch {
        publishing_selection: None,
        deployment_started: true,
        id: "fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: profile.clone(),
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    group.panels = vec!["one".into()];
    let mut saved = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".into(),
            name: "Fixture".into(),
            panels: vec![PanelState {
                local_id: "one".into(),
                kind: PanelKind::Shell,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    saved.cloud_groups.0.push(group);
    app.board = Board::from_runtime_state(&saved).unwrap();
    app.cloud_prototype.groups = saved.cloud_groups.clone();
    app.cloud_prototype.root = Some(temp.path().into());
    let state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"fixture","repository":temp.path(),"revision":"a".repeat(40),"profile":profile,
        "stage":"Ready","operation":{"state":"bound","worker_id":"fixture"},"spec":null,
        "worker":{"id":"fixture","name":"fixture","imageName":"example/worker","desiredStatus":"RUNNING","publicIp":"127.0.0.1","portMappings":{"22":9}},
        "sessions":[{"panel_id":"panel-1","agent":"claude","tmux":"panel-1","branch":"agent/panel-1","worktree":"/workspace/agents/panel-1"}],
        "source_ready":true
    }))
    .unwrap();
    Store::lock(&temp.path().join("fixture")).unwrap().save(&state).unwrap();
    std::fs::write(
        temp.path().join("settings.json"),
        serde_json::to_vec(&serde_json::json!({
            "runpod_key_file":temp.path().join("unused-key"),"ssh_identity_file":temp.path().join("absent-identity"),
            "docker_config":temp.path().join("docker"),"registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]
        }))
        .unwrap(),
    )
    .unwrap();
    let workspace = app.board.workspace_id_by_local_id("workspace").unwrap();
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

fn request(actor: &str, call: &serde_json::Value) -> UsageRequest {
    let mut fields = serde_json::json!({
        "request_id": "cloud-agent", "actor": actor, "host_instance": manifest::host_instance(),
        "deadline_at_millis": i64::MAX, "claimed": true,
    });
    for (name, value) in call.as_object().unwrap() {
        fields[name.as_str()] = value.clone();
    }
    serde_json::from_value(fields).unwrap()
}

fn message(actor: &str, session: &str, text: &str) -> UsageRequest {
    request(
        actor,
        &serde_json::json!({"cloud_agent_message": {"cloud": "fixture", "session": session, "text": text}}),
    )
}

fn read(actor: &str, session: &str) -> UsageRequest {
    request(
        actor,
        &serde_json::json!({"cloud_agent_read": {"cloud": "fixture", "session": session}}),
    )
}

fn outcome() -> Outcome {
    Outcome {
        text: "❯ read #1774 and execute PR 1\n❯ ".into(),
        truncated: false,
        exit_status: 0,
        uid: 10001,
        duration: Duration::from_millis(1_400),
    }
}

#[test]
fn a_caller_without_its_own_agent_panel_reaches_no_route() {
    let (_temp, app, _actor) = cloud_with_agent();
    let ask = Ask::of(&message("horizon:nobody", "panel-1", "go")).unwrap();
    let refused = Route::of(
        &app,
        &message("horizon:nobody", "panel-1", "go"),
        ask.cloud(),
        ask.codes(),
    );
    assert_eq!(
        refused.unwrap_err(),
        "cloud_agent_unavailable: requires a Horizon agent panel"
    );
}

#[test]
fn the_call_needs_a_ready_cloud_and_a_session_its_record_names() {
    let (temp, app, actor) = cloud_with_agent();
    let ask = Ask::of(&message(&actor, "panel-1", "go")).unwrap();
    let route = Route::of(&app, &message(&actor, "panel-1", "go"), ask.cloud(), ask.codes()).unwrap();
    assert_eq!(route.directory, temp.path().join("fixture"));
    assert_eq!(route.ready(ask.codes()), Ok(()));

    // The record names no such session, so nothing is typed into a terminal of this cloud.
    let stranger = Ask::of(&message(&actor, "panel-2", "go")).unwrap();
    let record = Ask::record(&route).unwrap();
    assert!(agent_session::session(&record, stranger.session()).is_none());

    // A cloud that is not ready takes no write and no read.
    let saved = Store::lock(&route.directory)
        .and_then(|store| store.load())
        .unwrap()
        .unwrap();
    Store::lock(&route.directory)
        .unwrap()
        .save(&Deployment {
            stage: horizon_core::cloud_runtime::Stage::Stopped,
            ..saved
        })
        .unwrap();
    assert_eq!(route.ready(ask.codes()), Err("cloud_agent_not_ready: Stopped".into()));
    std::fs::remove_dir_all(&route.directory).unwrap();
    assert_eq!(
        route.ready(ask.codes()),
        Err("cloud_agent_not_ready: no deployment recorded".into()),
        "a missing record is a refusal, not a silent call"
    );
}

#[test]
fn a_call_that_expired_before_horizon_read_it_runs_nothing() {
    let (_temp, app, actor) = cloud_with_agent();
    let mut expired = message(&actor, "panel-1", "go");
    expired.deadline_at_millis = manifest::now_millis() - 1;
    let ask = Ask::of(&expired).unwrap();
    assert_eq!(
        Route::of(&app, &expired, ask.cloud(), ask.codes()).unwrap_err(),
        "cloud_agent_expired: the request expired before Horizon started the write; nothing ran"
    );
    let mut expired_read = read(&actor, "panel-1");
    expired_read.deadline_at_millis = manifest::now_millis() - 1;
    let ask = Ask::of(&expired_read).unwrap();
    assert_eq!(
        Route::of(&app, &expired_read, ask.cloud(), ask.codes()).unwrap_err(),
        "cloud_agent_expired: the request expired before Horizon started the read; nothing ran"
    );
}

#[test]
fn a_request_that_names_no_valid_call_is_refused_and_keeps_its_own_code() {
    let (_temp, _app, actor) = cloud_with_agent();
    let refused = Ask::of(&message(&actor, "panel-1", "go\npush")).err().unwrap();
    assert!(refused.starts_with("cloud_agent_invalid_request"), "{refused}");
    let refused = Ask::of(&request(&actor, &serde_json::json!({}))).err().unwrap();
    assert!(refused.starts_with("cloud_agent_invalid_request"), "{refused}");
    // A write and a read each answer their own field, so a caller reads the call it made.
    assert_eq!(
        Ask::of(&message(&actor, "panel-1", "go")).unwrap().codes().started,
        "the write"
    );
    assert_eq!(Ask::of(&read(&actor, "panel-1")).unwrap().codes().started, "the read");
}

#[test]
fn the_answer_carries_the_session_the_text_and_the_account() {
    let (_temp, _app, actor) = cloud_with_agent();
    let ask = Ask::of(&message(&actor, "panel-1", "read #1774 and execute PR 1")).unwrap();
    let answer = Answer {
        session: Session::new("panel-1", "claude", "/workspace/agents/panel-1".into()),
        outcome: outcome(),
        duration: Duration::from_millis(1_400),
    };
    let value = answer_value(&answer);
    assert_eq!(value["agent"], "claude");
    assert_eq!(value["worktree"], "/workspace/agents/panel-1");
    assert_eq!(value["exit_status"], 0);
    assert_eq!(value["uid"], 10001);
    assert_eq!(value["bytes"], outcome().text.len());
    assert_eq!(value["truncated"], false);
    assert_eq!(value["duration_millis"], 1_400);
    assert_eq!(ask.text_bytes(), "read #1774 and execute PR 1".len());
}

#[test]
fn the_journal_records_the_shape_of_a_write_and_never_its_text() {
    let (_temp, _app, actor) = cloud_with_agent();
    let ask = Ask::of(&message(&actor, "panel-1", "push with token ghp_secret")).unwrap();
    let record = audit(&actor, &ask, None, Duration::from_millis(2_000));
    assert_eq!(
        (record.cloud.as_str(), record.session.as_str(), record.action),
        ("fixture", "panel-1", CloudAgentAction::Message)
    );
    assert_eq!(record.outcome, CloudAgentOutcome::Completed);
    assert_eq!(record.text_bytes, "push with token ghp_secret".len() as u64);
    assert_eq!(record.duration_millis, Some(2_000));
    let encoded = serde_json::to_string(&record).unwrap();
    assert!(!encoded.contains("ghp_secret"), "{encoded}");
    assert!(!encoded.contains("push with"), "{encoded}");
    assert!(encoded.contains(&actor), "{encoded}");

    let failed = audit(
        &actor,
        &Ask::of(&read(&actor, "panel-1")).unwrap(),
        Some(&"cloud_agent_not_running: x".to_owned()),
        Duration::from_millis(1),
    );
    assert_eq!(failed.outcome, CloudAgentOutcome::Failed);
    assert_eq!(failed.action, CloudAgentAction::Read);
    assert_eq!(failed.text_bytes, 0);
    assert_eq!(failed.code.as_deref(), Some("cloud_agent_not_running: x"));
}
