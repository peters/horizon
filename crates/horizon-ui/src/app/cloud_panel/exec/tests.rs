use super::*;
use horizon_core::{
    Board, PanelKind, PanelOptions, PanelState, RuntimeState, WorkspaceState, browser_actor,
    cloud_panel::{CloudConfig, CloudGroup, CloudLaunch},
    cloud_runtime::state::Deployment,
};

/// A ready cloud in its own workspace, its record on disk, and an agent panel beside it.
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
        "sessions":[],"source_ready":true
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

fn request(actor: &str, cloud: &str, command: &str) -> UsageRequest {
    serde_json::from_value(serde_json::json!({
        "request_id": "cloud-exec", "actor": actor, "host_instance": manifest::host_instance(),
        "deadline_at_millis": i64::MAX, "claimed": true,
        "cloud_exec": {"cloud": cloud, "command": command},
    }))
    .unwrap()
}

fn worker_answer() -> cloud_exec::Answer {
    cloud_exec::Answer {
        exit_status: 1,
        uid: cloud_exec::AGENT_UID,
        stdout: "partial\n".into(),
        stderr: "denied\n".into(),
        duration: Duration::from_millis(1_500),
    }
}

#[test]
fn a_caller_without_its_own_agent_panel_reaches_no_route() {
    let (_temp, mut app, actor) = cloud_with_agent();
    let refused = app.cloud_exec_route(&request("horizon:nobody", "fixture", "git status"));
    assert!(
        refused.unwrap_err().starts_with("cloud_exec_unavailable"),
        "a foreign actor"
    );
    // An agent panel of another workspace may not command this cloud.
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
    let refused = app.cloud_exec_route(&request(&stranger, "fixture", "git status"));
    assert!(
        refused.unwrap_err().starts_with("cloud_exec_unknown_cloud"),
        "a cloud of another workspace"
    );
    let refused = app.cloud_exec_route(&request(&actor, "other-cloud", "git status"));
    assert!(refused.unwrap_err().starts_with("cloud_exec_unknown_cloud"));
}

#[test]
fn the_route_is_the_clouds_own_record_and_its_settings() {
    let (temp, app, actor) = cloud_with_agent();
    let route = app.cloud_exec_route(&request(&actor, "fixture", "git status")).unwrap();
    assert_eq!(route.directory, temp.path().join("fixture"));
    assert_eq!(route.settings, temp.path().join("settings.json"));
    assert!(route.directory.is_dir() && route.settings.is_file());
}

#[test]
fn a_cloud_that_is_not_ready_names_the_stage_that_stops_it() {
    let (temp, _app, _actor) = cloud_with_agent();
    let route = Route {
        directory: temp.path().join("fixture"),
        settings: temp.path().join("settings.json"),
        placement: Placement::default(),
    };
    assert_eq!(ready(&route), Ok(()));
    for (stage, label) in [(Stage::Stopped, "Stopped"), (Stage::Provision, "Provision worker")] {
        let saved = Store::lock(&route.directory)
            .and_then(|store| store.load())
            .unwrap()
            .unwrap();
        let saved = Deployment { stage, ..saved };
        Store::lock(&route.directory).unwrap().save(&saved).unwrap();
        assert_eq!(ready(&route), Err(format!("cloud_exec_not_ready: {label}")));
    }
    std::fs::remove_dir_all(&route.directory).unwrap();
    assert_eq!(
        ready(&route),
        Err("cloud_exec_not_ready: no deployment recorded".into()),
        "a missing record is a refusal, not a silent run"
    );
}

#[test]
fn a_command_that_switches_user_is_refused_rather_than_sanitized() {
    let (_temp, _app, actor) = cloud_with_agent();
    for refused in [
        "sudo -i",
        "su -",
        "/usr/bin/setpriv --reuid=0 -- true",
        "git status; runuser -u root -- true",
    ] {
        assert_eq!(
            HorizonApp::cloud_exec_command(&request(&actor, "fixture", refused)).unwrap_err(),
            "cloud_exec_refused: user switch",
            "{refused}"
        );
    }
    // A command that runs as the agent user keeps its exact text and its bound.
    let command = HorizonApp::cloud_exec_command(&request(&actor, "fixture", "git commit -am 'work'")).unwrap();
    assert_eq!(command.text, "git commit -am 'work'");
    assert_eq!(command.timeout, Duration::from_secs(30));
}

