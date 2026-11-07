use super::*;
use horizon_core::cloud_runtime::prices::Profile;
use horizon_core::{
    PanelKind, PanelOptions, browser_actor,
    cloud_panel::{CloudConfig, CloudGroup, CloudLaunch, Placement},
};
use std::path::{Path, PathBuf};

const ID: &str = "27d497e2-1f54-4bc4-b746-ab2a3d9efad5";
const OTHER: &str = "9b2c4b0e-6f18-4d3a-9f0a-0e8f7b6d1c22";
const CLOUD: &str = "cloud-1";
/// An app with one cloud card, an agent panel in its workspace, and the agent's actor.
type Fixture = (tempfile::TempDir, HorizonApp, String, String);

fn profile() -> Profile {
    CloudConfig::parse(
        "version: 1\ndefault: quick-start\nprofiles:\n  quick-start:\n    provider: runpod\n    image: ghcr.io/peters/horizon-worker-base:cpu\n    cpu: 2\n    memory_gb: 4\n    gpu: false\n",
    )
    .unwrap()
    .profiles["quick-start"]
        .clone()
}

fn build_request(actor: &str, host: &str, lifecycle: Value, deadline: i64) -> UsageRequest {
    let mut value = json!({
        "request_id": "request-1",
        "actor": actor,
        "host_instance": host,
        "deadline_at_millis": deadline,
        "claimed": false,
    });
    value["cloud_lifecycle"] = lifecycle;
    serde_json::from_value(value).unwrap()
}

fn submit(action: &str, cloud: &str) -> Value {
    json!({"operation": "submit", "operation_id": ID, "action": action, "cloud": cloud})
}

fn operation_request(operation: &str) -> Value {
    json!({"operation": operation, "operation_id": ID})
}

/// An app with a cloud root, a settings file, one cloud card in a new workspace and an
/// agent panel in it. The workspace local ID and the agent's actor are returned.
fn agent_app() -> Fixture {
    let (temp, mut app) = crate::app::test_support::test_app();
    let root = temp.path().join("cloud");
    std::fs::create_dir_all(&root).unwrap();
    let absolute = |name: &str| root.join(name);
    let settings = json!({
        "runpod_key_file": absolute("key"),
        "ssh_identity_file": absolute("identity"),
        "docker_config": absolute("docker"),
        "registry_pull_auth_id": null,
        "cpu_flavors": ["cpu3c"], "gpu_types": [], "data_centers": ["US-MO-2"],
    });
    std::fs::write(root.join("settings.json"), settings.to_string()).unwrap();
    app.cloud_prototype.root = Some(root.clone());
    let workspace = app.board.create_workspace("agents");
    let local = app.board.workspace(workspace).unwrap().local_id.clone();
    let panel = app
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
    let actor = browser_actor(&app.board.panel(panel).unwrap().local_id);
    app.cloud_prototype.groups.0.push(group(local.clone()));
    (temp, app, local, actor)
}

/// The card of cloud `CLOUD` in `workspace`.
fn group(workspace: String) -> CloudGroup {
    let mut group = CloudGroup::new(
        1,
        "shop".into(),
        workspace,
        PathBuf::from("/home/person/shop"),
        [0.0, 0.0],
    );
    group.remote = Some(CloudLaunch {
        publishing_selection: None,
        deployment_started: true,
        id: CLOUD.into(),
        revision: "a".repeat(40),
        profile_name: "quick-start".into(),
        profile: profile(),
        placement: Placement::default(),
    });
    group
}

/// The record of a running cloud of one creator.
fn running_record(creator: Option<Creator>) -> Deployment {
    record("Ready", creator)
}

/// The record of a stopped cloud of one creator.
fn stopped_record(creator: Option<Creator>) -> Deployment {
    record("Stopped", creator)
}

