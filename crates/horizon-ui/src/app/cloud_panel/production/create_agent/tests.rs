use super::*;
use horizon_core::{
    PanelKind, PanelOptions, browser_actor, cloud_panel::CloudConfig, cloud_runtime::state::Deployment,
};
use std::path::{Path, PathBuf};

mod github_app;

const ID: &str = "27d497e2-1f54-4bc4-b746-ab2a3d9efad5";
const OTHER: &str = "9b2c4b0e-6f18-4d3a-9f0a-0e8f7b6d1c22";

fn profile() -> Profile {
    CloudConfig::parse(
        "version: 1\ndefault: quick-start\nprofiles:\n  quick-start:\n    provider: runpod\n    image: ghcr.io/peters/horizon-worker-base:cpu\n    cpu: 2\n    memory_gb: 4\n    gpu: false\n",
    )
    .unwrap()
    .profiles["quick-start"]
        .clone()
}

fn build_request(actor: &str, host: &str, create: Value, deadline: i64) -> UsageRequest {
    let mut value = json!({
        "request_id": "request-1",
        "actor": actor,
        "host_instance": host,
        "deadline_at_millis": deadline,
        "claimed": false,
    });
    value["cloud_create"] = create;
    serde_json::from_value(value).unwrap()
}

fn submit(repository: &str) -> Value {
    json!({"operation": "submit", "operation_id": ID, "repository": repository, "quick_start": true})
}

fn status_request(operation_id: &str) -> Value {
    json!({"operation": "status", "operation_id": operation_id})
}

/// An app whose cloud root holds `settings`, with an agent panel in a workspace, and the
/// agent's actor. The workspace local ID is returned too.
fn agent_app(settings: Option<Value>) -> (tempfile::TempDir, HorizonApp, String, String) {
    let (temp, mut app) = crate::app::test_support::test_app();
    let root = temp.path().join("cloud");
    std::fs::create_dir_all(&root).unwrap();
    let absolute = |name: &str| root.join(name);
    let mut file = json!({
        "runpod_key_file": absolute("key"),
        "ssh_identity_file": absolute("identity"),
        "docker_config": absolute("docker"),
        "registry_pull_auth_id": null,
        "cpu_flavors": ["cpu3c"], "gpu_types": [], "data_centers": ["US-MO-2"],
    });
    if let Some(settings) = settings {
        file["agent_lifecycle"] = settings;
    }
    std::fs::write(root.join("settings.json"), file.to_string()).unwrap();
    app.cloud_prototype.root = Some(root.clone());
    let workspace = app.board.create_workspace("agents");
    let local = app.board.workspace(workspace).unwrap().local_id.clone();
    let (command, args) = ("/bin/sh", vec!["-c".into(), "exit 0".into()]);
    let panel = app
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
    let actor = browser_actor(&app.board.panel(panel).unwrap().local_id);
    // A session the session writer never saved, so no fence can be taken for it.
    app.active_session = Some(crate::app::ActiveSession {
        session_id: "never-saved".into(),
        lease: None,
        last_lease_refresh: None,
        persistent: true,
    });
    (temp, app, local, actor)
}

fn policy() -> Value {
    json!({
        "providers": ["runpod"],
        "max_hourly": 1.0,
        "repositories": [PathBuf::from("/home/person/shop")],
        "allow_github_sign_in_skip": false,
    })
}

fn answer(app: &mut HorizonApp, request: &UsageRequest) -> Result<Value, String> {
    let ctx = Context::default();
    app.answer_cloud_create(request, &ctx)
}

/// Records an operation as the host holds one after `submit` accepted it, without the
/// reading thread: its refusals are tested apart from what a submitted cloud registers.
fn accept(app: &mut HorizonApp, actor: &str, workspace: &str, cloud: &str) {
    app.cloud_prototype.production.create_agent.operations.insert(
        ID.into(),
        Operation {
            id: ID.into(),
            actor: actor.into(),
            workspace: workspace.into(),
            cloud: cloud.into(),
            name: "shop".into(),
            phase: Phase::Submitted,
            card: None,
            ready: None,
            error: None,
            cancel: Some(Cancellation::default()),
            skip_sign_in: false,
        },
    );
}