#[test]
fn an_empty_command_or_an_expired_request_never_reaches_a_worker() {
    let (_temp, app, actor) = cloud_with_agent();
    assert!(
        HorizonApp::cloud_exec_command(&request(&actor, "fixture", "   "))
            .unwrap_err()
            .starts_with("cloud_exec_invalid_request")
    );
    let mut expired = request(&actor, "fixture", "git status");
    expired.deadline_at_millis = manifest::now_millis() - 1;
    assert!(
        app.cloud_exec_route(&expired)
            .unwrap_err()
            .starts_with("cloud_exec_expired"),
        "a caller that stopped waiting runs nothing"
    );
}

#[test]
fn the_answer_carries_the_exit_status_both_streams_the_account_and_the_duration() {
    // The shape the agent reads, as the host builds it from the worker's answer.
    assert_eq!(
        answer_value(&worker_answer()),
        json!({
            "exit_status": 1,
            "uid": 10001,
            "stdout": "partial\n",
            "stderr": "denied\n",
            "duration_millis": 1_500,
        })
    );
    // A non-zero exit status is a result, not a tool error.
    assert_eq!(Outcome::Ran(worker_answer()).error(), None);
}

#[test]
fn every_failure_the_worker_proves_is_an_error_the_caller_can_read() {
    let failures = [
        (Failure::TimedOut, "cloud_exec_timed_out"),
        (Failure::Transport, "cloud_exec_failed: transport"),
        (Failure::Contract, "cloud_exec_failed: worker contract"),
        (Failure::OutputBound, "cloud_exec_failed: output exceeded its bound"),
        (Failure::NoIsolation, "cloud_exec_refused: no agent isolation"),
        (Failure::NotRoot, "cloud_exec_refused: not root"),
        (Failure::Uid(1000), "cloud_exec_refused: uid 1000"),
    ];
    for (failure, expected) in failures {
        let outcome = match &failure {
            Failure::TimedOut => Outcome::TimedOut,
            failure => Outcome::Failed(failure.to_string()),
        };
        assert_eq!(outcome.error().as_deref(), Some(expected), "{failure:?}");
    }
}

#[test]
fn the_journal_records_the_shape_of_a_command_and_never_its_text() {
    let (_temp, _app, actor) = cloud_with_agent();
    let request = request(&actor, "fixture", "gh pr merge 12 --body 'token ghp_secret'");
    let exec = request.cloud_exec.clone().unwrap();
    let record = audit(
        &request.actor,
        &exec,
        &Outcome::Ran(worker_answer()),
        Duration::from_millis(2_000),
    );
    assert_eq!(
        (record.cloud.as_str(), record.outcome, record.exit_status, record.uid),
        ("fixture", CloudExecOutcome::Completed, Some(1), Some(10001))
    );
    assert_eq!(record.duration_millis, Some(2_000));
    assert_eq!(record.command_bytes, exec.command.len() as u64);
    assert_eq!(
        record.command_sha256,
        manifest::provider_usage::command_sha256(&exec.command)
    );
    let encoded = serde_json::to_string(&record).unwrap();
    assert!(!encoded.contains("ghp_secret"), "{encoded}");
    assert!(!encoded.contains("gh pr merge"), "{encoded}");
    assert!(encoded.contains(&request.actor), "{encoded}");

    let timed_out = audit(&request.actor, &exec, &Outcome::TimedOut, Duration::from_millis(1));
    assert_eq!(timed_out.outcome, CloudExecOutcome::TimedOut);
    assert_eq!(timed_out.code.as_deref(), Some("cloud_exec_timed_out"));
    assert_eq!((timed_out.exit_status, timed_out.uid), (None, None));
}
