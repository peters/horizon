use super::*;
use horizon_core::{
    PanelKind, PanelOptions, browser_actor,
    cloud_panel::{CloudConfig, CloudGroup, CloudLaunch},
    cloud_runtime::{
        companions::{Catalog, Companion, Declaration, Row, Scope, Snapshot, Status, Target, intent::Intent},
        state::Deployment,
    },
};

fn cloud(issue: u32, id: &str, workspace: &str) -> CloudGroup {
    let config = CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 8\n    memory_gb: 32\n").unwrap();
    let mut group = CloudGroup::new(issue, id.into(), workspace.into(), "/synthetic".into(), [0.0, 0.0]);
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: id.into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: config.profiles["dev"].clone(),
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    group
}

fn operation(action: intent::Action, state: intent::State, phase: Phase) -> Operation {
    Operation {
        intent: Intent {
            operation_id: OperationId::generate(),
            action,
            target_cloud_id: "target".into(),
            state,
        },
        phase,
    }
}

fn deployment() -> Deployment {
    serde_json::from_value(json!({
        "version":1,"cloud_id":"target","repository":"/synthetic","revision":"a".repeat(40),
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
        "stage":"Stopped","operation":{"state":"bound","worker_id":"worker1"},"spec":null,"sessions":[],
        "source_ready":false,"stop_requested":true
    }))
    .unwrap()
}

fn snapshot() -> Snapshot {
    let companion = Companion {
        alias: "consumer".into(),
        repository: "example/consumer".into(),
        profile: "dev".into(),
        target_cloud_id: Some("target".into()),
        selected: true,
        status: Status::Stopped,
        access: None,
    };
    Snapshot {
        catalog: Catalog {
            version: 1,
            source_cloud_id: "source".into(),
            observed_at: 0,
            companions: vec![companion.clone()],
        },
        rows: vec![Row {
            companion,
            candidates: Vec::new(),
            error: None,
        }],
        publication_error: None,
        notice: None,
    }
}

#[test]
#[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
fn agents_reach_only_the_clouds_of_their_own_workspace() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("agents");
    let local = app.board.workspace(workspace).unwrap().local_id.clone();
    let (command, args) = if cfg!(windows) {
        ("cmd.exe", vec!["/C".into(), "exit 0".into()])
    } else {
        ("/bin/sh", vec!["-c".into(), "exit 0".into()])
    };
    let agent = app
        .board
        .create_panel(
            PanelOptions {
                command: Some(command.into()),
                args,
                kind: PanelKind::Codex,
                ..PanelOptions::default()
            },
            workspace,
        )
        .unwrap();
    let actor = browser_actor(&app.board.panel(agent).unwrap().local_id);
    app.cloud_prototype.groups = CloudGroups(vec![cloud(1, "source", &local), cloud(2, "elsewhere", "other")]);
    let state = &mut app.cloud_prototype.production.companions;
    state.sync(Some("session"), &app.cloud_prototype.groups);
    state.entries.get_mut("source").unwrap().snapshot = Some(snapshot());
    let listing = app.companion_listing(&local);
    assert_eq!(listing["clouds"].as_array().unwrap().len(), 1);
    assert_eq!(listing["clouds"][0]["cloud"], "source");
    assert_eq!(listing["clouds"][0]["companions"][0]["alias"], "consumer");
    assert_eq!(listing["clouds"][0]["companions"][0]["status"], "stopped");

    let request = |actor: &str, cloud: &str| -> UsageRequest {
        serde_json::from_value(json!({
            "request_id": "companion", "actor": actor, "host_instance": manifest::host_instance(),
            "deadline_at_millis": i64::MAX, "claimed": true,
            "cloud_companion": {"action": "ensure_ready", "cloud": cloud, "alias": "consumer",
                "operation_id": OperationId::generate()}
        }))
        .unwrap()
    };
    let ctx = egui::Context::default();
    let error = app
        .start_companion_request(&request("horizon:nobody", "source"), &ctx)
        .unwrap_err();
    assert!(error.starts_with("cloud_companion_unavailable"), "{error}");
    let error = app
        .start_companion_request(&request(&actor, "elsewhere"), &ctx)
        .unwrap_err();
    assert!(error.starts_with("cloud_companion_unknown_cloud"), "{error}");
    // Nothing was held or started for a refused request.
    let agent = &app.cloud_prototype.production.companions.agent;
    assert!(!agent.holds("source") && !agent.holds("elsewhere"));
}

