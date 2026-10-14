use super::status::board_json;
use super::*;
use crate::app::test_support::test_app;
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};
use horizon_core::upkeep::Status;
use horizon_core::{PanelKind, PanelOptions, browser_actor};

fn request(actor: &str, upkeep: &Value, deadline: i64) -> UsageRequest {
    serde_json::from_value(json!({
        "request_id": "upkeep", "actor": actor, "host_instance": manifest::host_instance(),
        "deadline_at_millis": deadline, "claimed": true, "upkeep": upkeep,
    }))
    .unwrap()
}

fn ask(upkeep: &Value) -> (UsageRequest, UpkeepRequest) {
    let request = request("horizon:agent", upkeep, i64::MAX);
    let parsed = request.upkeep.clone().unwrap();
    assert!(parsed.valid(), "{upkeep}");
    (request, parsed)
}

fn caller() -> Caller {
    Caller {
        home: "workspace-mine".into(),
        clouds: vec!["mine".into()],
        loops: vec![("mine".into(), Some(true))],
        ambiguous: Vec::new(),
    }
}

fn config(repositories: &[&str]) -> Config {
    Config {
        repositories: repositories.iter().map(|name| (*name).to_owned()).collect(),
        authors: vec!["app/dependabot".into()],
        agent: worker::AGENT.into(),
        every_minutes: 60,
        instructions: "Keep them moving.".into(),
        ..Config::default()
    }
}

/// Saved Upkeeps: one in the caller's workspace and one in another.
fn saved() -> Vec<(String, Config)> {
    vec![
        ("theirs".into(), config(&["acme/secret"])),
        ("mine".into(), config(&["acme/web", "acme/api"])),
    ]
}

/// A workspace named `name` with a cloud `id` in it.
fn cloud_workspace(app: &mut HorizonApp, name: &str, id: &str) -> WorkspaceId {
    let profile = CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker\n    cpu: 4\n    memory_gb: 8\n",
    )
    .unwrap()
    .profiles["dev"]
        .clone();
    let workspace = app.board.create_workspace(name);
    let local = app.board.workspace(workspace).unwrap().local_id.clone();
    let mut group = CloudGroup::new(1, id.into(), local, std::env::temp_dir(), [0.0, 0.0]);
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: id.into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile,
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    app.cloud_prototype.groups.0.push(group);
    workspace
}

fn decided(upkeep: &Value, saved: Vec<(String, Config)>) -> Result<Next, String> {
    let (request, parsed) = ask(upkeep);
    decide(&request, &parsed, &caller(), saved)
}

#[test]
fn create_only_proposes_and_never_replaces_an_upkeep() {
    let Ok(Next::ProposeNew(proposed)) = decided(
        &json!({"operation": "create", "repositories": ["acme/web"]}),
        Vec::new(),
    ) else {
        panic!("create must propose");
    };
    assert_eq!(proposed.authors, ["app/dependabot"]);
    assert_eq!((proposed.agent.as_str(), proposed.every_minutes), (worker::AGENT, 60));
    assert_eq!(proposed.instructions, DEFAULT_INSTRUCTIONS);
    assert!(proposed.validate().is_ok());
    let error = decided(&json!({"operation": "create", "repositories": ["acme/web"]}), saved())
        .err()
        .unwrap();
    assert!(error.starts_with("upkeep_exists"), "{error}");
    // An Upkeep in another workspace does not count.
    assert!(matches!(
        decided(
            &json!({"operation": "create", "repositories": ["acme/web"]}),
            vec![("theirs".into(), config(&["acme/web"]))]
        ),
        Ok(Next::ProposeNew(..))
    ));
}

#[test]
fn add_proposes_only_the_new_repositories_and_keeps_the_limit() {
    let Ok(Next::ProposeRepositories {
        cloud,
        saved: held,
        repositories,
    }) = decided(
        &json!({"operation": "add", "repositories": ["ACME/web", "acme/docs"]}),
        saved(),
    )
    else {
        panic!("add must propose");
    };
    assert_eq!((cloud.as_str(), repositories), ("mine", vec!["acme/docs".to_owned()]));
    // The editor opens over the list as it was read, without the proposal.
    assert_eq!(held.repositories, ["acme/web", "acme/api"]);
    let Ok(Next::Answer(Ok(value))) = decided(&json!({"operation": "add", "repositories": ["acme/web"]}), saved())
    else {
        panic!("nothing new to add");
    };
    assert_eq!(value["added"], 0);
    let full: Vec<String> = (0..50).map(|n| format!("acme/r{n}")).collect();
    let error = decided(
        &json!({"operation": "add", "repositories": ["acme/more"]}),
        vec![(
            "mine".into(),
            config(&full.iter().map(String::as_str).collect::<Vec<_>>()),
        )],
    )
    .err()
    .unwrap();
    assert!(error.starts_with("upkeep_invalid_request"), "{error}");
}

