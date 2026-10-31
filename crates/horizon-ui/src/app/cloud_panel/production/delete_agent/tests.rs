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
/// The agent actor of the panel the fixture creates, once the app knows it.
type Fixture = (tempfile::TempDir, HorizonApp, String, String);

fn profile() -> Profile {
    CloudConfig::parse(
        "version: 1\ndefault: quick-start\nprofiles:\n  quick-start:\n    provider: runpod\n    image: ghcr.io/peters/horizon-worker-base:cpu\n    cpu: 2\n    memory_gb: 4\n    gpu: false\n",
    )
    .unwrap()
    .profiles["quick-start"]
        .clone()
}

fn build_request(actor: &str, host: &str, delete: Value, deadline: i64) -> UsageRequest {
    let mut value = json!({
        "request_id": "request-1",
        "actor": actor,
        "host_instance": host,
        "deadline_at_millis": deadline,
        "claimed": false,
    });
    value["cloud_delete"] = delete;
    serde_json::from_value(value).unwrap()
}

fn submit(cloud: &str) -> Value {
    json!({"operation": "submit", "operation_id": ID, "cloud": cloud})
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

/// The record of a cloud whose provider resources a previous delete already removed, as
/// `RunPod` leaves it: the worker is gone and no storage journal remains.
fn deleted_record(creator: Option<Creator>) -> Deployment {
    let mut record: Deployment = serde_json::from_value(json!({
        "version":1,"cloud_id":CLOUD,"repository":"/home/person/shop","revision":"a".repeat(40),
        "profile":profile(),"stage":"Deleted",
        "operation":{"state":"terminated","worker_id":"worker-1"},
        "spec":{"operation_id":CLOUD,"image_digest":"sha256:abc","profile":profile(),
            "public_key":"ssh-ed25519 AAAA fixture","registry_auth_id":null,
            "gpu_types":[],"cpu_flavors":["cpu3c"],"data_centers":["US-MO-2"]},
        "sessions":[]
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
    app.answer_cloud_delete(request, &ctx)
}

/// Polls until the operation ends, as a caller polls its status.
fn finish(app: &mut HorizonApp) -> Value {
    let ctx = Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        app.poll_cloud_delete_requests(&ctx);
        if app.cloud_prototype.production.delete_agent.operations[ID].phase.done() {
            return answer(
                app,
                &build_request(
                    &app.cloud_prototype.production.delete_agent.operations[ID].actor.clone(),
                    manifest::host_instance(),
                    operation_request("status"),
                    manifest::now_millis(),
                ),
            )
            .unwrap();
        }
        assert!(std::time::Instant::now() < deadline, "the delete operation did not end");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_request_from_another_host_or_without_an_agent_panel_is_unavailable() {
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(&temp.path().join("cloud"), &deleted_record(Some(creator(&actor))));
    let deadline = manifest::now_millis() + 10_000;
    let foreign = build_request(&actor, "another-host", submit(CLOUD), deadline);
    assert_eq!(answer(&mut app, &foreign).unwrap_err(), UNAVAILABLE);
    let stranger = build_request(
        "horizon:not-a-panel",
        manifest::host_instance(),
        submit(CLOUD),
        deadline,
    );
    assert_eq!(answer(&mut app, &stranger).unwrap_err(), UNAVAILABLE);
    assert!(
        app.cloud_prototype.production.delete_agent.operations.is_empty(),
        "nothing is recorded for a refused caller"
    );
}

#[test]
fn an_invalid_expired_or_unknown_submission_deletes_nothing() {
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(&temp.path().join("cloud"), &deleted_record(Some(creator(&actor))));
    let host = manifest::host_instance();
    let deadline = manifest::now_millis() + 10_000;
    let without_cloud = json!({"operation": "submit", "operation_id": ID});
    assert_eq!(
        answer(&mut app, &build_request(&actor, host, without_cloud, deadline)).unwrap_err(),
        INVALID
    );
    let expired = build_request(&actor, host, submit(CLOUD), manifest::now_millis() - 1);
    assert_eq!(answer(&mut app, &expired).unwrap_err(), EXPIRED);
    let unknown = build_request(&actor, host, submit("cloud-2"), deadline);
    assert_eq!(answer(&mut app, &unknown).unwrap_err(), UNKNOWN_CLOUD);
    let cards = app.cloud_prototype.groups.0.len();
    assert!(app.cloud_prototype.production.delete_agent.operations.is_empty());
    assert_eq!(app.cloud_prototype.groups.0.len(), cards, "no card changed");
}

#[test]
fn an_agents_own_cloud_is_deleted_and_the_record_proves_absence() {
    let (temp, mut app, _workspace, actor) = agent_app();
    let root = temp.path().join("cloud");
    save_cloud(&root, &deleted_record(Some(creator(&actor))));
    // A cloud whose provider resources an earlier delete already removed: this run drives
    // the same deletion path without a provider request, as the saved record says.
    let accepted = answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            submit(CLOUD),
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    assert_eq!(accepted["phase"], json!("submitted"));
    assert_eq!(accepted["cloud"], json!(CLOUD));
    assert_eq!(accepted["provider"], json!("runpod"));
    assert_eq!(
        accepted["in_progress"],
        json!(IN_PROGRESS),
        "the answer says a delete may already have started"
    );
    let answer = finish(&mut app);
    assert_eq!(answer["phase"], json!("done"));
    assert_eq!(answer["done"], json!(true));
    assert_eq!(answer["provider"], json!("runpod"));
    assert_eq!(answer["stage"], json!(Stage::Deleted.label()));
    assert_eq!(answer["absent"], json!(true));
    assert!(
        answer["absence"].as_str().unwrap_or_default().contains("confirm"),
        "the answer states what the record proves: {answer}"
    );
    assert_eq!(answer["error"], json!(null));
}

#[test]
fn another_agents_cloud_is_refused_by_the_lease() {
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(
        &temp.path().join("cloud"),
        &deleted_record(Some(creator("horizon:other-agent"))),
    );
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            submit(CLOUD),
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    let answer = finish(&mut app);
    assert_eq!(answer["phase"], json!("failed"));
    assert_eq!(answer["error"], json!(NOT_AN_AGENTS_CLOUD));
    assert_eq!(answer["absent"], json!(false));
    assert!(
        NOT_AN_AGENTS_CLOUD.contains("on its card"),
        "the refusal names the way that stays open to the person"
    );
}

#[test]
fn a_cloud_a_person_created_is_refused_by_the_lease() {
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(&temp.path().join("cloud"), &deleted_record(None));
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            submit(CLOUD),
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    let answer = finish(&mut app);
    assert_eq!(answer["error"], json!(NOT_AN_AGENTS_CLOUD));
}

#[test]
fn a_cloud_without_a_worker_is_deleted_and_its_card_goes() {
    let (temp, mut app, _workspace, actor) = agent_app();
    let root = temp.path().join("cloud");
    let mut record = deleted_record(Some(creator(&actor)));
    record.spec = None;
    save_cloud(&root, &record);
    let card = app.cloud_prototype.groups.0[0].issue;
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            submit(CLOUD),
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    let answer = finish(&mut app);
    assert_eq!(answer["error"], json!(null), "nothing failed: nothing was rented");
    assert!(
        !app.cloud_prototype.groups.0.iter().any(|group| group.issue == card),
        "a cloud that never requested a worker leaves no card the person must remove"
    );
    assert_eq!(
        refusal(&deleted_record(None), &actor).as_deref(),
        Some(NOT_AN_AGENTS_CLOUD)
    );
    assert_eq!(
        refusal(&record, &actor),
        None,
        "a record without a worker is removable, not refused"
    );
    assert_eq!(refusal(&deleted_record(Some(creator(&actor))), &actor), None);
}

#[test]
fn a_missing_record_without_a_journal_is_removed_and_one_with_a_journal_is_refused() {
    let (_temp, mut app, _workspace, actor) = agent_app();
    let card = app.cloud_prototype.groups.0[0].issue;
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            submit(CLOUD),
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    let done = finish(&mut app);
    assert_eq!(done["error"], json!(null), "no record and no journal: nothing remains");
    assert!(
        !app.cloud_prototype.groups.0.iter().any(|group| group.issue == card),
        "a card with nothing behind it is removed"
    );

    // A provider journal written without a deployment may still name a resource.
    let (temp, mut app, _workspace, actor) = agent_app();
    let directory = cloud_runtime::state::cloud_directory(&temp.path().join("cloud"), CLOUD).unwrap();
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("workspace-volume.json"), b"{}").unwrap();
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            submit(CLOUD),
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    let refused = finish(&mut app);
    assert_eq!(
        refused["error"],
        json!(NO_RECORD),
        "a journal without a record is not nothing"
    );
    assert!(
        app.cloud_prototype.groups.0.iter().any(|group| group.issue == card),
        "the card that can settle the journal stays"
    );
}

#[test]
fn a_busy_cloud_or_a_cloud_an_agent_create_owns_is_refused() {
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(&temp.path().join("cloud"), &deleted_record(Some(creator(&actor))));
    let request = build_request(
        &actor,
        manifest::host_instance(),
        submit(CLOUD),
        manifest::now_millis() + 10_000,
    );
    // The card's own deployment, a provider check or a preparation holds the cloud.
    let receiver = channel::<Event>().1;
    app.cloud_prototype.production.runtimes.entry(1).or_default().receiver = Some(receiver);
    assert_eq!(answer(&mut app, &request).unwrap_err(), BUSY);
    assert!(
        app.cloud_prototype.production.delete_agent.operations.is_empty(),
        "a busy cloud records no operation"
    );
    app.cloud_prototype.production.runtimes.remove(&1);
    // An agent's create operation owns the cloud it rents until it ends.
    app.cloud_prototype.production.create_agent.hold_cloud_for_test(CLOUD);
    assert_eq!(answer(&mut app, &request).unwrap_err(), AGENT_BUSY);
}

#[test]
fn a_cloud_that_already_has_a_delete_operation_is_polled_not_deleted_again() {
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(&temp.path().join("cloud"), &deleted_record(Some(creator(&actor))));
    app.cloud_prototype.production.delete_agent.operations.insert(
        OTHER.into(),
        Operation {
            id: OTHER.into(),
            ..running(&actor, Phase::Deleting)
        },
    );
    let request = build_request(
        &actor,
        manifest::host_instance(),
        submit(CLOUD),
        manifest::now_millis() + 10_000,
    );
    assert_eq!(answer(&mut app, &request).unwrap_err(), DELETE_BUSY);
}

#[test]
fn the_status_answer_reports_the_stage_provider_and_absence() {
    let (_temp, mut app, _workspace, actor) = agent_app();
    let mut operation = running(&actor, Phase::Deleting);
    operation.stage = Some(Stage::DeleteWorker);
    operation.absent = Some(Absence::MayRemain);
    app.cloud_prototype
        .production
        .delete_agent
        .operations
        .insert(ID.into(), operation);
    let reported = answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            operation_request("status"),
            manifest::now_millis(),
        ),
    )
    .unwrap();
    assert_eq!(reported["phase"], json!("deleting"));
    assert_eq!(reported["done"], json!(false));
    assert_eq!(reported["stage"], json!("Delete worker"));
    assert_eq!(reported["cloud"], json!(CLOUD));
    assert_eq!(reported["absent"], json!(false));
    assert!(
        reported["absence"].as_str().unwrap_or_default().contains("still holds"),
        "a retained journal is reported as it is: {reported}"
    );
    // Without a verdict, the answer claims no absence at all.
    let mut unconfirmed = running(&actor, Phase::Deleting);
    unconfirmed.absent = Some(Absence::Unconfirmed);
    assert!(absence_text(&unconfirmed).contains("do not confirm"));
    let running_operation = running(&actor, Phase::Deleting);
    assert_eq!(absence_text(&running_operation), IN_PROGRESS);
}

#[test]
fn a_cancel_answers_that_the_delete_may_already_have_started() {
    let (_temp, mut app, _workspace, actor) = agent_app();
    let cancel = Cancellation::default();
    let mut operation = running(&actor, Phase::Deleting);
    operation.cancel = Some(cancel.clone());
    app.cloud_prototype
        .production
        .delete_agent
        .operations
        .insert(ID.into(), operation);
    let cancelled = answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            operation_request("cancel"),
            manifest::now_millis(),
        ),
    )
    .unwrap();
    assert_eq!(cancelled["phase"], json!("cancelled"));
    assert_eq!(cancelled["error"], json!(CANCELLED));
    assert_eq!(cancelled["absent"], json!(false));
    assert!(cancel.is_cancelled(), "the deletion of this operation is asked to stop");
    // A finished delete cannot be cancelled: only its record shows what the provider kept.
    app.cloud_prototype
        .production
        .delete_agent
        .operations
        .get_mut(ID)
        .unwrap()
        .phase = Phase::Done;
    let refused = answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            operation_request("cancel"),
            manifest::now_millis(),
        ),
    )
    .unwrap_err();
    assert_eq!(refused, DONE);
}