#[test]
fn a_held_source_skips_its_periodic_refresh_until_every_request_releases_it() {
    let temp = tempfile::tempdir().unwrap();
    let groups = CloudGroups(vec![cloud(1, "source", "workspace")]);
    let mut state = super::super::State::default();
    state.sync(Some("session"), &groups);
    state.agent.hold("source");
    state.agent.hold("source");
    let ctx = egui::Context::default();
    state.tick(temp.path(), &groups, &ctx);
    assert!(state.entries["source"].job.is_none());
    // An owner's uncheck still runs, so it can withdraw a creation before allocation.
    state
        .entries
        .get_mut("source")
        .unwrap()
        .clearing
        .insert("consumer".into());
    state.tick(temp.path(), &groups, &ctx);
    assert!(state.entries["source"].job.is_some());
    state.entries.get_mut("source").unwrap().job = None;
    state.entries.get_mut("source").unwrap().clearing.clear();
    state.agent.release("source");
    assert!(state.agent.holds("source"));
    state.agent.release("source");
    assert!(!state.agent.holds("source"));
    state.tick(temp.path(), &groups, &ctx);
    assert!(state.entries["source"].job.is_some());
}

#[test]
fn finished_operations_leave_the_card_where_the_cloud_is() {
    use intent::{Action, State};
    let ready = operation(Action::EnsureReady, State::Succeeded, Phase::Ready);
    // A card that stayed connected already shows the ready cloud.
    assert!(finish(Ok(ready.clone()), Some(deployment()), true).is_empty());
    // Otherwise the card connects it the way its own Resume does.
    assert!(matches!(
        finish(Ok(ready), Some(deployment()), false).as_slice(),
        [Event::Resumed]
    ));
    let stopped = operation(Action::Stop, State::Succeeded, Phase::Stopped);
    assert!(matches!(
        finish(Ok(stopped.clone()), Some(deployment()), false).as_slice(),
        [Event::Stopped(_)]
    ));
    assert!(matches!(
        finish(Ok(stopped), None, false).as_slice(),
        [Event::Failed(..)]
    ));
    let uncertain = operation(Action::EnsureReady, State::Uncertain, Phase::ReconcileRequired);
    let events = finish(Ok(uncertain), Some(deployment()), false);
    assert!(matches!(events.as_slice(), [Event::Snapshot(_), Event::Failed(..)]));
    let events = finish(Err(cloud_runtime::Error::Busy), None, false);
    assert!(matches!(events.as_slice(), [Event::Failed(..)]));
}

#[test]
fn settled_and_confirmation_answers_start_nothing() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    let scope = Scope {
        session_id: "session".into(),
        workspace_id: "workspace".into(),
    };
    let context = Context {
        source: Target {
            scope,
            cloud_id: "source".into(),
            declaration: Declaration::new("example/source", "dev"),
        },
        declarations: std::collections::BTreeMap::new(),
        inventory: Vec::new(),
    };
    for operation in [
        operation(intent::Action::EnsureReady, intent::State::Succeeded, Phase::Ready),
        operation(
            intent::Action::EnsureReady,
            intent::State::Submitted,
            Phase::ConfirmationRequired,
        ),
    ] {
        let (answer, started) = app.continue_operation(
            "source",
            Submitted {
                operation,
                context: context.clone(),
                alias: "consumer".into(),
            },
            &ctx,
        );
        assert!(!started);
        assert_eq!(answer["done"], true);
        assert!(answer.get("executing").is_none());
    }
    assert!(app.cloud_prototype.production.companions.agent.executing.is_empty());
}

#[test]
fn a_busy_journal_is_retried_briefly() {
    let mut attempts = 0;
    let result = retry_busy(|| {
        attempts += 1;
        if attempts < 3 {
            Err(cloud_runtime::Error::Busy)
        } else {
            Ok(attempts)
        }
    });
    assert_eq!(result.unwrap(), 3);
}