fn prepared_message(app: &mut HorizonApp, request: Box<Request>) {
    send(
        app,
        Message::Prepared {
            operation: ID.into(),
            request,
            skip_sign_in: false,
        },
    );
}

fn send(app: &mut HorizonApp, message: Message) {
    let sender = app.cloud_prototype.production.create_agent.sender();
    sender.send(message).unwrap();
}

/// Polls until the operation ends, as a caller polls its status.
fn finish(app: &mut HorizonApp, ctx: &Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !app.cloud_prototype.production.create_agent.operations[ID].phase.done() {
        assert!(std::time::Instant::now() < deadline, "the operation did not end");
        app.poll_cloud_create_requests(ctx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn cancellation() -> Value {
    json!({"operation": "cancel", "operation_id": ID})
}

#[test]
fn a_request_from_another_host_or_without_an_agent_panel_is_unavailable() {
    let (_temp, mut app, _workspace, actor) = agent_app(Some(policy()));
    let deadline = manifest::now_millis() + 10_000;
    let foreign = build_request(&actor, "another-host", submit("/home/person/shop"), deadline);
    assert_eq!(answer(&mut app, &foreign).unwrap_err(), UNAVAILABLE);
    let stranger = build_request(
        "horizon:not-a-panel",
        manifest::host_instance(),
        submit("/home/person/shop"),
        deadline,
    );
    assert_eq!(answer(&mut app, &stranger).unwrap_err(), UNAVAILABLE);
    assert!(app.cloud_prototype.production.create_agent.operations.is_empty());
    assert!(app.cloud_prototype.groups.0.is_empty(), "nothing is registered");
}

#[test]
fn an_invalid_or_expired_submission_rents_and_registers_nothing() {
    let (_temp, mut app, _workspace, actor) = agent_app(Some(policy()));
    let host = manifest::host_instance();
    let deadline = manifest::now_millis() + 10_000;
    let committed_profile = json!({
        "operation": "submit", "operation_id": ID,
        "repository": "/home/person/shop", "quick_start": false,
    });
    assert_eq!(
        answer(&mut app, &build_request(&actor, host, committed_profile, deadline)).unwrap_err(),
        INVALID
    );
    let expired = build_request(&actor, host, submit("/home/person/shop"), manifest::now_millis() - 1);
    assert!(
        answer(&mut app, &expired)
            .unwrap_err()
            .starts_with("cloud_create_expired")
    );
    assert!(app.cloud_prototype.production.create_agent.operations.is_empty());
    assert!(app.cloud_prototype.groups.0.is_empty());
}

#[test]
fn a_machine_without_an_agent_lifecycle_policy_rents_nothing() {
    let (_temp, mut app, _workspace, actor) = agent_app(None);
    let request = build_request(
        &actor,
        manifest::host_instance(),
        submit("/home/person/shop"),
        manifest::now_millis() + 10_000,
    );
    // The settings are read on the operation's own thread, so the caller first sees an
    // accepted operation and then the refusal in its status.
    assert_eq!(answer(&mut app, &request).unwrap()["phase"], json!("submitted"));
    let ctx = Context::default();
    finish(&mut app, &ctx);
    let operation = &app.cloud_prototype.production.create_agent.operations[ID];
    assert_eq!(operation.phase, Phase::Failed);
    assert_eq!(operation.error.as_deref(), Some(REFUSED));
    assert!(operation.card.is_none(), "nothing is registered");
    assert!(app.cloud_prototype.groups.0.is_empty());
    assert!(app.cloud_prototype.production.runtimes.is_empty());
}

#[test]
fn a_quick_start_cloud_needs_a_saved_session() {
    let (_temp, mut app, _workspace, actor) = agent_app(Some(policy()));
    app.active_session = None;
    let request = build_request(
        &actor,
        manifest::host_instance(),
        submit("/home/person/shop"),
        manifest::now_millis() + 10_000,
    );
    assert_eq!(answer(&mut app, &request).unwrap_err(), NEEDS_SESSION);
    assert!(app.cloud_prototype.groups.0.is_empty());
}

#[test]
fn a_repeated_submission_answers_the_same_operation_without_a_second_cloud() {
    let (_temp, mut app, _workspace, actor) = agent_app(Some(policy()));
    let request = build_request(
        &actor,
        manifest::host_instance(),
        submit("/home/person/shop"),
        manifest::now_millis() + 10_000,
    );
    let first = answer(&mut app, &request).unwrap();
    let again = answer(&mut app, &request).unwrap();
    assert_eq!(first["operation_id"], json!(ID));
    assert_eq!(first["cloud"], again["cloud"]);
    assert_eq!(app.cloud_prototype.production.create_agent.operations.len(), 1);
    // Another agent cannot read, or take over, an operation it did not submit.
    // A caller without an agent panel is refused before any operation is looked at.
    let stranger = build_request(
        "horizon:other",
        manifest::host_instance(),
        status_request(ID),
        manifest::now_millis(),
    );
    assert_eq!(answer(&mut app, &stranger).unwrap_err(), UNAVAILABLE);
}

/// The record of a cloud whose worker is ready and running, as the deployment saves it.
fn ready_record(root: &Path) -> Deployment {
    serde_json::from_value(json!({
        "version":1,"cloud_id":"cloud-1","repository":root.join("shop"),"revision":"a".repeat(40),
        "profile":profile(),"stage":"Ready",
        "operation":{"state":"bound","worker_id":"worker-1"},
        "spec":null,"sessions":[],
        "worker":{"id":"worker-1","name":"worker-1","imageName":"example/worker","desiredStatus":"RUNNING",
            "costPerHr":0.44,"memoryInGb":4,"vcpuCount":2}
    }))
    .unwrap()
}

/// A prepared request makes the cloud the card shows, before anything is rented.
fn prepared_request(root: &Path) -> Box<Request> {
    let mut request = Request::new(
        "cloud-1".into(),
        root.join("shop"),
        "a".repeat(40),
        profile(),
        root.join("state").join("cloud-1"),
        Settings::for_cloud(&root.join("settings.json"), &Placement::default()).unwrap(),
    );
    request.profile_name = Some("quick-start".into());
    Box::new(request)
}

#[test]
fn the_cloud_is_registered_on_the_card_of_the_callers_workspace_before_allocation() {
    let (temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    let cloud = "cloud-1";
    accept(&mut app, &actor, &workspace, cloud);
    let mut prepared = prepared_request(&root);
    prepared.cloud_id.clone_from(&cloud.to_owned());
    let (card, _fence) = app
        .register_create_cloud(ID, &prepared)
        .expect("the cloud is registered");

    let groups = &app.cloud_prototype.groups.0;
    assert_eq!(groups.len(), 1, "the cloud has one card, in the caller's workspace");
    let group = &groups[0];
    assert_eq!(
        (group.workspace.clone(), group.title.clone()),
        (workspace, "shop".into())
    );
    assert_eq!(group.cwd, prepared.repository);
    assert_eq!(group.issue, card);
    let launch = group.remote.as_ref().expect("the card is a cloud");
    assert_eq!(
        (launch.id.as_str(), launch.profile_name.as_str()),
        (cloud, "quick-start")
    );
    assert_eq!(launch.revision, prepared.revision);
    assert!(
        launch.deployment_started,
        "the card knows the deployment started, so it never starts its own"
    );
    let operation = &app.cloud_prototype.production.create_agent.operations[ID];
    assert_eq!(operation.card, Some(card));
    assert!(
        !app.cloud_prototype.production.runtimes.contains_key(&card),
        "no deployment is started before the session fence is durable"
    );
    // A refusal before the cloud exists leaves no card at all.
    assert!(
        app.cloud_prototype.production.create_agent.running(),
        "the operation is still in flight while its fence is waited for"
    );
}

#[test]
fn a_cloud_refused_after_registration_removes_the_card_it_created() {
    let (temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    let cloud = "cloud-1";
    accept(&mut app, &actor, &workspace, cloud);
    let mut prepared = prepared_request(&root);
    prepared.cloud_id.clone_from(&cloud.to_owned());
    let (card, _fence) = app.register_create_cloud(ID, &prepared).expect("registered");
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    for error in ["cloud_create_busy: first", "cloud_create_failed: second"] {
        send(
            &mut app,
            Message::Refused {
                operation: ID.into(),
                error: error.into(),
            },
        );
        app.poll_cloud_create_requests(&Context::default());
    }
    let operation = &app.cloud_prototype.production.create_agent.operations[ID];
    assert_eq!(operation.phase, Phase::Failed);
    assert_eq!(
        operation.error.as_deref(),
        Some("cloud_create_busy: first"),
        "the first outcome of an operation stands"
    );
    // A cloud that never allocated leaves no card to remove later, and the same
    // repository may be submitted again.
    assert!(app.cloud_prototype.groups.0.is_empty(), "the card is removed again");
    assert!(operation.card.is_none());
    assert!(!app.cloud_prototype.production.runtimes.contains_key(&card));
}

#[test]
fn a_cancel_before_the_cloud_is_registered_rents_nothing() {
    let (temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    accept(&mut app, &actor, &workspace, "cloud-1");
    let cancelled = answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            cancellation(),
            manifest::now_millis(),
        ),
    )
    .unwrap();
    assert_eq!(cancelled["phase"], json!("cancelled"));
    // The reading thread reports what it read after the cancel: it registers no card.
    prepared_message(&mut app, prepared_request(&root));
    app.poll_cloud_create_requests(&Context::default());
    assert!(app.cloud_prototype.groups.0.is_empty(), "no card was made");
    assert!(
        app.cloud_prototype.production.runtimes.is_empty(),
        "no card, so no deployment"
    );
    let operation = &app.cloud_prototype.production.create_agent.operations[ID];
    assert_eq!(
        (operation.phase, operation.error.as_deref()),
        (Phase::Cancelled, Some(CANCELLED))
    );
    assert!(operation.card.is_none());
}

#[test]
fn a_cancel_while_the_session_is_made_durable_rents_nothing() {
    let (temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    let cloud = "cloud-1";
    accept(&mut app, &actor, &workspace, cloud);
    let mut prepared = prepared_request(&root);
    prepared.cloud_id.clone_from(&cloud.to_owned());
    let (card, _fence) = app.register_create_cloud(ID, &prepared).expect("registered");
    let cancelled = answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            cancellation(),
            manifest::now_millis(),
        ),
    )
    .unwrap();
    assert_eq!(cancelled["phase"], json!("cancelled"));
    assert!(
        app.cloud_prototype.groups.0.is_empty(),
        "nothing was rented, so the card goes again"
    );
    // The fence reports afterwards; the cancelled operation never deploys.
    send(
        &mut app,
        Message::Durable {
            operation: ID.into(),
            request: prepared,
        },
    );
    app.poll_cloud_create_requests(&Context::default());
    assert!(
        !app.cloud_prototype.production.runtimes.contains_key(&card),
        "no deployment starts after the cancel"
    );
    let operation = &app.cloud_prototype.production.create_agent.operations[ID];
    assert_eq!(
        (operation.phase, operation.error.as_deref()),
        (Phase::Cancelled, Some(CANCELLED)),
        "the cancel stands, and its answer names the worker that may exist"
    );
    assert!(operation.card.is_none());
}

#[test]
fn a_card_removed_during_the_fence_rents_nothing() {
    let (temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    let cloud = "cloud-1";
    accept(&mut app, &actor, &workspace, cloud);
    let mut prepared = prepared_request(&root);
    prepared.cloud_id.clone_from(&cloud.to_owned());
    let (card, _fence) = app.register_create_cloud(ID, &prepared).expect("registered");
    // The person removes the card while the session save runs.
    app.cloud_prototype.groups.0.retain(|group| group.issue != card);
    send(
        &mut app,
        Message::Durable {
            operation: ID.into(),
            request: prepared,
        },
    );
    app.poll_cloud_create_requests(&Context::default());
    assert!(
        !app.cloud_prototype.production.runtimes.contains_key(&card),
        "a card nobody shows deploys nothing"
    );
    let operation = &app.cloud_prototype.production.create_agent.operations[ID];
    assert_eq!(operation.phase, Phase::Failed);
    assert_eq!(operation.error.as_deref(), Some(CARD_REMOVED));
    assert!(!app.cloud_prototype.production.create_agent.owns(card));
}

#[test]
fn a_late_message_never_replaces_a_cancelled_token() {
    let (temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    let cloud = "cloud-1";
    accept(&mut app, &actor, &workspace, cloud);
    let mut prepared = prepared_request(&root);
    prepared.cloud_id.clone_from(&cloud.to_owned());
    let (card, _fence) = app.register_create_cloud(ID, &prepared).expect("registered");
    send(
        &mut app,
        Message::Durable {
            operation: ID.into(),
            request: prepared.clone(),
        },
    );
    app.poll_cloud_create_requests(&Context::default());
    assert_eq!(
        app.cloud_prototype.production.create_agent.operations[ID].phase,
        Phase::RollingOut
    );
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            cancellation(),
            manifest::now_millis(),
        ),
    )
    .unwrap();
    let token = app.cloud_prototype.production.runtimes[&card]
        .cancel
        .clone()
        .expect("the card holds the token of its deployment");
    assert!(token.is_cancelled());
    // A second report of the same durable session must not start a deployment with a
    // fresh token that nobody cancelled.
    send(
        &mut app,
        Message::Durable {
            operation: ID.into(),
            request: prepared,
        },
    );
    app.poll_cloud_create_requests(&Context::default());
    let held = app.cloud_prototype.production.runtimes[&card]
        .cancel
        .clone()
        .expect("the card keeps its token");
    assert!(
        held.is_cancelled(),
        "a cancelled operation's token is never replaced by a live one"
    );
    assert_eq!(
        app.cloud_prototype.production.create_agent.operations[ID].phase,
        Phase::Cancelled
    );
}

#[test]
fn the_worker_is_rented_only_after_the_session_is_durable() {
    let (temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    let cloud = "cloud-1";
    accept(&mut app, &actor, &workspace, cloud);
    let mut prepared = prepared_request(&root);
    prepared.cloud_id.clone_from(&cloud.to_owned());
    let (card, _fence) = app.register_create_cloud(ID, &prepared).expect("registered");
    // Registration alone starts nothing: the fence message has not arrived.
    assert!(
        !app.cloud_prototype.production.runtimes.contains_key(&card),
        "nothing is deployed before the session that names the cloud is durable"
    );
    assert_eq!(
        app.cloud_prototype.production.create_agent.operations[ID].phase,
        Phase::Submitted
    );
    send(
        &mut app,
        Message::Durable {
            operation: ID.into(),
            request: prepared,
        },
    );
    let ctx = Context::default();
    app.poll_cloud_create_requests(&ctx);
    assert_eq!(
        app.cloud_prototype.production.create_agent.operations[ID].phase,
        Phase::RollingOut,
        "the durable session is what releases the deployment"
    );
    let runtime = &app.cloud_prototype.production.runtimes[&card];
    assert!(runtime.receiver.is_some() && runtime.sender.is_some());
    // This fixture has no provider credential and no SSH identity, so the deployment
    // stops before it reaches a provider.
    finish(&mut app, &ctx);
    let operation = &app.cloud_prototype.production.create_agent.operations[ID];
    assert_eq!(operation.phase, Phase::Failed);
    let error = operation.error.as_deref().unwrap_or_default();
    assert!(!error.is_empty(), "the failure says why");
    assert!(operation.ready.is_none());
    assert!(
        app.cloud_prototype.production.runtimes[&card]
            .state
            .as_ref()
            .is_none_or(|state| state.worker.is_none()),
        "no worker was requested"
    );
}

#[test]
fn a_worker_that_asks_for_a_github_sign_in_gets_the_policy_answer_without_a_person() {
    let prompts = [
        Event::GitHub(Prompt::Web {
            url: "https://github.example/device".into(),
        }),
        Event::GitHub(Prompt::Device {
            user_code: "WDJB-MJHT".into(),
            verification_uri: "https://github.example/device".into(),
            expires_at: std::time::SystemTime::now(),
        }),
    ];
    for prompt in prompts {
        assert_eq!(
            sign_in(&prompt, true),
            Some((SKIPPED_SIGN_IN, true)),
            "the policy's skip continues without GitHub access"
        );
        let (line, continue_without) = sign_in(&prompt, false).expect("the person is needed");
        assert!(!continue_without);
        assert!(line.starts_with("cloud_create_needs_person"), "{line}");
        assert!(
            line.contains("cloud_list"),
            "the answer must name the worker that may already exist: {line}"
        );
    }
    // A worker that already holds access, and every other event, ask nothing of a person.
    assert_eq!(
        sign_in(
            &Event::GitHub(Prompt::Ended {
                reason: "expired".into(),
                renewable: true
            }),
            false
        ),
        None
    );
    assert_eq!(sign_in(&Event::Output("line".into()), false), None);
}

#[test]
fn a_workspace_cloud_for_the_same_repository_refuses_a_second_one() {
    let (temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    let prepared = prepared_request(&root);
    let mut group = horizon_core::cloud_panel::CloudGroup::new(
        1,
        "shop".into(),
        workspace,
        prepared.repository.clone(),
        [0.0, 0.0],
    );
    group.remote = Some(CloudLaunch {
        publishing_selection: None,
        deployment_started: true,
        id: "cloud-1".into(),
        revision: "a".repeat(40),
        profile_name: "quick-start".into(),
        profile: profile(),
        placement: Placement::default(),
    });
    let workspace_in_cloud = group.workspace.clone();
    app.cloud_prototype.groups.0.push(group);
    accept(&mut app, &actor, &workspace_in_cloud, "cloud-1");
    prepared_message(&mut app, prepared);
    app.poll_cloud_create_requests(&Context::default());
    let error = app.cloud_prototype.production.create_agent.operations[ID]
        .error
        .clone()
        .unwrap_or_default();
    assert!(error.starts_with("cloud_create_busy"), "{error}");
    assert_eq!(app.cloud_prototype.groups.0.len(), 1, "no second card was made");
}

#[test]
fn the_cards_own_start_path_cannot_deploy_a_cloud_an_agent_owns() {
    let (temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    let prepared = prepared_request(&root);
    let mut group = horizon_core::cloud_panel::CloudGroup::new(
        1,
        "shop".into(),
        workspace,
        prepared.repository.clone(),
        [0.0, 0.0],
    );
    group.remote = Some(CloudLaunch {
        publishing_selection: None,
        deployment_started: true,
        id: "cloud-1".into(),
        revision: "a".repeat(40),
        profile_name: "quick-start".into(),
        profile: profile(),
        placement: Placement::default(),
    });
    app.cloud_prototype.groups.0.push(group);
    let (workspace, cloud) = (app.cloud_prototype.groups.0[0].workspace.clone(), "cloud-1");
    accept(&mut app, &actor, &workspace, cloud);
    let state = &mut app.cloud_prototype.production.create_agent;
    let running = state.operations.get_mut(ID).unwrap();
    running.phase = Phase::RollingOut;
    running.card = Some(1);
    assert!(state.owns(1));
    app.prepare_production_deployment(1, None, &Context::default());
    let runtime = app.cloud_prototype.production.runtimes.get(&1);
    assert!(
        runtime.is_none_or(|runtime| runtime.preparation.is_none() && runtime.receiver.is_none()),
        "the card started no second deployment"
    );
    // Once the operation ends, the card owns its cloud again and prepares its own start.
    app.cloud_prototype
        .production
        .create_agent
        .operations
        .get_mut(ID)
        .unwrap()
        .phase = Phase::Ready;
    assert!(!app.cloud_prototype.production.create_agent.owns(1));
    app.prepare_production_deployment(1, None, &Context::default());
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert!(
        runtime.preparation.is_some() && runtime.receiver.is_none(),
        "the card prepares its own start once no agent operation owns it"
    );
    app.finish_cloud_preparations(&Context::default());
    assert!(
        app.cloud_prototype.production.runtimes[&1].receiver.is_none(),
        "a record that is missing deploys nothing"
    );
}

#[test]
fn a_status_answer_carries_the_stage_output_and_what_a_ready_cloud_is() {
    let (temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    let (cloud, _workspace) = ("cloud-1", workspace);
    let record = ready_record(&root);
    let runtime = app.cloud_prototype.production.runtimes.entry(1).or_default();
    runtime.stage = Some(Stage::Ready);
    runtime
        .logs
        .push_back(super::super::LogLine::new("step one".into(), None, None));
    runtime
        .logs
        .push_back(super::super::LogLine::new("step two".into(), None, None));
    let state = &mut app.cloud_prototype.production.create_agent;
    state.operations.insert(
        ID.into(),
        Operation {
            id: ID.into(),
            actor: actor.clone(),
            workspace: root.to_string_lossy().into_owned(),
            cloud: cloud.into(),
            name: "shop".into(),
            phase: Phase::Ready,
            card: Some(1),
            ready: Some(ready_facts(&record)),
            error: None,
            cancel: None,
            skip_sign_in: false,
        },
    );
    let answer = answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            status_request(ID),
            manifest::now_millis(),
        ),
    )
    .unwrap();
    assert_eq!(answer["phase"], json!("ready"));
    assert_eq!(answer["done"], json!(true));
    assert_eq!(answer["stage"], json!("Ready"));
    assert_eq!(answer["lines"], json!(["step one", "step two"]));
    assert_eq!(answer["ready"]["worker"], json!("worker-1"));
    assert_eq!(answer["ready"]["hourly"], json!(0.44));
    assert_eq!(
        answer["ready"]["image"],
        json!("ghcr.io/peters/horizon-worker-base:cpu")
    );
}

#[test]
fn a_ready_cloud_is_reported_while_its_presentation_watch_still_runs() {
    let (temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    accept(&mut app, &actor, &workspace, "cloud-1");
    app.cloud_prototype
        .production
        .create_agent
        .operations
        .get_mut(ID)
        .unwrap()
        .card = Some(1);
    let sender = app.cloud_prototype.production.create_agent.sender();
    let ctx = Context::default();
    let outcome = finish_deployment(
        Ok(ready_record(&root)),
        |ready, error| {
            assert!(
                ready.is_some() && error.is_none(),
                "a ready deployment reports what the cloud is"
            );
            sender
                .send(Message::Finished {
                    operation: ID.into(),
                    ready: ready.map(Box::new),
                    error,
                })
                .unwrap();
        },
        |_state| {
            // A live cloud's watch runs on this thread and returns only on a cancel or a
            // closed card, so the agent must already be done here.
            app.poll_cloud_create_requests(&ctx);
            let operation = &app.cloud_prototype.production.create_agent.operations[ID];
            assert_eq!(operation.phase, Phase::Ready, "the report comes before the watch");
            assert_eq!(
                operation.ready.as_ref().map(|ready| ready["worker"].clone()),
                Some(json!("worker-1"))
            );
            assert!(
                !app.cloud_prototype.production.create_agent.owns(1),
                "the card's own start paths work again once the cloud is ready"
            );
        },
    );
    assert!(outcome.is_ok());
}

#[test]
fn a_cancel_of_a_cloud_that_became_ready_answers_done() {
    let (temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    accept(&mut app, &actor, &workspace, "cloud-1");
    app.cloud_prototype
        .production
        .create_agent
        .operations
        .get_mut(ID)
        .unwrap()
        .card = Some(1);
    let sender = app.cloud_prototype.production.create_agent.sender();
    let ctx = Context::default();
    let status = build_request(
        &actor,
        manifest::host_instance(),
        status_request(ID),
        manifest::now_millis(),
    );
    let cancel = build_request(
        &actor,
        manifest::host_instance(),
        cancellation(),
        manifest::now_millis(),
    );
    let outcome = finish_deployment(
        Ok(ready_record(&root)),
        |ready, error| {
            sender
                .send(Message::Finished {
                    operation: ID.into(),
                    ready: ready.map(Box::new),
                    error,
                })
                .unwrap();
        },
        |_state| {
            // Read and cancel while the watch of the live cloud still runs.
            app.poll_cloud_create_requests(&ctx);
            let answer = app.answer_cloud_create(&status, &ctx).unwrap();
            assert_eq!(answer["phase"], json!("ready"));
            assert_eq!(answer["done"], json!(true));
            assert_eq!(answer["ready"]["worker"], json!("worker-1"));
            let refused = app.answer_cloud_create(&cancel, &ctx).unwrap_err();
            assert!(refused.starts_with("cloud_create_done"), "{refused}");
            assert_eq!(
                app.cloud_prototype.production.create_agent.operations[ID].phase,
                Phase::Ready,
                "a ready cloud stays on its card"
            );
        },
    );
    assert!(outcome.is_ok());
}

#[test]
fn a_cancel_answers_that_the_worker_may_have_been_rented() {
    let (_temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let cancel = Cancellation::default();
    let state = &mut app.cloud_prototype.production.create_agent;
    state.operations.insert(
        ID.into(),
        Operation {
            id: ID.into(),
            actor: actor.clone(),
            workspace,
            cloud: "cloud-1".into(),
            name: "shop".into(),
            phase: Phase::RollingOut,
            card: None,
            ready: None,
            error: None,
            cancel: Some(cancel.clone()),
            skip_sign_in: false,
        },
    );
    let cancel_request = build_request(
        &actor,
        manifest::host_instance(),
        json!({"operation": "cancel", "operation_id": ID}),
        manifest::now_millis(),
    );
    let cancelled = answer(&mut app, &cancel_request).unwrap();
    assert_eq!(cancelled["phase"], json!("cancelled"));
    assert_eq!(cancelled["error"], json!(CANCELLED));
    assert!(
        cancel.is_cancelled(),
        "the deployment of this operation is asked to stop"
    );
    let operation = &app.cloud_prototype.production.create_agent.operations[ID];
    assert_eq!(
        (operation.phase, operation.error.as_deref()),
        (Phase::Cancelled, Some(CANCELLED))
    );
    // A ready cloud is stopped on its card, never by a cancel of the create operation.
    app.cloud_prototype
        .production
        .create_agent
        .operations
        .get_mut(ID)
        .unwrap()
        .phase = Phase::Ready;
    let refused = answer(&mut app, &cancel_request).unwrap_err();
    assert!(refused.starts_with("cloud_create_done"), "{refused}");
}

#[test]
fn the_operation_of_another_agent_is_unknown_and_the_repository_names_the_cloud() {
    let (_temp, mut app, workspace, actor) = agent_app(Some(policy()));
    let state = &mut app.cloud_prototype.production.create_agent;
    state.operations.insert(
        OTHER.into(),
        Operation {
            id: OTHER.into(),
            actor: "horizon:another-agent".into(),
            workspace,
            cloud: "cloud-1".into(),
            name: "shop".into(),
            phase: Phase::RollingOut,
            card: None,
            ready: None,
            error: None,
            cancel: None,
            skip_sign_in: false,
        },
    );
    for operation in [
        status_request(OTHER),
        json!({"operation": "cancel", "operation_id": OTHER}),
    ] {
        let request = build_request(&actor, manifest::host_instance(), operation, manifest::now_millis());
        assert!(
            answer(&mut app, &request)
                .unwrap_err()
                .starts_with("cloud_create_unknown_operation")
        );
    }
    assert_eq!(folder_name("/home/person/shop/"), "shop");
    assert_eq!(folder_name("/home/person"), "person");
}