#[test]
fn remove_saves_a_smaller_list_but_never_an_empty_one() {
    let Ok(Next::Remove { cloud, names }) =
        decided(&json!({"operation": "remove", "repositories": ["acme/API"]}), saved())
    else {
        panic!("remove must save");
    };
    assert_eq!((cloud.as_str(), names), ("mine", vec!["acme/API".to_owned()]));
    // Under the lock, the change applies to the list as it is then.
    let (live, _) = ask(&json!({"operation": "remove", "repositories": ["acme/API"]}));
    let mut held = config(&["acme/web", "acme/api", "acme/new"]);
    let removed = remove_from(&mut held, &["acme/API".to_owned()], &live).unwrap();
    // The board names the repository as the settings spell it, not as the agent did.
    assert_eq!(removed, ["acme/api"]);
    assert_eq!(held.repositories, ["acme/web", "acme/new"]);
    // A refused change leaves the list as it was.
    let mut held = config(&["acme/api"]);
    assert!(remove_from(&mut held, &["acme/api".to_owned()], &live).is_err());
    assert_eq!(held.repositories, ["acme/api"]);
    let late = request(
        "horizon:agent",
        &json!({"operation": "remove", "repositories": ["acme/api"]}),
        0,
    );
    let mut held = config(&["acme/web", "acme/api"]);
    let error = remove_from(&mut held, &["acme/api".to_owned()], &late).unwrap_err();
    assert!(error.starts_with("upkeep_expired"), "{error}");
    assert_eq!(held.repositories, ["acme/web", "acme/api"]);
    let error = decided(
        &json!({"operation": "remove", "repositories": ["acme/web", "acme/api"]}),
        saved(),
    )
    .err()
    .unwrap();
    assert!(error.contains("at least one repository"), "{error}");
    assert!(matches!(
        decided(&json!({"operation": "remove", "repositories": ["acme/secret"]}), saved()),
        Ok(Next::Answer(Ok(value))) if value["removed"] == 0
    ));
    // A request past its deadline saves nothing.
    let late = request(
        "horizon:agent",
        &json!({"operation": "remove", "repositories": ["acme/api"]}),
        0,
    );
    let parsed = late.upkeep.clone().unwrap();
    let error = decide(&late, &parsed, &caller(), saved()).err().unwrap();
    assert!(error.starts_with("upkeep_expired"), "{error}");
}

#[test]
fn signals_and_status_reach_only_the_upkeep_of_the_callers_workspace() {
    assert!(matches!(
        decided(&json!({"operation": "pause"}), saved()),
        Ok(Next::Signal { cloud, run_now: false, paused: Some(true) }) if cloud == "mine"
    ));
    assert!(matches!(
        decided(&json!({"operation": "resume"}), saved()),
        Ok(Next::Signal {
            paused: Some(false),
            ..
        })
    ));
    assert!(matches!(
        decided(&json!({"operation": "run_now"}), saved()),
        Ok(Next::Signal {
            run_now: true,
            paused: None,
            ..
        })
    ));
    assert!(matches!(
        decided(&json!({"operation": "status"}), saved()),
        Ok(Next::Status { cloud, running: Some(true), .. }) if cloud == "mine"
    ));
    for operation in ["status", "pause", "run_now"] {
        let error = decided(
            &json!({"operation": operation}),
            vec![("theirs".into(), config(&["acme/secret"]))],
        )
        .err()
        .unwrap();
        assert!(error.starts_with("upkeep_not_found"), "{error}");
    }
    // An ID that another saved cloud also has may name another workspace's Upkeep.
    let shared = Caller {
        ambiguous: vec!["mine".into()],
        ..caller()
    };
    for operation in ["status", "pause", "run_now"] {
        let (request, parsed) = ask(&json!({"operation": operation}));
        let error = decide(&request, &parsed, &shared, saved()).err().unwrap();
        assert!(error.starts_with("upkeep_ambiguous_cloud"), "{error}");
    }
}

