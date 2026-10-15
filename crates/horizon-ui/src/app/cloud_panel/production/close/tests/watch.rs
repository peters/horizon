//! A Ready connection watch is not an operation, and cannot block explicit discard.
use super::super::super::Event;
use super::*;
use std::{path::Path, sync::mpsc};

fn ready_watch(app: &mut HorizonApp, root: &Path) -> mpsc::Sender<Event> {
    Store::lock(&root.join("fixture")).unwrap().save(&deployment()).unwrap();
    add_cloud(app, root, Some(deployment()));
    let (sender, receiver) = mpsc::channel();
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&101).unwrap();
    runtime.receiver = Some(receiver);
    runtime.cancel = Some(horizon_core::cloud_runtime::Cancellation::default());
    assert!(runtime.connected_ready());
    assert!(!runtime.busy());
    sender
}

#[test]
fn failed_delete_of_a_ready_watched_cloud_can_be_removed_anyway() {
    for settings in ["missing", "unreadable"] {
        let (temp, mut app) = test_app();
        let _watch = ready_watch(&mut app, temp.path());
        if settings == "unreadable" {
            std::fs::create_dir(temp.path().join("settings.json")).unwrap();
        }
        let record = temp.path().join("fixture/deployment.json");
        let original = std::fs::read(&record).unwrap();
        let ctx = egui::Context::default();
        app.request_cloud_close(101);
        app.settle_cloud_removals_at(&ctx);
        app.delete_for_close(101, &ctx);
        app.settle_cloud_removals_at(&ctx);
        let runtime = &app.cloud_prototype.production.runtimes[&101];
        assert!(
            runtime.connected_ready(),
            "the actual failed preflight keeps its watch: {settings}"
        );
        assert!(!runtime.busy());
        assert!(runtime.error.is_some());
        let close = &app.cloud_prototype.production.close;
        assert!(close.failed[&101].deletion_tried);
        assert!(offer::offer(runtime, true, &close.records[&101], close.failed.get(&101)).remove_anyway);
        assert_eq!(
            app.begin_cloud_removal(101, Origin::Anyway, &ctx),
            RemovalStart::Pending,
            "the connection watch cannot refuse the offered override: {settings}"
        );
        assert!(app.cloud_prototype.production.runtimes[&101].receiver.is_some());
        app.settle_cloud_removals_at(&ctx);
        assert!(
            app.cloud_prototype.groups.0.is_empty(),
            "completion must also ignore only the watch"
        );
        assert_eq!(std::fs::read(record).unwrap(), original);
    }
}

#[test]
fn remove_anyway_still_refuses_an_operation_before_dispatch_or_at_completion() {
    for phase in ["begin", "completion"] {
        let (temp, mut app) = test_app();
        let _watch = ready_watch(&mut app, temp.path());
        let record = temp.path().join("fixture/deployment.json");
        let original = std::fs::read(&record).unwrap();
        let ctx = egui::Context::default();
        if phase == "begin" {
            app.cloud_prototype.production.runtimes.get_mut(&101).unwrap().stage = Some(Stage::Stopping);
            assert_eq!(
                app.begin_cloud_removal(101, Origin::Anyway, &ctx),
                RemovalStart::Refused
            );
        } else {
            assert_eq!(
                app.begin_cloud_removal(101, Origin::Anyway, &ctx),
                RemovalStart::Pending
            );
            app.cloud_prototype.production.runtimes.get_mut(&101).unwrap().stage = Some(Stage::Stopping);
            app.settle_cloud_removals_at(&ctx);
        }
        assert!(app.cloud_prototype.production.runtimes[&101].busy());
        assert_eq!(app.cloud_prototype.groups.0.len(), 1, "{phase}");
        assert_eq!(std::fs::read(record).unwrap(), original);
    }
}

#[test]
fn watched_remove_anyway_still_checks_the_original_store_lock() {
    let (temp, mut app) = test_app();
    let _watch = ready_watch(&mut app, temp.path());
    let record = temp.path().join("fixture/deployment.json");
    let original = std::fs::read(&record).unwrap();
    let other = Store::lock(&temp.path().join("fixture")).unwrap();
    let ctx = egui::Context::default();
    assert_eq!(
        app.begin_cloud_removal(101, Origin::Anyway, &ctx),
        RemovalStart::Pending
    );
    app.settle_cloud_removals_at(&ctx);
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    assert!(
        app.cloud_prototype.production.close.failed[&101]
            .reason
            .contains("Another controller")
    );
    assert_eq!(std::fs::read(&record).unwrap(), original);
    drop(other);
    assert_eq!(
        app.begin_cloud_removal(101, Origin::Anyway, &ctx),
        RemovalStart::Pending
    );
    app.settle_cloud_removals_at(&ctx);
    assert!(app.cloud_prototype.groups.0.is_empty());
    assert_eq!(std::fs::read(record).unwrap(), original);
}
