//! The card a companion creation adds or reuses: a never-started cloud's Start offer,
//! and finding the reserved cloud's card again on a retry.
use super::tests::{creation, declaration, owner};
use super::*;

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
    let submitted = || super::super::Submitted {
        operation: operation.clone(),
        context: context.clone(),
        alias: "consumer".into(),
    };
    // The run that found the card never started ended on its own; the agent's next
    // status poll is what puts the offer on the card.
    let status: super::super::UsageRequest = serde_json::from_value(json!({
        "request_id": "companion", "actor": "horizon:agent", "host_instance": "host",
        "deadline_at_millis": i64::MAX, "claimed": true,
        "cloud_companion": {"action": "status", "cloud": "source", "alias": "consumer", "operation_id": id}
    }))
    .unwrap();
    let (answer, started) = app.answer_submitted(&status, "source", submitted(), &ctx);
    assert!(!started);
    assert_eq!(answer["phase"], "confirmation_required");
    assert_eq!(
        (answer["done"].as_bool(), answer["operation_id"].clone()),
        (Some(false), json!(id))
    );
    // Ensure Ready sent again shows the same offer.
    let (answer, started) = app.continue_operation("source", submitted(), &ctx);
    assert!(!started);
    assert_eq!(answer["operation_id"], json!(id));
    assert_eq!(creation(&mut app).pending.len(), 1);
    let pending = &creation(&mut app).pending[0];
    assert!(pending.existing && pending.waiting());
    assert_eq!(pending.card, Some(2));
    assert_eq!(pending.cloud_id.as_deref(), Some("target"));
    assert_eq!(pending.chosen.as_deref(), Some(Path::new("/checkouts/target")));
}

#[test]
fn a_retry_finds_the_reserved_cloud_by_id_not_by_a_remembered_card_number() {
    let (temp, mut app) = crate::app::test_support::test_app();
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    let ctx = egui::Context::default();
    // Card 7 was removed while the request waited, and its number went to another cloud.
    let mut other =
        horizon_core::cloud_panel::CloudGroup::new(7, "other".into(), "workspace".into(), "/other".into(), [0.0, 0.0]);
    other.remote = Some(
        serde_json::from_value(json!({
            "id": "other", "revision": "a".repeat(40), "profile_name": "cpu",
            "profile": {"provider": "runpod", "image": "example.invalid/worker", "cpu": 4, "memory_gb": 8}
        }))
        .unwrap(),
    );
    app.cloud_prototype.groups.0.push(other);
    app.request_companion_creation(owner(), "consumer", declaration(), OperationId::generate());
    let workspace = app.board.create_workspace("companions");
    let local = app.board.workspace(workspace).unwrap().local_id.clone();
    let pending = &mut creation(&mut app).pending[0];
    pending.owner.scope.workspace_id = local;
    pending.cloud_id = Some("reserved".into());
    pending.recorded = true;
    pending.checkout = Some(Checkout {
        repository: "/checkouts/consumer".into(),
        revision: "a".repeat(40),
        profile: serde_json::from_value(
            json!({"provider": "runpod", "image": "example.invalid/worker", "cpu": 4, "memory_gb": 8}),
        )
        .unwrap(),
    });
    pending.card = Some(7);
    app.add_companion_cloud(0, &ctx);
    let card = app
        .cloud_prototype
        .groups
        .0
        .iter()
        .find(|group| group.remote.as_ref().is_some_and(|launch| launch.id == "reserved"))
        .map(|group| group.issue)
        .expect("the reserved cloud gets its own card again");
    assert_ne!(card, 7);
    assert_eq!(creation(&mut app).pending[0].card, Some(card));
    // A retry reuses that card instead of adding another.
    app.add_companion_cloud(0, &ctx);
    let cards = app
        .cloud_prototype
        .groups
        .0
        .iter()
        .filter(|group| group.remote.as_ref().is_some_and(|launch| launch.id == "reserved"))
        .count();
    assert_eq!(cards, 1);
}