/// An app that holds one delete operation of `actor` at `Phase::Deleting`, with the token a
/// cancel asks to stop, and no thread behind it, so one interleaving is tested at a time.
fn deleting_app() -> (tempfile::TempDir, HorizonApp, String, Cancellation) {
    let (temp, mut app, _workspace, actor) = agent_app();
    let cancel = Cancellation::default();
    let mut operation = running(&actor, Phase::Deleting);
    operation.cancel = Some(cancel.clone());
    app.cloud_prototype
        .production
        .delete_agent
        .operations
        .insert(ID.into(), operation);
    (temp, app, actor, cancel)
}

fn finish_message(
    app: &mut HorizonApp,
    state: Option<Box<Deployment>>,
    absent: Option<Absence>,
    error: Option<String>,
) {
    let sender = app.cloud_prototype.production.delete_agent.sender();
    sender
        .send(Message::Finished {
            operation: ID.into(),
            state,
            absent,
            error,
        })
        .unwrap();
    app.poll_cloud_delete_requests(&Context::default());
}

#[test]
fn a_cancel_inside_a_provider_request_reports_the_absence_the_record_proves() {
    let (_temp, mut app, actor, cancel) = deleting_app();
    let cancelled = answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            operation_request("cancel"),
            manifest::now_millis(),
        ),
    )
    .unwrap();
    assert_eq!(cancelled["phase"], json!("cancelled"));
    assert!(cancel.is_cancelled());
    assert_eq!(
        cancelled["done"],
        json!(false),
        "a cancelled delete still waits for the provider request it already sent"
    );
    assert_eq!(cancelled["absence"], json!(IN_PROGRESS));
    // The request had already gone past recall, so the deletion ran to its end: the record
    // and the provider journal prove that nothing the cloud created remains.
    finish_message(
        &mut app,
        Some(Box::new(deleted_record(Some(creator(&actor))))),
        Some(Absence::Proven),
        None,
    );
    let reported = answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            operation_request("status"),
            manifest::now_millis(),
        ),
    )
    .unwrap();
    assert_eq!(
        reported["phase"],
        json!("cancelled"),
        "the cancel is what happened to the request"
    );
    assert_eq!(reported["error"], json!(CANCELLED));
    assert_eq!(reported["done"], json!(true));
    assert_eq!(reported["stage"], json!(Stage::Deleted.label()));
    assert_eq!(
        reported["absent"],
        json!(true),
        "the record and the journal prove the absence the delete reached"
    );
    let absence = reported["absence"].as_str().unwrap_or_default();
    assert!(
        absence.contains("the cancel arrived"),
        "the answer names the cancel: {absence}"
    );
    assert!(
        absence.contains("gone past recall"),
        "the answer names the recall it missed: {absence}"
    );
    assert!(
        absence.contains("nothing it created remains"),
        "the answer names the absence the record proves: {absence}"
    );
    assert!(
        !absence.contains("stopped before"),
        "the delete did not stop, and the answer must not say it did: {absence}"
    );
}

