use super::*;
use horizon_core::cloud_runtime::companions::Scope;
#[cfg(unix)]
use std::process::Command;

fn owner() -> Owner {
    Owner {
        scope: Scope {
            session_id: "session".into(),
            workspace_id: "workspace".into(),
        },
        cloud_id: "source".into(),
    }
}

fn declaration() -> Declaration {
    Declaration::new("example/consumer", "cpu")
}

fn creation(app: &mut HorizonApp) -> &mut State {
    &mut app.cloud_prototype.production.companions.agent.creation
}

#[test]
fn repeated_requests_share_one_pending_creation_and_polls_read_it() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let id = OperationId::generate();
    let first = app.request_companion_creation(owner(), "consumer", declaration(), id);
    assert_eq!(first["phase"], "confirmation_required");
    assert_eq!(first["done"], false);
    assert_eq!(first["operation_id"], json!(id));
    let again = app.request_companion_creation(owner(), "consumer", declaration(), OperationId::generate());
    assert_eq!(again["operation_id"], json!(id));
    let creation = creation(&mut app);
    assert_eq!(creation.pending.len(), 1);
    let answer = |action, id| creation.answer("source", "consumer", action, id);
    assert_eq!(answer(None, id).unwrap().unwrap()["phase"], "confirmation_required");
    assert!(answer(None, OperationId::generate()).is_none());
    let ensure = answer(Some(Action::EnsureReady), OperationId::generate());
    assert_eq!(ensure.unwrap().unwrap()["operation_id"], json!(id));
    assert!(answer(Some(Action::Stop), id).unwrap().is_err());
    assert!(
        creation
            .answer("source", "other", Some(Action::EnsureReady), id)
            .is_none()
    );
}

#[test]
fn declining_forgets_the_request_and_answers_its_polls_as_refused() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    let id = OperationId::generate();
    app.request_companion_creation(owner(), "consumer", declaration(), id);
    creation(&mut app)
        .actions
        .push(("source".into(), "consumer".into(), Choice::Decline));
    app.poll_companion_creations(&ctx);
    let creation = creation(&mut app);
    assert!(creation.pending.is_empty());
    let refused = creation.answer("source", "consumer", None, id).unwrap().unwrap();
    assert_eq!(
        (refused["phase"].as_str(), refused["done"].as_bool()),
        (Some("refused"), Some(true))
    );
    // Resending the declined request stays refused.
    let resent = creation
        .answer("source", "consumer", Some(Action::EnsureReady), id)
        .unwrap()
        .unwrap();
    assert_eq!(resent["phase"], "refused");
    // The same ID from another source cloud or alias is a different request.
    assert!(creation.answer("elsewhere", "consumer", None, id).is_none());
    assert!(creation.answer("source", "other", None, id).is_none());
    // A fresh Ensure Ready after a decline asks the owner again.
    assert!(
        creation
            .answer("source", "consumer", Some(Action::EnsureReady), OperationId::generate())
            .is_none()
    );
    assert!(app.cloud_prototype.groups.0.is_empty());
}

#[test]
fn the_owner_chooses_a_checkout_through_the_picker() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    app.request_companion_creation(owner(), "consumer", declaration(), OperationId::generate());
    creation(&mut app)
        .actions
        .push(("source".into(), "consumer".into(), Choice::Browse));
    app.poll_companion_creations(&ctx);
    assert!(app.dir_picker.is_some());
    app.choose_companion_checkout("source", "consumer", Path::new("/checkouts/consumer"));
    let pending = &creation(&mut app).pending[0];
    assert_eq!(pending.chosen.as_deref(), Some(Path::new("/checkouts/consumer")));
    // Choosing never starts the creation on its own.
    assert!(pending.waiting());
    // A reservation that failed validation leaves the checkout open to correct.
    creation(&mut app).pending[0].cloud_id = Some("minted".into());
    app.choose_companion_checkout("source", "consumer", Path::new("/checkouts/fixed"));
    assert_eq!(
        creation(&mut app).pending[0].chosen.as_deref(),
        Some(Path::new("/checkouts/fixed"))
    );
}

#[test]
fn only_a_declaration_without_any_matching_cloud_is_missing() {
    let source = Target {
        scope: owner().scope,
        cloud_id: "source".into(),
        declaration: Declaration::new("example/source", "cpu"),
    };
    let mut context = Context {
        source: source.clone(),
        declarations: [("consumer".into(), declaration())].into(),
        inventory: vec![source.clone()],
    };
    assert_eq!(super::super::missing(&context, "consumer"), Some(declaration()));
    assert_eq!(super::super::missing(&context, "unknown"), None);
    // A same-worker sibling never has a cloud of its own to create.
    let sibling: Declaration = serde_json::from_value(json!({
        "repository": "example/consumer", "profile": "cpu", "placement": "same_worker"
    }))
    .unwrap();
    context.declarations.insert("sibling".into(), sibling);
    assert_eq!(super::super::missing(&context, "sibling"), None);
    context.inventory.push(Target {
        cloud_id: "target".into(),
        declaration: declaration(),
        ..source
    });
    assert_eq!(super::super::missing(&context, "consumer"), None);
}

