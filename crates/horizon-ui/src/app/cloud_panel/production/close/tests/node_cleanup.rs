use super::*;
use horizon_core::cloud_runtime::{
    self,
    tailnet::receipt::{EnrollmentLedger, ManualCleanupReason},
};

fn pending_node() -> Deployment {
    let mut state = deployment();
    state.stage = Stage::Deleted;
    state.operation = CreateState::Terminated {
        worker_id: "worker".into(),
    };
    state.tailnet_enrollment = serde_json::from_value(serde_json::json!({"receipts":[{
        "tailnet_id":"work", "provider":"runpod", "worker_id":"worker",
        "allocation_request_id":"fixture", "device_id":"n-exact"
    }]}))
    .unwrap();
    state
}

fn api_failure() -> String {
    cloud_runtime::Error::TailnetNodeCleanup {
        tailnet_id: "work".into(),
        device_id: "n-exact".into(),
        reason: "the saved desktop API credential is unavailable",
    }
    .to_string()
}

#[test]
fn normal_remove_rechecks_locked_ledger_despite_a_stale_confirmed_snapshot() {
    let (temp, mut app) = test_app();
    let path = temp.path().join("fixture");
    let state = pending_node();
    let store = Store::lock(&path).unwrap();
    store.save(&state).unwrap();
    let before = std::fs::read(path.join("deployment.json")).unwrap();
    let mut cached = state.clone();
    let receipt = cached.tailnet_enrollment.receipts()[0].clone();
    cached.tailnet_enrollment.confirm_removed(&receipt).unwrap();
    add_cloud(&mut app, temp.path(), Some(cached));
    let ctx = egui::Context::default();
    assert_eq!(app.remove_deleted_cloud(101, &ctx), RemovalStart::Pending);
    app.settle_cloud_removals_at(&ctx);
    assert_eq!(
        app.cloud_prototype.groups.0.len(),
        1,
        "the held record must refuse removal"
    );
    drop(store);
    app.cloud_prototype.production.runtimes.get_mut(&101).unwrap().error = Some(api_failure());
    assert_eq!(app.remove_deleted_cloud(101, &ctx), RemovalStart::Pending);
    app.settle_cloud_removals_at(&ctx);
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    let runtime = &app.cloud_prototype.production.runtimes[&101];
    assert_eq!(runtime.error.as_deref(), Some(api_failure().as_str()));
    assert!(super::super::super::cards::node_cleanup_pending(runtime));
    assert_eq!(std::fs::read(path.join("deployment.json")).unwrap(), before);
}

#[test]
fn automatic_close_keeps_the_card_and_original_api_reason_when_only_provider_deletion_completed() {
    let (temp, mut app) = test_app();
    let path = temp.path().join("fixture");
    let state = pending_node();
    Store::lock(&path).unwrap().save(&state).unwrap();
    let before = std::fs::read(path.join("deployment.json")).unwrap();
    add_cloud(&mut app, temp.path(), Some(state));
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&101).unwrap();
    runtime.operation = Some(super::super::Action::Delete);
    runtime.error = Some(api_failure());
    app.cloud_prototype.production.close.deleting.insert(101);
    let ctx = egui::Context::default();
    app.finish_closing_clouds(&ctx);
    app.settle_cloud_removals_at(&ctx);
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    assert_eq!(
        app.cloud_prototype.production.runtimes[&101].error.as_deref(),
        Some(api_failure().as_str())
    );
    assert!(app.cloud_prototype.production.close.deleting.is_empty());
    assert_eq!(app.cloud_prototype.production.close.confirming, Some(101));
    assert_eq!(std::fs::read(path.join("deployment.json")).unwrap(), before);
}

#[test]
fn a_selected_legacy_cloud_without_device_identity_stays_manual_after_close() {
    let (temp, mut app) = test_app();
    let path = temp.path().join("fixture");
    let mut state = pending_node();
    state.tailnet_enrollment = EnrollmentLedger::default();
    Store::lock(&path).unwrap().save(&state).unwrap();
    cloud_runtime::tailnet::Selection {
        tailnet: Some("work".into()),
    }
    .commit(&path)
    .unwrap();
    let before = std::fs::read(path.join("deployment.json")).unwrap();
    add_cloud(&mut app, temp.path(), Some(state));
    app.cloud_prototype.production.close.deleting.insert(101);
    let ctx = egui::Context::default();
    app.finish_closing_clouds(&ctx);
    app.settle_cloud_removals_at(&ctx);
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    let runtime = &app.cloud_prototype.production.runtimes[&101];
    assert_eq!(
        runtime.error.as_deref(),
        Some(ManualCleanupReason::DeviceIdentityUnavailable.message())
    );
    assert!(super::super::super::cards::node_cleanup_pending(runtime));
    assert_eq!(std::fs::read(path.join("deployment.json")).unwrap(), before);
}

#[test]
fn explicit_owner_remove_anyway_retains_the_original_pending_record() {
    let (temp, mut app) = test_app();
    let path = temp.path().join("fixture");
    let state = pending_node();
    Store::lock(&path).unwrap().save(&state).unwrap();
    let before = std::fs::read(path.join("deployment.json")).unwrap();
    add_cloud(&mut app, temp.path(), Some(state));
    app.cloud_prototype.production.runtimes.get_mut(&101).unwrap().error = Some(api_failure());
    app.request_cloud_close(101);
    let ctx = egui::Context::default();
    app.settle_cloud_removals_at(&ctx);
    let mut warning_visible = false;
    for _ in 0..2 {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.render_cloud_close_confirmation(ui.ctx());
        });
        warning_visible |= output.shapes.iter().any(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) => text.galley.text().contains("hides its retry controls"),
            _ => false,
        });
        output.textures_delta.clear();
    }
    assert!(warning_visible);
    let store = Store::lock(&path).unwrap();
    app.remove_cloud_anyway(101, &ctx);
    app.settle_cloud_removals_at(&ctx);
    assert_eq!(
        app.cloud_prototype.groups.0.len(),
        1,
        "explicit discard must still respect the existing lifecycle lock"
    );
    drop(store);
    app.remove_cloud_anyway(101, &ctx);
    app.settle_cloud_removals_at(&ctx);
    assert!(app.cloud_prototype.groups.0.is_empty());
    assert_eq!(std::fs::read(path.join("deployment.json")).unwrap(), before);
    assert!(
        Store::lock(&path)
            .unwrap()
            .load()
            .unwrap()
            .unwrap()
            .tailnet_enrollment
            .cleanup_pending()
    );
}

#[test]
fn a_different_saved_node_pair_does_not_retain_an_earlier_api_failure() {
    let (temp, mut app) = test_app();
    let state = pending_node();
    Store::lock(&temp.path().join("fixture")).unwrap().save(&state).unwrap();
    add_cloud(&mut app, temp.path(), Some(state));
    let unrelated = api_failure().replace("device n-exact:", "device n-other:");
    app.cloud_prototype.production.runtimes.get_mut(&101).unwrap().error = Some(unrelated.clone());
    let ctx = egui::Context::default();
    app.remove_deleted_cloud(101, &ctx);
    app.settle_cloud_removals_at(&ctx);
    let error = app.cloud_prototype.production.runtimes[&101].error.as_deref().unwrap();
    assert_ne!(error, unrelated);
    assert!(error.contains("device n-exact:"));
    assert!(!error.contains("device n-other:"));
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
}