fn record(stage: &str, creator: Option<Creator>) -> Deployment {
    let mut record: Deployment = serde_json::from_value(json!({
        "version":1,"cloud_id":CLOUD,"repository":"/home/person/shop","revision":"a".repeat(40),
        "profile":profile(),"stage":stage,
        "operation":{"state":"bound","worker_id":"worker-1"},
        "spec":{"operation_id":CLOUD,"image_digest":"sha256:abc","profile":profile(),
            "public_key":"ssh-ed25519 AAAA fixture","registry_auth_id":null,
            "gpu_types":[],"cpu_flavors":["cpu3c"],"data_centers":["US-MO-2"]},
        "worker":{"id":"worker-1","name":"fixture","imageName":"example/worker",
            "desiredStatus":"RUNNING","publicIp":"127.0.0.1","portMappings":{"22":9}},
        "sessions":[],"source_ready":true
    }))
    .unwrap();
    record.created_by = creator;
    record
}

/// Writes `record` as the cloud's saved state, so its directory holds a readable record.
fn save_cloud(root: &Path, record: &Deployment) {
    let directory = cloud_runtime::state::cloud_directory(root, CLOUD).unwrap();
    std::fs::create_dir_all(&directory).unwrap();
    let store = Store::lock(&directory).unwrap();
    store.save(record).unwrap();
}

fn creator(actor: &str) -> Creator {
    Creator {
        actor: actor.into(),
        session: Some("session-1".into()),
    }
}

fn answer(app: &mut HorizonApp, request: &UsageRequest) -> Result<Value, String> {
    let ctx = Context::default();
    app.answer_cloud_lifecycle(request, &ctx)
}