#[cfg(unix)]
fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(path).args(args).output().unwrap();
    assert!(output.status.success(), "{args:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[cfg(unix)]
fn repository(path: &Path, origin: &str, config: &str) -> String {
    std::fs::create_dir_all(path.join(".horizon")).unwrap();
    git(path, &["init", "--quiet"]);
    git(path, &["remote", "add", "origin", origin]);
    std::fs::write(path.join(".horizon/cloud.yml"), config).unwrap();
    git(path, &["add", "."]);
    git(
        path,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "Fixture",
        ],
    );
    git(path, &["rev-parse", "HEAD"])
}

#[cfg(unix)]
const PROFILE: &str = "version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n";

#[cfg(unix)]
#[test]
fn a_confirmed_request_reserves_a_fresh_cloud_bound_to_the_checkout_and_records_the_operation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let source_repository = temp.path().join("source");
    let checkout = temp.path().join("consumer");
    let revision = repository(
        &source_repository,
        "https://github.com/example/source.git",
        &format!("{PROFILE}companions:\n  consumer:\n    repository: example/consumer\n    profile: cpu\n"),
    );
    repository(&checkout, "https://github.com/example/consumer.git", PROFILE);
    let mut group = horizon_core::cloud_panel::CloudGroup::new(
        1,
        "Source".into(),
        "workspace".into(),
        source_repository,
        [0.0, 0.0],
    );
    group.remote = Some(
        serde_json::from_value(json!({
            "id": "source", "revision": revision, "profile_name": "cpu",
            "profile": {"provider": "runpod", "image": "example.invalid/worker", "cpu": 4, "memory_gb": 8}
        }))
        .unwrap(),
    );
    let groups = CloudGroups(vec![group]);
    let id = OperationId::generate();
    // Another repository is refused before anything is reserved.
    let wrong = Declaration::new("example/other", "cpu");
    let cloud_id = cloud_runtime::new_id();
    assert!(reserve(&root, &owner(), &groups, ("consumer", &wrong), &checkout, &cloud_id, id).is_err());
    let found = reserve(
        &root,
        &owner(),
        &groups,
        ("consumer", &declaration()),
        &checkout,
        &cloud_id,
        id,
    )
    .unwrap();
    assert_eq!(found.repository, checkout.canonicalize().unwrap());
    // A retry of the same reservation records nothing twice.
    reserve(
        &root,
        &owner(),
        &groups,
        ("consumer", &declaration()),
        &checkout,
        &cloud_id,
        id,
    )
    .unwrap();
    // Another cloud for the same alias is never reserved alongside it.
    let other = cloud_runtime::new_id();
    let error = reserve(
        &root,
        &owner(),
        &groups,
        ("consumer", &declaration()),
        &checkout,
        &other,
        id,
    )
    .unwrap_err();
    assert!(error.contains("another cloud"), "{error}");
    let context = inventory::prepare(&owner(), &groups, &Cancellation::default()).unwrap();
    let request = lifecycle::Request {
        root: &root,
        owner: &owner(),
        context: &context,
        alias: "consumer",
    };
    let operation = lifecycle::status(&request, id).unwrap();
    assert_eq!(operation.intent.target_cloud_id, cloud_id);
    assert_eq!(operation.phase, lifecycle::Phase::Submitted);
    // Nothing exists for the reserved cloud until the card creates it.
    assert!(!root.join(&cloud_id).join("deployment.json").exists());
    // Horizon closed before adding the card: the next request offers it again from
    // the recorded checkout.
    let (_app_temp, mut app) = crate::app::test_support::test_app();
    app.cloud_prototype.root = Some(root.clone());
    app.cloud_prototype.groups = groups.clone();
    app.cloud_prototype.production.companions.sync(Some("session"), &groups);
    let submitted = super::super::Submitted {
        operation,
        context,
        alias: "consumer".into(),
    };
    let (answer, started) = app.continue_operation("source", submitted, &egui::Context::default());
    assert!(!started);
    assert_eq!(answer["phase"], "confirmation_required");
    let pending = &creation(&mut app).pending[0];
    assert_eq!(pending.cloud_id.as_deref(), Some(cloud_id.as_str()));
    assert_eq!(pending.chosen, Some(checkout.canonicalize().unwrap()));
    assert!(pending.waiting() && pending.card.is_none());
}