#[test]
fn status_returns_only_parsed_fields_and_exact_pull_request_pages() {
    let board: Status = serde_json::from_value(json!({
        "schedule": {"every_minutes": 60, "paused": true},
        "runs": [{"started_at": "2026-10-13T10:00:00Z", "outcome": "done", "merged": 2}],
        "pull_requests": [
            {"repository": "acme/web", "number": 7, "title": "Bump x", "url": "https://github.com/acme/web/pull/7", "state": "ready", "checks": "passed", "secret": "dropped"},
            {"repository": "acme/web", "number": 8, "url": "https://evil.example/acme/web/pull/8", "state": "unknown_state"}
        ],
        "activity": [{"at": "2026-10-13T10:01:00Z", "text": "merged #7"}],
        "extra": {"anything": true}
    }))
    .unwrap();
    let value = board_json(&board);
    assert_eq!(value["paused"], true);
    assert_eq!(value["runs"][0]["outcome"], "done");
    assert_eq!(value["runs"][0]["started_at"], "2026-10-13T10:00:00Z");
    let pulls = value["pull_requests"].as_array().unwrap();
    assert_eq!(pulls[0]["url"], "https://github.com/acme/web/pull/7");
    assert_eq!(
        (pulls[0]["state"].as_str(), pulls[0]["checks"].as_str()),
        (Some("ready"), Some("passed"))
    );
    assert!(pulls[0].get("secret").is_none());
    assert_eq!(pulls[1]["url"], Value::Null);
    assert_eq!(pulls[1]["state"], "other");
    assert!(value.get("extra").is_none());
    // A missing or broken document gives a fixed text, never the parser's quote of it.
    let missing = status("upkeep-test-no-such-cloud", &config(&["acme/web"]), None);
    assert_eq!(missing["status_error"], "The Upkeep status is missing or not valid.");
    assert_eq!(missing["repositories"], json!(["acme/web"]));
}

#[test]
#[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
fn only_an_agent_panel_of_this_horizon_is_a_caller_and_it_sees_its_own_clouds() {
    let (_temp, mut app) = test_app();
    let profile = CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker\n    cpu: 4\n    memory_gb: 8\n",
    )
    .unwrap()
    .profiles["dev"]
        .clone();
    let mine = app.board.create_workspace("mine");
    let theirs = app.board.create_workspace("theirs");
    for (id, workspace) in [("mine", mine), ("theirs", theirs)] {
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        let mut group = CloudGroup::new(1, id.into(), local, std::env::temp_dir(), [0.0, 0.0]);
        group.remote = Some(CloudLaunch {
            deployment_started: true,
            id: id.into(),
            revision: "a".repeat(40),
            profile_name: "dev".into(),
            profile: profile.clone(),
            placement: horizon_core::cloud_panel::Placement::default(),
        });
        app.cloud_prototype.groups.0.push(group);
    }
    let panel = |app: &mut HorizonApp, kind| {
        let options = PanelOptions {
            command: Some("/bin/sh".into()),
            args: vec!["-c".into(), "exit 0".into()],
            kind,
            ..PanelOptions::default()
        };
        let id = app.board.create_panel(options, mine).unwrap();
        browser_actor(&app.board.panel(id).unwrap().local_id)
    };
    let agent = panel(&mut app, PanelKind::Codex);
    let shell = panel(&mut app, PanelKind::Shell);
    let (_, found) = app
        .caller(&request(&agent, &json!({"operation": "status"}), i64::MAX))
        .unwrap();
    let home = app.board.workspace(mine).unwrap().local_id.clone();
    assert_eq!(
        (found.home.as_str(), found.clouds),
        (home.as_str(), vec!["mine".to_owned()])
    );
    // Before it changes anything, the request checks that its caller still stands there.
    let asked = request(&agent, &json!({"operation": "pause"}), i64::MAX);
    assert_eq!(app.still_caller(&asked, &home), Ok(mine));
    assert!(app.cloud_in(&home, "mine").is_ok());
    assert!(app.cloud_in(&home, "theirs").is_err(), "a cloud of another workspace");
    let error = app.still_caller(&asked, "a-workspace-of-another-session").unwrap_err();
    assert!(error.starts_with("upkeep_unavailable"), "{error}");
    let error = app
        .caller(&request(&shell, &json!({"operation": "status"}), i64::MAX))
        .err()
        .unwrap();
    assert!(error.starts_with("upkeep_unavailable"), "{error}");
    let mut foreign = request(&agent, &json!({"operation": "status"}), i64::MAX);
    foreign.host_instance = "another-host".into();
    assert!(app.caller(&foreign).is_err());
    // The panel of a cloud member shows an agent on the worker, which never asks.
    let member = panel(&mut app, PanelKind::Claude);
    let member_id = member.trim_start_matches("horizon:").to_owned();
    app.cloud_prototype.groups.0[0].panels.push(member_id);
    let error = app
        .caller(&request(&member, &json!({"operation": "status"}), i64::MAX))
        .err()
        .unwrap();
    assert!(error.starts_with("upkeep_unavailable"), "{error}");
    let invalid = request(&agent, &json!({"operation": "add"}), i64::MAX);
    assert_eq!(app.caller(&invalid).err().unwrap(), "upkeep_invalid_request");
}