#[test]
fn a_cancel_before_the_provider_request_reports_that_no_absence_is_proven() {
    let (_temp, mut app, actor, _cancel) = deleting_app();
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            operation_request("cancel"),
            manifest::now_millis(),
        ),
    )
    .unwrap();
    // The deletion stopped before it sent a provider request: its record never reached the
    // deleted step, so nothing proves that a provider resource is gone.
    finish_message(
        &mut app,
        None,
        Some(Absence::Unconfirmed),
        Some("cloud_delete_failed: the cloud settings could not be read".into()),
    );
    let reported = answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            operation_request("status"),
            manifest::now_millis(),
        ),
    )
    .unwrap();
    assert_eq!(reported["phase"], json!("cancelled"));
    assert_eq!(reported["done"], json!(true));
    assert_eq!(reported["absent"], json!(false));
    assert_eq!(
        reported["error"],
        json!(CANCELLED),
        "the thread's own failure does not turn the request into a failure it did not answer"
    );
    let absence = reported["absence"].as_str().unwrap_or_default();
    assert!(
        absence.contains("the cancel arrived") && absence.contains("stopped before the provider confirmed absence"),
        "the answer names the cancel and the step it reached: {absence}"
    );
    assert!(
        !absence.contains("nothing it created remains"),
        "nothing proves absence here: {absence}"
    );
    // A record the answer could not read is no verdict either, and says the same.
    let mut unread = running(&actor, Phase::Cancelled);
    unread.finished = true;
    assert_eq!(absence_text(&unread), CANCELLED_STOPPED);
}

