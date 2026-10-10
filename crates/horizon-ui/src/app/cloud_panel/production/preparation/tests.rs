use super::super::Stage;
use crate::app::test_support::test_app;
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};
use std::time::{Duration, Instant};

/// A named pipe stands in for a disk that does not answer: reading it waits until the
/// test writes the record.
fn stalled_record(directory: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let path = directory.join("deployment.json");
    let made = std::process::Command::new("mkfifo").arg(&path).status().unwrap();
    assert!(made.success());
    path
}

/// A started cloud whose stopped record binds a worker; the machine has no settings.
fn stopped_cloud() -> (tempfile::TempDir, crate::app::HorizonApp, serde_json::Value) {
    let (temp, mut app) = test_app();
    let root = temp.path();
    let workspace = app.board.create_workspace("cloud fixture");
    let mut group = CloudGroup::new(
        1,
        "Fixture".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        root.into(),
        [0.0, 0.0],
    );
    let profile = CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 4\n    memory_gb: 8\n",
    )
    .unwrap()
    .profiles["dev"]
    .clone();
    let record = serde_json::json!({
        "version":1,"cloud_id":"fixture","repository":root,"revision":"a".repeat(40),
        "profile":profile,"stage":"Stopped","operation":{"state":"bound","worker_id":"worker1"},
        "spec":null,"sessions":[],"worker":null
    });
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile,
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    app.cloud_prototype.groups.0.push(group);
    app.cloud_prototype.root = Some(root.into());
    (temp, app, record)
}

#[test]
fn a_record_that_does_not_answer_never_stalls_the_frame_that_deploys() {
    let (temp, mut app, record) = stopped_cloud();
    let ctx = egui::Context::default();
    let pipe = stalled_record(&temp.path().join("fixture"));

    let started = Instant::now();
    app.start_production_deployment(1, &ctx);
    app.poll_cloud_preparations(&ctx);
    let waited = started.elapsed();
    // Answer the read before any assertion can fail, so the worker thread always ends.
    let writer = std::thread::spawn(move || std::fs::write(pipe, record.to_string()).unwrap());
    assert!(
        waited < Duration::from_secs(2),
        "the frame waited {waited:?} for the record"
    );
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert!(
        runtime.preparation.is_some(),
        "the preparation still waits for the record"
    );
    assert!(runtime.busy(), "no other operation starts while it waits");
    assert!(
        runtime.receiver.is_none(),
        "nothing deploys before the record is prepared"
    );

    writer.join().unwrap();
    app.finish_cloud_preparations(&ctx);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    let error = runtime.error.as_deref().unwrap_or_default();
    assert!(error.contains("settings.json"), "{error}");
    assert_eq!(runtime.stage, Some(Stage::Validate));
    assert!(runtime.receiver.is_none() && !runtime.busy());
}

#[test]
fn a_refused_reconnect_after_a_resume_is_still_that_resume() {
    let (temp, mut app, record) = stopped_cloud();
    let ctx = egui::Context::default();
    std::fs::create_dir_all(temp.path().join("fixture")).unwrap();
    std::fs::write(temp.path().join("fixture/deployment.json"), record.to_string()).unwrap();
    app.reconnect_resumed(vec![1], &ctx);
    app.finish_cloud_preparations(&ctx);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert!(runtime.error.as_deref().unwrap_or_default().contains("settings.json"));
    assert_eq!(runtime.operation, Some(super::lifecycle::Action::Resume));
}
