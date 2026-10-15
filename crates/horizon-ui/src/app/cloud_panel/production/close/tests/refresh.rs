//! A periodic display read cannot withdraw settled controls or authorize a final removal.
use super::*;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

fn primary(app: &HorizonApp) -> Option<Primary> {
    let close = &app.cloud_prototype.production.close;
    let launch = app.cloud_prototype.groups.0[0].remote.as_ref().unwrap();
    offer::offer(
        &app.cloud_prototype.production.runtimes[&101],
        launch.deployment_started,
        &close.records[&101],
        close.failed.get(&101),
    )
    .primary
}

fn until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "worker did not complete");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn periodic_refresh_keeps_controls_but_final_removal_checks_again() {
    use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
    let (temp, mut app) = test_app();
    let path = temp.path().join("fixture");
    let mut empty = deployment();
    empty.stage = Stage::Deleted;
    empty.operation = CreateState::Terminated {
        worker_id: "worker".into(),
    };
    Store::lock(&path).unwrap().save(&empty).unwrap();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    let ctx = egui::Context::default();
    app.request_cloud_close(101);
    app.settle_cloud_removals_at(&ctx);
    assert_eq!(primary(&app), Some(Primary::Remove));

    let record = path.join("deployment.json");
    std::fs::remove_file(&record).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&record)
            .status()
            .unwrap()
            .success()
    );
    let bytes = serde_json::to_vec(&deployment()).unwrap();
    let (permit, permitted) = mpsc::channel();
    let release = std::thread::spawn(move || {
        let explicit = permitted.recv_timeout(Duration::from_secs(5)).is_ok();
        let mut writer = None;
        until(|| {
            writer = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed())
                .open(&record)
                .ok();
            writer.is_some()
        });
        writer.as_mut().unwrap().write_all(&bytes).unwrap();
        drop(writer);
        std::fs::remove_file(&record).unwrap();
        std::fs::write(record, bytes).unwrap();
        explicit
    });
    app.cloud_prototype.production.close.record_read = None;
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            app.render_cloud_close_confirmation(ui.ctx());
        })
        .discard_textures();
    until(|| matches!(Store::lock(&path), Err(horizon_core::cloud_runtime::Error::Busy)));
    let during_refresh = primary(&app);
    app.remove_deleted_cloud(101, &ctx);
    assert_eq!(primary(&app), None, "a final decision still displays Checking");
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    permit.send(()).unwrap();
    assert!(release.join().unwrap(), "the UI returned before the fallback timeout");
    app.settle_cloud_removals_at(&ctx);
    assert_eq!(
        app.cloud_prototype.groups.0.len(),
        1,
        "fresh provider resources prevent removal"
    );
    app.request_cloud_close(101);
    app.settle_cloud_removals_at(&ctx);
    assert_eq!(primary(&app), Some(Primary::Delete));
    assert_eq!(
        during_refresh,
        Some(Primary::Remove),
        "periodic reads retain settled controls"
    );
}
