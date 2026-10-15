//! Dismissing a completed deletion cancels its automatic close, including blocked reads.
use super::*;
use horizon_core::cloud_runtime::Error;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

fn until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "worker did not complete");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn dismissing_a_blocked_after_delete_check_keeps_the_cloud_and_panels() {
    use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
    for escape in [false, true] {
        let (temp, mut app) = test_app();
        let mut deleted = deployment();
        deleted.stage = Stage::Deleted;
        deleted.operation = CreateState::Terminated {
            worker_id: "worker".into(),
        };
        let path = temp.path().join("fixture");
        Store::lock(&path).unwrap().save(&deleted).unwrap();
        add_cloud(&mut app, temp.path(), Some(deleted.clone()));
        let panel = add_member(&mut app);
        let already_hidden = add_member(&mut app);
        app.board.panel_mut(already_hidden).unwrap().visible = false;
        app.cloud_prototype.production.close.deleting.insert(101);
        let record = path.join("deployment.json");
        std::fs::remove_file(&record).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&record)
                .status()
                .unwrap()
                .success()
        );
        let bytes = serde_json::to_vec(&deleted).unwrap();
        let (permit, permitted) = mpsc::channel();
        let (ready, reader_ready) = mpsc::channel();
        let release = std::thread::spawn(move || {
            let mut writer = None;
            until(|| {
                writer = std::fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed())
                    .open(&record)
                    .ok();
                writer.is_some()
            });
            let _ = ready.send(());
            let explicit = permitted.recv_timeout(Duration::from_secs(5)).is_ok();
            writer.as_mut().unwrap().write_all(&bytes).unwrap();
            drop(writer);
            std::fs::remove_file(&record).unwrap();
            std::fs::write(record, bytes).unwrap();
            explicit
        });
        let ctx = egui::Context::default();
        app.finish_closing_clouds(&ctx);
        reader_ready.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(Store::lock(&path), Err(Error::Busy)));
        app.request_cloud_close(101);
        for _ in 0..2 {
            let _ = ctx
                .run_ui(egui::RawInput::default(), |ui| {
                    app.render_cloud_close_confirmation(ui.ctx());
                })
                .discard_textures();
        }
        if escape {
            let _ = ctx
                .run_ui(
                    egui::RawInput {
                        events: vec![egui::Event::Key {
                            key: egui::Key::Escape,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::default(),
                        }],
                        ..Default::default()
                    },
                    |ui| app.render_cloud_close_confirmation(ui.ctx()),
                )
                .discard_textures();
        } else {
            app.end_close_confirmation(101);
        }
        let cancelled = !app.cloud_prototype.production.close.deleting.contains(&101);
        let restored = app.board.panel(panel).unwrap().visible && !app.board.is_hidden_for_disposal(panel);
        app.finish_closing_clouds(&ctx);
        permit.send(()).unwrap();
        assert!(release.join().unwrap(), "the UI returned before the fallback timeout");
        app.settle_cloud_removals_at(&ctx);
        app.finish_closing_clouds(&ctx);
        app.settle_cloud_removals_at(&ctx);
        assert_eq!(
            app.cloud_prototype.groups.0.len(),
            1,
            "dismissal cannot restart removal"
        );
        assert!(
            cancelled && restored,
            "dismissal restores panels and cancels automatic close"
        );
        assert!(app.board.panel(panel).unwrap().visible);
        assert!(!app.board.is_hidden_for_disposal(panel));
        assert!(!app.board.panel(already_hidden).unwrap().visible);
        assert!(!app.cloud_close_confirmation_open());
    }
}