#[test]
fn a_status_poll_never_starts_a_recorded_operation() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    let context = Context {
        source: Target {
            scope: Scope {
                session_id: "session".into(),
                workspace_id: "workspace".into(),
            },
            cloud_id: "source".into(),
            declaration: Declaration::new("example/source", "dev"),
        },
        declarations: std::collections::BTreeMap::new(),
        inventory: Vec::new(),
    };
    let request = |action: &str| -> UsageRequest {
        serde_json::from_value(json!({
            "request_id": "companion", "actor": "horizon:agent", "host_instance": "host",
            "deadline_at_millis": i64::MAX, "claimed": true,
            "cloud_companion": {"action": action, "cloud": "source", "alias": "consumer",
                "operation_id": OperationId::generate()}
        }))
        .unwrap()
    };
    let submitted = || Submitted {
        operation: operation(intent::Action::EnsureReady, intent::State::Submitted, Phase::Submitted),
        context: context.clone(),
        alias: "consumer".into(),
    };
    let (answer, started) = app.answer_submitted(&request("status"), "source", submitted(), &ctx);
    assert!(!started);
    assert_eq!(answer["phase"], "submitted");
    // Nothing runs it, so polling alone would wait forever: the agent must resend.
    assert_eq!(
        (answer["done"].as_bool(), answer["resend"].as_bool()),
        (Some(false), Some(true))
    );
    // While it runs on its card, polling is enough.
    let agent = &mut app.cloud_prototype.production.companions.agent;
    agent.executing.insert("target".into());
    let (answer, _) = app.answer_submitted(&request("status"), "source", submitted(), &ctx);
    assert_eq!(answer["resend"], false);
    assert_eq!(answer["message"], hint(Phase::Submitted));
    app.cloud_prototype.production.companions.agent.executing.clear();
    // The same operation sent again as Ensure Ready tries to continue it on its card,
    // which is not open here.
    let (answer, started) = app.answer_submitted(&request("ensure_ready"), "source", submitted(), &ctx);
    assert!(!started);
    assert_ne!(answer["message"], hint(Phase::Submitted));
    assert_eq!(answer["resend"], true);
}

#[test]
fn an_expired_ensure_ready_or_stop_is_refused_but_a_poll_is_answered() {
    let request = |action: &str, deadline: i64| -> UsageRequest {
        serde_json::from_value(json!({
            "request_id": "companion", "actor": "horizon:agent", "host_instance": "host",
            "deadline_at_millis": deadline, "claimed": true,
            "cloud_companion": {"action": action, "cloud": "source", "alias": "consumer",
                "operation_id": OperationId::generate()}
        }))
        .unwrap()
    };
    let past = manifest::now_millis() - 1;
    for action in ["ensure_ready", "stop"] {
        assert!(
            expired(&request(action, past))
                .unwrap_err()
                .starts_with("cloud_companion_expired")
        );
        assert!(expired(&request(action, i64::MAX)).is_ok());
    }
    assert!(expired(&request("status", past)).is_ok());
}

#[test]
fn a_missing_companion_found_before_a_session_change_is_not_offered_after_it() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    app.cloud_prototype.groups = CloudGroups(vec![cloud(1, "source", "workspace")]);
    let state = &mut app.cloud_prototype.production.companions;
    state.sync(Some("session"), &app.cloud_prototype.groups);
    let previous = state.entries["source"].owner.clone();
    state.set_session(Some("other"));
    state.sync(Some("other"), &app.cloud_prototype.groups);
    let id = OperationId::generate();
    let request: UsageRequest = serde_json::from_value(json!({
        "request_id": "companion", "actor": "horizon:agent", "host_instance": "host",
        "deadline_at_millis": i64::MAX, "claimed": true,
        "cloud_companion": {"action": "ensure_ready", "cloud": "source", "alias": "consumer",
            "operation_id": id}
    }))
    .unwrap();
    let declaration = Declaration::new("example/consumer", "dev");
    // The lookup ran for the previous session; its owner no longer matches the card.
    let refused = app
        .answer_missing(&request, "source", previous, declaration.clone())
        .unwrap_err();
    assert!(refused.starts_with("cloud_companion_unavailable"), "{refused}");
    let creation = &mut app.cloud_prototype.production.companions.agent.creation;
    assert!(creation.answer("source", "consumer", None, id).is_none());
    let current = app.cloud_prototype.production.companions.entries["source"]
        .owner
        .clone();
    let asked = app.answer_missing(&request, "source", current, declaration).unwrap();
    assert_eq!(asked["phase"], "confirmation_required");
}