/// Polls until the operation ends, as a caller polls its status.
fn finish(app: &mut HorizonApp) -> Value {
    let ctx = Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        app.poll_cloud_lifecycle_requests(&ctx);
        if app.cloud_prototype.production.lifecycle_agent.operations[ID]
            .phase
            .done()
        {
            let actor = app.cloud_prototype.production.lifecycle_agent.operations[ID]
                .actor
                .clone();
            return answer(
                app,
                &build_request(
                    &actor,
                    manifest::host_instance(),
                    operation_request("status"),
                    manifest::now_millis(),
                ),
            )
            .unwrap();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the lifecycle operation did not end"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_request_from_another_host_or_without_an_agent_panel_is_unavailable() {
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(&temp.path().join("cloud"), &stopped_record(Some(creator(&actor))));
    let deadline = manifest::now_millis() + 10_000;
    let foreign = build_request(&actor, "another-host", submit("stop", CLOUD), deadline);
    assert_eq!(answer(&mut app, &foreign).unwrap_err(), UNAVAILABLE);
    let stranger = build_request(
        "horizon:not-a-panel",
        manifest::host_instance(),
        submit("stop", CLOUD),
        deadline,
    );
    assert_eq!(answer(&mut app, &stranger).unwrap_err(), UNAVAILABLE);
    assert!(
        app.cloud_prototype.production.lifecycle_agent.operations.is_empty(),
        "nothing is recorded for a refused caller"
    );
}

#[test]
fn an_invalid_expired_or_unknown_submission_stops_nothing() {
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(&temp.path().join("cloud"), &stopped_record(Some(creator(&actor))));
    let host = manifest::host_instance();
    let deadline = manifest::now_millis() + 10_000;
    let without_action = json!({"operation": "submit", "operation_id": ID, "cloud": CLOUD});
    assert_eq!(
        answer(&mut app, &build_request(&actor, host, without_action, deadline)).unwrap_err(),
        INVALID
    );
    let expired = build_request(&actor, host, submit("stop", CLOUD), manifest::now_millis() - 1);
    assert_eq!(answer(&mut app, &expired).unwrap_err(), EXPIRED);
    let unknown = build_request(&actor, host, submit("stop", "cloud-2"), deadline);
    assert_eq!(answer(&mut app, &unknown).unwrap_err(), UNKNOWN_CLOUD);
    assert!(app.cloud_prototype.production.lifecycle_agent.operations.is_empty());
}

#[test]
fn a_cloud_that_is_already_stopped_answers_stopped_and_sends_no_request() {
    // The saved record says the worker is stopped, so this run drives the whole answer
    // path without a provider request. `kept` says what a resume finds.
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(&temp.path().join("cloud"), &stopped_record(Some(creator(&actor))));
    let accepted = answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            submit("stop", CLOUD),
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    assert_eq!(accepted["phase"], json!("submitted"));
    assert_eq!(accepted["action"], json!("stop"));
    assert_eq!(accepted["done"], json!(false));
    assert_eq!(accepted["in_progress"], json!(IN_PROGRESS));

    let answer = finish(&mut app);
    assert_eq!(answer["phase"], json!("stopped"));
    assert_eq!(answer["done"], json!(true));
    assert_eq!(answer["stage"], json!(Stage::Stopped.label()));
    assert_eq!(answer["error"], json!(null));
    let kept = answer["kept"].as_str().unwrap_or_default();
    assert!(
        kept.contains("workspace"),
        "a caller learns what the stop kept: {answer}"
    );
}

#[test]
fn a_resume_of_a_running_cloud_is_refused_and_names_why() {
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(&temp.path().join("cloud"), &running_record(Some(creator(&actor))));
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            submit("resume", CLOUD),
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    let answer = finish(&mut app);
    assert_eq!(answer["phase"], json!("failed"));
    assert_eq!(answer["done"], json!(true));
    let error = answer["error"].as_str().unwrap_or_default();
    assert!(
        error.starts_with("cloud_lifecycle_refused") && error.contains("not stopped"),
        "a resume of a running cloud says which state it found: {error}"
    );
}

#[test]
fn another_agents_cloud_is_refused_before_any_provider_request() {
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(&temp.path().join("cloud"), &stopped_record(Some(creator(OTHER))));
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            submit("stop", CLOUD),
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    let answer = finish(&mut app);
    assert_eq!(answer["phase"], json!("failed"));
    assert_eq!(answer["error"], json!(NOT_AN_AGENTS_CLOUD));
    assert_eq!(
        answer["kept"],
        json!(null),
        "nothing was stopped, so there is nothing to keep"
    );
}

#[test]
fn a_cloud_a_person_created_or_one_without_a_record_is_refused() {
    let (temp, mut app, _workspace, actor) = agent_app();
    let root = temp.path().join("cloud");
    save_cloud(&root, &stopped_record(None));
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            submit("stop", CLOUD),
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    assert_eq!(finish(&mut app)["error"], json!(NOT_AN_AGENTS_CLOUD));

    // A cloud whose folder holds no record has nothing to stop.
    let directory = cloud_runtime::state::cloud_directory(&root, CLOUD).unwrap();
    std::fs::remove_file(directory.join("deployment.json")).unwrap();
    let new_id = "1f0a9d78-2c11-4b0f-8f0e-6f0e2b8a1c33";
    app.cloud_prototype.production.lifecycle_agent.operations.clear();
    let request = json!({"operation": "submit", "operation_id": new_id, "action": "stop", "cloud": CLOUD});
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            request,
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    let ctx = Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        app.poll_cloud_lifecycle_requests(&ctx);
        let operation = &app.cloud_prototype.production.lifecycle_agent.operations[new_id];
        if operation.phase.done() {
            assert_eq!(operation.error.as_deref(), Some(NO_RECORD));
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the operation did not end");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_lost_submission_answer_is_read_from_the_operation_and_never_sent_twice() {
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(&temp.path().join("cloud"), &stopped_record(Some(creator(&actor))));
    let host = manifest::host_instance();
    let deadline = manifest::now_millis() + 10_000;
    let first = answer(&mut app, &build_request(&actor, host, submit("stop", CLOUD), deadline)).unwrap();
    let second = answer(&mut app, &build_request(&actor, host, submit("stop", CLOUD), deadline)).unwrap();
    assert_eq!(
        first["operation_id"], second["operation_id"],
        "the same operation answers the repeated submission"
    );
    assert_eq!(
        app.cloud_prototype.production.lifecycle_agent.operations.len(),
        1,
        "one operation, and so one provider request"
    );
    // Another actor's status is its own: it needs an agent panel of its own, and this one
    // has none, so the request is answered as unavailable rather than with this operation.
    let other = answer(
        &mut app,
        &build_request(OTHER, host, operation_request("status"), deadline),
    );
    assert_eq!(
        other.unwrap_err(),
        UNAVAILABLE,
        "another actor reads no operation of this one"
    );
    let stranger = app
        .cloud_prototype
        .production
        .lifecycle_agent
        .operations
        .get(ID)
        .map(|operation| operation.actor.clone())
        .unwrap();
    assert_ne!(stranger, OTHER);
    finish(&mut app);
}