#[test]
fn the_operation_of_another_agent_is_unknown_and_a_repeated_submit_answers_it_once() {
    let (temp, mut app, _workspace, actor) = agent_app();
    save_cloud(&temp.path().join("cloud"), &deleted_record(Some(creator(&actor))));
    app.cloud_prototype.production.delete_agent.operations.insert(
        OTHER.into(),
        Operation {
            id: OTHER.into(),
            cloud: "cloud-2".into(),
            ..running("horizon:another-agent", Phase::Deleting)
        },
    );
    for operation in ["status", "cancel"] {
        let request = build_request(
            &actor,
            manifest::host_instance(),
            json!({"operation": operation, "operation_id": OTHER}),
            manifest::now_millis(),
        );
        assert_eq!(answer(&mut app, &request).unwrap_err(), UNKNOWN_OPERATION);
    }
    let request = build_request(
        &actor,
        manifest::host_instance(),
        submit(CLOUD),
        manifest::now_millis() + 10_000,
    );
    let first = answer(&mut app, &request).unwrap();
    let again = answer(&mut app, &request).unwrap();
    assert_eq!(first["operation_id"], json!(ID));
    assert_eq!(again["operation_id"], json!(ID));
    assert_eq!(
        app.cloud_prototype.production.delete_agent.operations.len(),
        2,
        "a repeated submit adds no operation"
    );
}