#[test]
fn agent_changes_reach_every_board_of_their_cloud_and_no_other() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("upkeep");
    let board = |app: &mut HorizonApp, cloud: &str| {
        let status = worker::directory(cloud).join("status.json");
        let options = PanelOptions {
            kind: PanelKind::Upkeep,
            command: Some(status.display().to_string()),
            ..PanelOptions::default()
        };
        let id = app.board.create_panel(options, workspace).unwrap();
        // As the app links a board to its saved Upkeep.
        let panel = app.board.panel_mut(id).unwrap();
        panel.content.upkeep_mut().unwrap().cloud = Some(cloud.to_owned());
        id
    };
    let changes = |app: &HorizonApp, id| {
        app.board
            .panel(id)
            .and_then(|panel| panel.content.upkeep())
            .map(|board| {
                board
                    .agent_changes
                    .iter()
                    .map(|change| change.kind.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap()
    };
    let mine = board(&mut app, "mine");
    let theirs = board(&mut app, "theirs");
    app.record_agent_change("mine", AgentChangeKind::Paused);
    app.show_agent_changes();
    assert_eq!(changes(&app, mine), [AgentChangeKind::Paused]);
    assert!(changes(&app, theirs).is_empty());
    // A board opened later shows the changes made before it.
    let reopened = board(&mut app, "mine");
    app.show_agent_changes();
    assert_eq!(changes(&app, reopened), [AgentChangeKind::Paused]);
    // A board rebuilt by a session switch, with the same panels as before, shows them too.
    let panel = app.board.panel_mut(reopened).unwrap();
    panel.content.upkeep_mut().unwrap().agent_changes.clear();
    app.show_agent_changes();
    assert_eq!(changes(&app, reopened), [AgentChangeKind::Paused]);
}

#[test]
fn a_repository_proposal_opens_the_editor_with_only_the_new_names_to_save() {
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let add = ["Acme/API".to_owned()];
    let error = app
        .propose_upkeep_repositories(&ctx, WorkspaceId(9_999), "mine", &config(&["acme/web"]), &add)
        .unwrap_err();
    assert!(error.contains("workspace is gone"), "{error}");
    let workspace = cloud_workspace(&mut app, "Dependencies", "mine");
    // A cloud that moved to another workspace while the request ran is not proposed there.
    let elsewhere = app.board.create_workspace("elsewhere");
    let error = app
        .propose_upkeep_repositories(&ctx, elsewhere, "mine", &config(&["acme/web"]), &add)
        .unwrap_err();
    assert!(error.contains("no longer in this workspace"), "{error}");
    // The saved Upkeeps may be read again right after a save: the proposal does not wait.
    assert!(app.upkeep.saved.is_none());
    app.propose_upkeep_repositories(&ctx, workspace, "mine", &config(&["acme/web"]), &add)
        .unwrap();
    let editor = app.upkeep.editor.as_ref().unwrap();
    assert_eq!(editor.cloud, "mine");
    assert_eq!(editor.changes(), (vec!["acme/api".to_owned()], Vec::new()));
    // The editor lists the proposal on its own, as the catalog may not hold it.
    assert_eq!(editor.proposed, ["acme/api"]);
    // It names the workspace, as the person may look at another one.
    assert_eq!(editor.proposed_for, "Dependencies");
    let error = app
        .propose_upkeep_repositories(&ctx, workspace, "mine", &config(&["acme/web"]), &add)
        .err()
        .unwrap();
    assert!(error.contains("already open"), "{error}");
    // New Upkeep waits too while the editor is open.
    let other = app.board.create_workspace("other");
    let error = app.propose_new_upkeep(other, &config(&["acme/x"])).err().unwrap();
    assert!(error.contains("already open"), "{error}");
}

#[test]
fn a_new_upkeep_proposal_waits_for_the_saved_upkeeps_and_never_doubles_one() {
    let (_temp, mut app) = test_app();
    let workspace = cloud_workspace(&mut app, "agents", "mine");
    let error = app.still_without_upkeep(workspace).unwrap_err();
    assert!(error.starts_with("upkeep_unavailable"), "{error}");
    app.upkeep.saved = Some(std::sync::Arc::new(vec![("theirs".into(), config(&["acme/x"]))]));
    assert!(app.still_without_upkeep(workspace).is_ok());
    // The Upkeep started while the request read the files.
    app.upkeep.saved = Some(std::sync::Arc::new(vec![("mine".into(), config(&["acme/x"]))]));
    let error = app.still_without_upkeep(workspace).unwrap_err();
    assert!(error.starts_with("upkeep_exists"), "{error}");
    // A workspace that closed while the request ran gets no dialog.
    let error = app.still_without_upkeep(WorkspaceId(9_999)).unwrap_err();
    assert!(error.starts_with("upkeep_unavailable"), "{error}");
}