#[cfg(unix)]
#[test]
fn a_matching_checkout_in_the_workspace_is_found_and_chosen() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let temp = tempfile::tempdir().unwrap();
    let checkout = temp.path().join("consumer");
    let revision = repository(&checkout, "https://github.com/example/consumer.git", PROFILE);
    let unrelated = temp.path().join("other");
    repository(&unrelated, "https://github.com/example/other.git", PROFILE);
    // Clouds in the workspace point at their checkouts; one is the companion's repository.
    for (issue, cwd) in [(1, &unrelated), (2, &checkout)] {
        let mut group = horizon_core::cloud_panel::CloudGroup::new(
            issue,
            format!("cloud {issue}"),
            "workspace".into(),
            cwd.clone(),
            [0.0, 0.0],
        );
        group.remote = Some(
            serde_json::from_value(json!({
                "id": format!("cloud-{issue}"), "revision": revision, "profile_name": "other",
                "profile": {"provider": "runpod", "image": "example.invalid/worker", "cpu": 4, "memory_gb": 8}
            }))
            .unwrap(),
        );
        app.cloud_prototype.groups.0.push(group);
    }
    let ctx = egui::Context::default();
    app.request_companion_creation(owner(), "consumer", declaration(), OperationId::generate());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while creation(&mut app).pending[0].search.is_some() && std::time::Instant::now() < deadline {
        app.poll_companion_creations(&ctx);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let pending = &creation(&mut app).pending[0];
    let found = checkout.canonicalize().unwrap();
    assert_eq!(pending.checkouts, std::slice::from_ref(&found));
    // A single match is chosen; the owner still decides with Create.
    assert_eq!(pending.chosen, Some(found));
    assert!(pending.waiting());
}

#[test]
fn a_checked_cloud_that_never_started_is_offered_to_the_owner_to_start() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    let launch = |id: &str| -> horizon_core::cloud_panel::CloudLaunch {
        serde_json::from_value(json!({
            "id": id, "revision": "a".repeat(40), "profile_name": "cpu",
            "profile": {"provider": "runpod", "image": "example.invalid/worker", "cpu": 4, "memory_gb": 8}
        }))
        .unwrap()
    };
    for (issue, id) in [(1, "source"), (2, "target")] {
        let mut group = horizon_core::cloud_panel::CloudGroup::new(
            issue,
            id.into(),
            "workspace".into(),
            format!("/checkouts/{id}").into(),
            [0.0, 0.0],
        );
        group.remote = Some(launch(id));
        app.cloud_prototype.groups.0.push(group);
    }
    let groups = app.cloud_prototype.groups.clone();
    app.cloud_prototype.production.companions.sync(Some("session"), &groups);
    let context = Context {
        source: Target {
            scope: owner().scope,
            cloud_id: "source".into(),
            declaration: Declaration::new("example/source", "cpu"),
        },
        declarations: [("consumer".into(), declaration())].into(),
        inventory: Vec::new(),
    };
    let operation = lifecycle::Operation {
        intent: horizon_core::cloud_runtime::companions::intent::Intent {
            operation_id: OperationId::generate(),
            action: Action::EnsureReady,
            target_cloud_id: "target".into(),
            state: horizon_core::cloud_runtime::companions::intent::State::Submitted,
        },
        phase: lifecycle::Phase::ConfirmationRequired,
    };
    let id = operation.intent.operation_id;
    let submitted = super::super::Submitted {
        operation,
        context,
        alias: "consumer".into(),
    };
    let (answer, started) = app.continue_operation("source", submitted, &ctx);
    assert!(!started);
    assert_eq!(answer["phase"], "confirmation_required");
    assert_eq!(
        (answer["done"].as_bool(), answer["operation_id"].clone()),
        (Some(false), json!(id))
    );
    let pending = &creation(&mut app).pending[0];
    assert!(pending.existing && pending.waiting());
    assert_eq!(pending.card, Some(2));
    assert_eq!(pending.cloud_id.as_deref(), Some("target"));
    assert_eq!(pending.chosen.as_deref(), Some(Path::new("/checkouts/target")));
}

#[test]
fn a_decline_after_the_card_was_added_names_the_kept_cloud() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    let id = OperationId::generate();
    app.request_companion_creation(owner(), "consumer", declaration(), id);
    let pending = &mut creation(&mut app).pending[0];
    // A later step failed after the reservation and the card.
    pending.cloud_id = Some("reserved".into());
    pending.checkout = Some(Checkout {
        repository: "/checkouts/consumer".into(),
        revision: "a".repeat(40),
        profile: serde_json::from_value(
            json!({"provider": "runpod", "image": "example.invalid/worker", "cpu": 4, "memory_gb": 8}),
        )
        .unwrap(),
    });
    pending.card = Some(7);
    // Polls name the reserved cloud once the reservation is recorded.
    assert_eq!(pending.describe()["target_cloud_id"], "reserved");
    creation(&mut app)
        .actions
        .push(("source".into(), "consumer".into(), Choice::Decline));
    app.poll_companion_creations(&ctx);
    let refused = creation(&mut app)
        .answer("source", "consumer", None, id)
        .unwrap()
        .unwrap();
    assert_eq!(refused["target_cloud_id"], "reserved");
    assert!(refused["message"].as_str().unwrap().contains("card stays"));
}