#[test]
fn recorded_answers_cannot_restore_prompts_after_a_session_switch() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    app.cloud_prototype.groups = CloudGroups(vec![cloud(1, "source", "workspace"), cloud(2, "target", "workspace")]);
    let state = &mut app.cloud_prototype.production.companions;
    state.sync(Some("session"), &app.cloud_prototype.groups);
    let previous = state.entries["source"].owner.clone();
    let context = Context {
        source: Target {
            scope: previous.scope.clone(),
            cloud_id: "source".into(),
            declaration: Declaration::new("example/source", "dev"),
        },
        declarations: BTreeMap::from([("consumer".into(), Declaration::new("example/consumer", "dev"))]),
        inventory: Vec::new(),
    };
    state.set_session(Some("other"));
    state.sync(Some("other"), &app.cloud_prototype.groups);
    for action in ["status", "ensure_ready"] {
        let id = OperationId::generate();
        let request: UsageRequest = serde_json::from_value(json!({
            "request_id":"companion", "actor":"horizon:agent", "host_instance":"host",
            "deadline_at_millis":i64::MAX, "claimed":true,
            "cloud_companion":{"action":action,"cloud":"source","alias":"consumer","operation_id":id}
        }))
        .unwrap();
        let submitted = || {
            let mut submitted = Submitted {
                operation: operation(
                    intent::Action::EnsureReady,
                    intent::State::Submitted,
                    Phase::ConfirmationRequired,
                ),
                context: context.clone(),
                alias: "consumer".into(),
            };
            submitted.operation.intent.operation_id = id;
            submitted
        };
        let (answer, started) = app.answer_recorded(&request, "source", &previous, submitted(), &ctx);
        assert!(answer.unwrap_err().starts_with("cloud_companion_unavailable"));
        assert!(!started);
        assert!(
            app.cloud_prototype
                .production
                .companions
                .agent
                .creation
                .answer("source", "consumer", None, id)
                .is_none()
        );
        let current = app.cloud_prototype.production.companions.entries["source"]
            .owner
            .clone();
        let (answer, started) = app.answer_recorded(&request, "source", &current, submitted(), &ctx);
        assert_eq!(answer.unwrap()["phase"], "confirmation_required");
        assert!(!started);
        assert!(
            app.cloud_prototype
                .production
                .companions
                .agent
                .creation
                .answer("source", "consumer", None, id)
                .is_some()
        );
        app.cloud_prototype.production.companions.agent.discard_creations();
    }
}

#[test]
fn agent_card_attempts_replace_old_operation_and_progress() {
    use super::super::super::{Runtime, lifecycle::Action};
    for (stop, action, stage) in [
        (false, Action::Resume, Stage::Provision),
        (true, Action::Stop, Stage::Stopping),
    ] {
        let (temp, mut app) = crate::app::test_support::test_app();
        let root = temp.path().join("clouds");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            serde_json::to_vec(&json!({
                "runpod_key_file":root.join("key"), "ssh_identity_file":root.join("identity"),
                "docker_config":root.join("docker"), "registry_pull_auth_id":null,
                "cpu_flavors":["cpu3c"], "gpu_types":[]
            }))
            .unwrap(),
        )
        .unwrap();
        app.cloud_prototype.root = Some(root);
        app.cloud_prototype.groups =
            CloudGroups(vec![cloud(1, "source", "workspace"), cloud(2, "target", "workspace")]);
        app.cloud_prototype
            .production
            .companions
            .sync(Some("session"), &app.cloud_prototype.groups);
        let owner = app.cloud_prototype.production.companions.entries["source"]
            .owner
            .clone();
        let context = Context {
            source: Target {
                scope: owner.scope,
                cloud_id: "source".into(),
                declaration: Declaration::new("example/source", "dev"),
            },
            declarations: BTreeMap::new(),
            inventory: Vec::new(),
        };
        let mut runtime = Runtime {
            operation: Some(Action::Deploy),
            ..Runtime::default()
        };
        runtime.progress.stage(Stage::Ready, Instant::now());
        let previous_attempt = runtime.progress.attempt();
        app.cloud_prototype.production.runtimes.insert(2, runtime);
        app.execute_on_card(
            "source",
            "target",
            ("consumer", stop),
            OperationId::generate(),
            context,
            &egui::Context::default(),
        )
        .unwrap();
        let runtime = &app.cloud_prototype.production.runtimes[&2];
        assert_eq!(runtime.operation, Some(action));
        assert_eq!(runtime.stage, Some(stage));
        assert_eq!(runtime.progress.attempt(), previous_attempt + 1);
        assert!(runtime.progress.elapsed().is_none());
        assert!(runtime.progress.ended_in(Stage::Ready).is_none());
    }
}