/// An operation the host holds without its thread, so its answers are tested apart from
/// what a running deletion reports.
fn running(actor: &str, phase: Phase) -> Operation {
    Operation {
        id: ID.into(),
        actor: actor.into(),
        cloud: CLOUD.into(),
        provider: "runpod".into(),
        card: 1,
        phase,
        stage: None,
        absent: None,
        error: None,
        cancel: Some(Cancellation::default()),
        finished: false,
    }
}

#[test]
fn the_finished_record_reaches_the_clouds_card() {
    let (temp, mut app, _workspace, actor) = agent_app();
    let root = temp.path().join("cloud");
    save_cloud(&root, &deleted_record(Some(creator(&actor))));
    app.cloud_prototype.production.runtimes.entry(1).or_default();
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            submit(CLOUD),
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    finish(&mut app);
    let runtime = app.cloud_prototype.production.runtimes.get(&1);
    assert_eq!(
        runtime.and_then(|runtime| runtime.stage),
        Some(Stage::Deleted),
        "the card shows the cloud the delete left, so its own controls agree"
    );
    assert!(
        runtime.is_some_and(|runtime| runtime
            .state
            .as_ref()
            .is_some_and(|state| state.stage == Stage::Deleted)),
        "the card reads the saved record"
    );
}

#[test]
fn a_cancelled_operation_still_leaves_the_saved_record_on_the_card() {
    let (temp, mut app, _workspace, actor) = agent_app();
    let root = temp.path().join("cloud");
    save_cloud(&root, &deleted_record(Some(creator(&actor))));
    app.cloud_prototype.production.runtimes.entry(1).or_default();
    answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            submit(CLOUD),
            manifest::now_millis() + 10_000,
        ),
    )
    .unwrap();
    // The caller cancels, and the deletion reports afterwards what the provider did.
    let cancelled = answer(
        &mut app,
        &build_request(
            &actor,
            manifest::host_instance(),
            operation_request("cancel"),
            manifest::now_millis(),
        ),
    )
    .unwrap();
    assert_eq!(cancelled["phase"], json!("cancelled"));
    let sender = app.cloud_prototype.production.delete_agent.sender();
    sender
        .send(Message::Finished {
            operation: ID.into(),
            state: Some(Box::new(deleted_record(Some(creator(&actor))))),
            absent: Some(Absence::Proven),
            error: None,
        })
        .unwrap();
    app.poll_cloud_delete_requests(&Context::default());
    let runtime = app.cloud_prototype.production.runtimes.get(&1);
    assert_eq!(
        runtime.and_then(|runtime| runtime.stage),
        Some(Stage::Deleted),
        "the card shows the record the deletion saved, also after a cancel"
    );
    let operation = &app.cloud_prototype.production.delete_agent.operations[ID];
    assert_eq!(
        (operation.phase, operation.error.as_deref()),
        (Phase::Cancelled, Some(CANCELLED)),
        "the cancel still stands as the operation's answer"
    );
}

#[test]
fn a_card_that_holds_another_record_or_another_operation_keeps_its_view() {
    let (temp, mut app, _workspace, actor) = agent_app();
    let root = temp.path().join("cloud");
    let saved = deleted_record(Some(creator(&actor)));
    save_cloud(&root, &saved);
    let request = build_request(
        &actor,
        manifest::host_instance(),
        submit(CLOUD),
        manifest::now_millis() + 10_000,
    );
    answer(&mut app, &request).unwrap();
    let mut published = saved.clone();
    published.revision = "b".repeat(40);
    assert!(
        !saved_is_newer(&saved, Some(&published)),
        "a card that shows another revision is not overridden"
    );
    assert!(
        saved_is_newer(&saved, None),
        "a card with no record takes the saved one"
    );
    // A card that another operation holds keeps its own view until that operation ends.
    let mut shown = saved.clone();
    shown.stage = Stage::Provision;
    let runtime = app.cloud_prototype.production.runtimes.entry(1).or_default();
    runtime.state = Some(shown);
    runtime.stage = Some(Stage::Provision);
    let receiver = channel::<Event>().1;
    app.cloud_prototype.production.runtimes.get_mut(&1).unwrap().receiver = Some(receiver);
    finish(&mut app);
    assert_eq!(
        app.cloud_prototype.production.runtimes[&1].stage,
        Some(Stage::Provision),
        "a busy card is not told about the record"
    );
}

#[test]
fn an_absence_read_that_another_operation_held_is_read_once_more() {
    let attempts = std::cell::Cell::new(0);
    let read = || {
        attempts.set(attempts.get() + 1);
        if attempts.get() == 1 {
            Err(cloud_runtime::Error::Busy)
        } else {
            Ok(Absence::Proven)
        }
    };
    assert_eq!(
        read_absence(read),
        Some(Absence::Proven),
        "a record held for the moment of the first read is read again"
    );
    assert_eq!(attempts.get(), 2);
    assert_eq!(
        read_absence(|| Err(cloud_runtime::Error::Busy)),
        None,
        "a record that stays unreadable is reported as unproven, never as absent"
    );
}
