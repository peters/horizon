#[cfg(unix)]
use super::super::super::{Event, lifecycle::CleanupRetry};
use super::*;
use horizon_core::cloud_panel::CloudGroup;
#[cfg(unix)]
use horizon_core::cloud_runtime::tailnet::receipt::{EnrollmentLedger, ManualCleanupReason};

/// Texts drawn by one frame of the runtime actions.
fn action_texts(ctx: &egui::Context, runtime: &mut super::super::super::Runtime) -> Vec<String> {
    ctx.run_ui(egui::RawInput::default(), |ui| {
        runtime_actions(ui, 1, runtime);
    })
    .discard_textures()
    .shapes
    .iter()
    .filter_map(|shape| match &shape.shape {
        egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
        _ => None,
    })
    .collect()
}

/// A Ready cloud with a bound, running worker.
fn ready_bound_runtime() -> super::super::super::Runtime {
    let state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"delete-fixture","repository":"/synthetic","revision":"a",
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
        "stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},"spec":null,
        "worker":{"id":"worker1","name":"delete-fixture","imageName":"registry.example/worker","desiredStatus":"RUNNING","costPerHr":0.74},
        "sessions":[]
    }))
    .unwrap();
    super::super::super::Runtime {
        stage: Some(Stage::Ready),
        state: Some(state),
        ..Default::default()
    }
}

fn has(texts: &[String], wanted: &str) -> bool {
    texts.iter().any(|text| text.starts_with(wanted))
}

#[test]
fn deleted_resources_keep_manual_node_message_and_offer_cleanup_retry() {
    let ctx = egui::Context::default();
    let mut runtime = ready_bound_runtime();
    runtime.stage = Some(Stage::Deleted);
    runtime.operation = Some(Action::Delete);
    runtime.error = Some("Tailnet node n-exact needs manual removal; API is offline".into());
    runtime.state.as_mut().unwrap().tailnet_enrollment = serde_json::from_value(serde_json::json!({"receipts":[{
        "tailnet_id":"work","provider":"runpod","worker_id":"worker1",
        "allocation_request_id":"delete-fixture","device_id":"n-exact"
    }]}))
    .unwrap();
    let texts = action_texts(&ctx, &mut runtime);
    assert!(has(&texts, "Retry node cleanup"));
    assert!(has(&texts, "Tailnet node n-exact"));
    runtime.error = Some(super::super::super::DELETED_RESOURCES_MESSAGE.into());
    let ledger = &mut runtime.state.as_mut().unwrap().tailnet_enrollment;
    ledger.confirm_removed(&ledger.receipts()[0].clone()).unwrap();
    assert!(!has(&action_texts(&ctx, &mut runtime), "Retry node cleanup"));
    runtime.operation = Some(Action::Deploy);
    assert!(!has(&action_texts(&ctx, &mut runtime), "Retry node cleanup"));
}

#[cfg(unix)]
fn valid_retry_settings(root: &std::path::Path) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "runpod_key_file": root.join("runpod"),
        "ssh_identity_file": root.join("ssh"),
        "docker_config": root.join("docker"),
        "registry_pull_auth_id": null, "cpu_flavors": [], "gpu_types": []
    }))
    .unwrap()
}

#[cfg(unix)]
struct DelayedRetrySettings {
    reading: std::sync::mpsc::Receiver<()>,
    permit: std::sync::mpsc::Sender<()>,
    writer: std::thread::JoinHandle<bool>,
}

#[cfg(unix)]
impl DelayedRetrySettings {
    fn start(settings: std::path::PathBuf, bytes: Vec<u8>) -> Self {
        use std::{
            io::Write as _,
            os::unix::fs::OpenOptionsExt as _,
            sync::mpsc,
            time::{Duration, Instant},
        };
        let (ready, reading) = mpsc::channel();
        let (permit, permitted) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut file = loop {
                if let Ok(file) = std::fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed())
                    .open(&settings)
                {
                    break file;
                }
                assert!(Instant::now() < deadline, "settings reader did not start");
                std::thread::sleep(Duration::from_millis(1));
            };
            ready.send(()).unwrap();
            let ui_returned = permitted.recv_timeout(Duration::from_secs(5)).is_ok();
            file.write_all(&bytes).unwrap();
            ui_returned
        });
        Self {
            reading,
            permit,
            writer,
        }
    }

    fn wait_for_reader(&self) {
        self.reading.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    }

    fn finish(self) {
        self.permit.send(()).unwrap();
        assert!(
            self.writer.join().unwrap(),
            "Retry waited for settings on the UI thread"
        );
    }
}

#[cfg(unix)]
fn pending_retry_ledger() -> EnrollmentLedger {
    serde_json::from_value(serde_json::json!({"receipts":[{
        "tailnet_id":"work", "provider":"runpod", "worker_id":"worker1",
        "allocation_request_id":"fixture", "device_id":"n-pending-fixture"
    }]}))
    .unwrap()
}

#[cfg(unix)]
struct CleanupRetryFixture {
    temp: tempfile::TempDir,
    app: crate::app::HorizonApp,
    ctx: egui::Context,
    original: Vec<u8>,
    selection: Vec<u8>,
    legacy: bool,
}

#[cfg(unix)]
impl CleanupRetryFixture {
    fn new(legacy: bool) -> Self {
        let (temp, mut app) = crate::app::test_support::test_app();
        let ctx = egui::Context::default();
        app.prepare_production_clouds(&ctx);
        resumable_cloud(&mut app, false);
        app.cloud_prototype.root = Some(temp.path().to_owned());
        let mut runtime = ready_bound_runtime();
        runtime.stage = Some(Stage::Deleted);
        runtime.error = legacy.then(|| ManualCleanupReason::DeviceIdentityUnavailable.message().into());
        let state = runtime.state.as_mut().unwrap();
        state.cloud_id = "fixture".into();
        state.stage = Stage::Deleted;
        state.operation = cloud_runtime::CreateState::Terminated {
            worker_id: "worker1".into(),
        };
        state.worker = None;
        state.browserstack_released = true;
        state.tailnet_enrollment = if legacy {
            EnrollmentLedger::default()
        } else {
            pending_retry_ledger()
        };
        let cloud_root = temp.path().join("fixture");
        cloud_runtime::state::Store::lock(&cloud_root)
            .unwrap()
            .save(state)
            .unwrap();
        cloud_runtime::tailnet::Selection {
            tailnet: Some("work".into()),
        }
        .commit(&cloud_root)
        .unwrap();
        let original = std::fs::read(cloud_root.join("deployment.json")).unwrap();
        let selection = std::fs::read(cloud_root.join("tailnet.json")).unwrap();
        app.cloud_prototype.production.runtimes.insert(7, runtime);
        Self {
            temp,
            app,
            ctx,
            original,
            selection,
            legacy,
        }
    }

    fn first_delete_events(&mut self) -> std::sync::mpsc::Sender<Event> {
        let mut ready = ready_bound_runtime();
        ready.state.as_mut().unwrap().cloud_id = "fixture".into();
        assert_eq!(ready.stage, Some(Stage::Ready));
        assert!(ready.cleanup_retry.is_none());
        ready.progress.begin_deletion();
        ready.operation = Some(Action::Delete);
        ready.stage = Some(Stage::DeleteWorker);
        let (sender, receiver) = std::sync::mpsc::channel();
        ready.receiver = Some(receiver);
        self.app.cloud_prototype.production.runtimes.insert(7, ready);
        sender
    }

    fn delayed_retry(&mut self, bytes: Vec<u8>, cancel_read: bool) {
        let settings = self.temp.path().join("settings.json");
        if !settings.exists() {
            assert!(
                std::process::Command::new("mkfifo")
                    .arg(&settings)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let delayed = DelayedRetrySettings::start(settings, bytes);
        self.app.apply_card_action(7, Action::Delete, &self.ctx);
        delayed.wait_for_reader();
        let runtime = self.app.cloud_prototype.production.runtimes.get_mut(&7).unwrap();
        assert!(runtime.busy());
        assert!(has(&action_texts(&self.ctx, runtime), "Deleting cloud resources"));
        let cancel = runtime.cancel.clone().unwrap();
        self.app.apply_card_action(7, Action::Delete, &self.ctx);
        if cancel_read {
            cancel.cancel();
            assert!(
                self.app.cloud_prototype.production.runtimes[&7]
                    .cancel
                    .as_ref()
                    .unwrap()
                    .is_cancelled()
            );
        }
        delayed.finish();
    }

    fn settle(&mut self) {
        use std::time::{Duration, Instant};
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.app.cloud_prototype.production.runtimes[&7].receiver.is_some() {
            self.app.prepare_production_clouds(&self.ctx);
            assert!(
                Instant::now() < deadline,
                "normal app polling did not settle cleanup Retry"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!self.app.cloud_prototype.production.runtimes[&7].busy());
    }

    fn assert_bytes(&self) {
        let root = self.temp.path().join("fixture");
        assert_eq!(std::fs::read(root.join("deployment.json")).unwrap(), self.original);
        assert_eq!(std::fs::read(root.join("tailnet.json")).unwrap(), self.selection);
    }

    fn assert_retained(&mut self, expected_error: &str) {
        self.settle();
        let runtime = self.app.cloud_prototype.production.runtimes.get_mut(&7).unwrap();
        assert_eq!(runtime.stage, Some(Stage::Deleted));
        assert!(runtime.error.as_deref().unwrap().contains(expected_error));
        assert!(node_cleanup_pending(runtime));
        let texts = action_texts(&self.ctx, runtime);
        assert!(has(&texts, "Retry node cleanup"));
        if self.legacy {
            assert!(has(&texts, ManualCleanupReason::DeviceIdentityUnavailable.message()));
        }
        let status = super::super::status::of(
            runtime,
            super::super::status::Occupancy::default(),
            std::time::SystemTime::now(),
        );
        if runtime.state_unavailable {
            assert_eq!(status.tone, super::super::status::Tone::Failed);
        } else {
            assert_eq!(status.tone, super::super::status::Tone::Attention);
            assert_eq!(status.verb, "Tailnet node cleanup pending");
        }
        self.assert_bytes();
    }
}

#[cfg(unix)]
#[test]
fn cleanup_retry_keeps_the_ui_responsive_during_settings_read_and_retains_failures() {
    for legacy in [false, true] {
        for cancel_read in [false, true] {
            let mut fixture = CleanupRetryFixture::new(legacy);
            for _ in 0..2 {
                let bytes = if cancel_read {
                    valid_retry_settings(fixture.temp.path())
                } else {
                    b"{".to_vec()
                };
                fixture.delayed_retry(bytes, cancel_read);
                fixture.assert_retained(if cancel_read {
                    "cancelled"
                } else {
                    "Invalid cloud state or settings"
                });
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn cleanup_retry_retains_deleted_after_a_locked_record_and_the_next_settings_read_stays_background() {
    for legacy in [false, true] {
        let mut fixture = CleanupRetryFixture::new(legacy);
        let store = cloud_runtime::state::Store::lock(&fixture.temp.path().join("fixture")).unwrap();
        fixture.delayed_retry(valid_retry_settings(fixture.temp.path()), false);
        fixture.assert_retained(&cloud_runtime::Error::Busy.to_string());
        assert_eq!(store.load().unwrap().unwrap().stage, Stage::Deleted);
        drop(store);
        fixture.delayed_retry(valid_retry_settings(fixture.temp.path()), true);
        fixture.assert_retained("cancelled");
    }
}

#[cfg(unix)]
#[test]
fn first_deletion_snapshot_survives_a_contended_reload_and_keeps_the_next_retry_background() {
    let mut fixture = CleanupRetryFixture::new(false);
    let store = cloud_runtime::state::Store::lock(&fixture.temp.path().join("fixture")).unwrap();
    let sender = fixture.first_delete_events();
    sender.send(Event::stage(Stage::DeleteWorker)).unwrap();
    sender
        .send(Event::Snapshot(Box::new(store.load().unwrap().unwrap())))
        .unwrap();
    let error = cloud_runtime::Error::TailnetNodeCleanup {
        tailnet_id: "work".into(),
        device_id: "n-pending-fixture".into(),
        reason: "the node API is unavailable",
    }
    .to_string();
    sender.send(Event::failed(error.clone())).unwrap();
    assert!(matches!(
        cloud_runtime::state::Store::lock(store.root()),
        Err(cloud_runtime::Error::Busy)
    ));
    fixture.assert_retained(&error);
    let runtime = &fixture.app.cloud_prototype.production.runtimes[&7];
    assert_eq!(runtime.cleanup_retry, Some(CleanupRetry::Recorded));
    assert!(runtime.progress.ended_in(Stage::Deleted).is_some());
    assert_eq!(store.load().unwrap().unwrap().stage, Stage::Deleted);
    drop(store);
    fixture.delayed_retry(valid_retry_settings(fixture.temp.path()), true);
    fixture.assert_retained("cancelled");
}

#[cfg(unix)]
#[test]
fn first_deletion_final_snapshot_clears_confirmed_cleanup_without_a_reload() {
    let mut fixture = CleanupRetryFixture::new(false);
    let store = cloud_runtime::state::Store::lock(&fixture.temp.path().join("fixture")).unwrap();
    let sender = fixture.first_delete_events();
    let mut saved = store.load().unwrap().unwrap();
    sender.send(Event::Snapshot(Box::new(saved.clone()))).unwrap();
    let receipt = saved.tailnet_enrollment.receipts()[0].clone();
    saved.tailnet_enrollment.confirm_removed(&receipt).unwrap();
    store.save(&saved).unwrap();
    fixture.original = std::fs::read(store.root().join("deployment.json")).unwrap();
    sender.send(Event::Snapshot(Box::new(saved))).unwrap();
    sender.send(Event::deleted()).unwrap();
    assert!(matches!(
        cloud_runtime::state::Store::lock(store.root()),
        Err(cloud_runtime::Error::Busy)
    ));
    fixture.settle();
    let runtime = fixture.app.cloud_prototype.production.runtimes.get_mut(&7).unwrap();
    assert_eq!(runtime.stage, Some(Stage::Deleted));
    assert!(runtime.cleanup_retry.is_none());
    assert!(!node_cleanup_pending(runtime));
    assert!(!has(&action_texts(&fixture.ctx, runtime), "Retry node cleanup"));
    assert!(runtime.state.as_ref().unwrap().tailnet_enrollment.receipts()[0].node_deleted);
    fixture.assert_bytes();
}

#[cfg(unix)]
#[test]
fn cleanup_retry_retains_deleted_when_its_launch_identity_is_invalid() {
    for legacy in [false, true] {
        let mut fixture = CleanupRetryFixture::new(legacy);
        fixture.app.cloud_prototype.groups.0[0].remote.as_mut().unwrap().id = "../foreign".into();
        assert!(!fixture.temp.path().join("settings.json").exists());
        for _ in 0..2 {
            fixture.app.apply_card_action(7, Action::Delete, &fixture.ctx);
            assert!(fixture.app.cloud_prototype.production.runtimes[&7].receiver.is_none());
            fixture.app.prepare_production_clouds(&fixture.ctx);
            fixture.assert_retained("Invalid cloud identity");
            assert!(fixture.app.cloud_prototype.production.runtimes[&7].state_unavailable);
            assert!(!fixture.temp.path().join("settings.json").exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn cleanup_retry_display_hint_yields_to_fresh_lifecycle_facts_without_granting_removal() {
    for event_kind in 0..6 {
        let mut fixture = CleanupRetryFixture::new(true);
        let at = std::time::Instant::now();
        let runtime = fixture.app.cloud_prototype.production.runtimes.get_mut(&7).unwrap();
        runtime.cleanup_retry = Some(CleanupRetry::Legacy);
        runtime.stage = Some(Stage::DeleteWorker);
        runtime.operation = Some(Action::Delete);
        runtime.progress.begin_deletion();
        runtime.error = None;
        let mut fresh = runtime.state.clone().unwrap();
        fresh.stage = Stage::Ready;
        let event = match event_kind {
            0 => Event::stage(Stage::Ready),
            1 => Event::Snapshot(Box::new(fresh)),
            2 => Event::Ready(Box::new(fresh), at),
            3 | 5 => {
                fresh.stage = Stage::Deleted;
                fresh.tailnet_enrollment = pending_retry_ledger();
                if event_kind == 3 {
                    let receipt = fresh.tailnet_enrollment.receipts()[0].clone();
                    fresh.tailnet_enrollment.confirm_removed(&receipt).unwrap();
                }
                Event::Snapshot(Box::new(fresh))
            }
            _ => Event::deleted(),
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        runtime.receiver = Some(receiver);
        sender.send(event).unwrap();
        sender.send(Event::failed("A later unrelated failure".into())).unwrap();
        fixture.settle();
        let runtime = fixture.app.cloud_prototype.production.runtimes.get_mut(&7).unwrap();
        assert_eq!(
            runtime.stage,
            Some(if event_kind < 3 { Stage::Ready } else { Stage::Deleted })
        );
        assert_eq!(
            runtime.cleanup_retry,
            (event_kind == 5).then_some(CleanupRetry::Recorded)
        );
        assert_eq!(node_cleanup_pending(runtime), event_kind == 5);
        let texts = action_texts(&fixture.ctx, runtime);
        assert_eq!(has(&texts, "Retry node cleanup"), event_kind == 5);
        assert!(!has(&texts, ManualCleanupReason::DeviceIdentityUnavailable.message()));
        fixture.app.remove_deleted_cloud(7, &fixture.ctx);
        fixture.app.settle_cloud_removals_at(&fixture.ctx);
        assert_eq!(
            fixture.app.cloud_prototype.groups.0.len(),
            1,
            "cached facts cannot remove the original selected legacy record"
        );
        fixture.assert_bytes();
    }
}

#[cfg(unix)]
#[test]
fn cleanup_retry_legacy_hint_survives_an_empty_deleted_snapshot_before_failure() {
    let mut fixture = CleanupRetryFixture::new(true);
    let runtime = fixture.app.cloud_prototype.production.runtimes.get_mut(&7).unwrap();
    runtime.cleanup_retry = Some(CleanupRetry::Legacy);
    runtime.stage = Some(Stage::DeleteWorker);
    runtime.error = None;
    let (sender, receiver) = std::sync::mpsc::channel();
    runtime.receiver = Some(receiver);
    sender
        .send(Event::Snapshot(Box::new(runtime.state.clone().unwrap())))
        .unwrap();
    sender
        .send(Event::failed("A settings or local-state failure".into()))
        .unwrap();
    fixture.assert_retained("A settings or local-state failure");
}

#[test]
fn deletion_replaces_deployment_actions_with_its_steps_and_total_time() {
    use horizon_core::cloud_runtime::{Cancellation, progress::Progress};
    use std::time::{Duration, Instant};
    let ctx = egui::Context::default();
    let mut runtime = ready_bound_runtime();
    let ready = action_texts(&ctx, &mut runtime);
    for shown in ["Delete cloud resources…", "Reconnect cloud", "Stop worker…"] {
        assert!(has(&ready, shown), "{shown} is shown before deletion");
    }
    // Rate and steps live in the header, Cost and Overview; Manage holds the actions.
    assert!(!has(&ready, "Provision worker"));

    let (_sender, receiver) = std::sync::mpsc::channel();
    runtime.receiver = Some(receiver);
    runtime.cancel = Some(Cancellation::default());
    runtime.stage = Some(Stage::ReleaseDevices);
    let pending = action_texts(&ctx, &mut runtime);
    assert!(pending.iter().any(|text| text == "Deleting cloud resources"));
    assert!(
        has(&pending, "Cancel operation"),
        "nothing irreversible was requested yet"
    );

    runtime.stage = Some(Stage::DeleteWorker);
    runtime.progress.stage(Stage::DeleteWorker, Instant::now());
    runtime
        .progress
        .update(Progress::activity("Deleting the worker and confirming its removal"));
    let deleting = action_texts(&ctx, &mut runtime);
    for shown in [
        "Deleting cloud resources · 0m",
        "Release hosted devices",
        "Delete worker · 0m",
        "Delete workspace storage",
        "Deleting the worker and confirming its removal",
    ] {
        assert!(has(&deleting, shown), "{shown} is shown while deleting");
    }
    for hidden in [
        "Cancel operation",
        "Delete cloud resources…",
        "Delete resources permanently",
        "Reconnect cloud",
        "Deploy cloud",
        "Stop worker…",
        "Worker rate",
        "Provision worker",
        "Check provider",
        "Redeploy cloud…",
    ] {
        assert!(!has(&deleting, hidden), "{hidden} is hidden while deleting");
    }

    // A failed deletion keeps its steps, the failed one red, and offers a retry.
    runtime.receiver = None;
    runtime.stage = Some(Stage::Ready);
    runtime.progress.finish(Instant::now());
    runtime.error = Some("Provider transport failed; reconcile before retrying".into());
    let failed = action_texts(&ctx, &mut runtime);
    for shown in ["Provider transport failed", "Delete cloud resources…"] {
        assert!(has(&failed, shown), "{shown} is shown after a failed deletion");
    }
    assert!(!has(&failed, "Provision worker"));
    assert!(!has(&failed, "Deleting cloud resources"));
    let status = super::super::status::of(
        &runtime,
        super::super::status::Occupancy::default(),
        std::time::SystemTime::now(),
    );
    assert_eq!(status.verb, "Deletion failed");
    assert_eq!(status.track.stages, &Stage::DELETION);
    assert_eq!(status.track.current, Some(1), "stopped at Delete worker");
    assert!(status.track.failed);
    assert!(
        runtime.progress.stage_duration(Stage::DeleteWorker).is_some(),
        "its time is frozen"
    );

    runtime.stage = Some(Stage::Deleted);
    runtime.progress.reset();
    let start = Instant::now().checked_sub(Duration::from_secs(30)).unwrap();
    runtime.progress.stage(Stage::DeleteWorker, start);
    runtime.progress.stage(Stage::Deleted, start + Duration::from_secs(12));
    let deleted = action_texts(&ctx, &mut runtime);
    for shown in [
        super::super::super::DELETED_RESOURCES_MESSAGE,
        "Deleted in 0m 12s",
        "Redeploy cloud…",
        "Remove cloud",
    ] {
        assert!(
            deleted.iter().any(|text| text == shown),
            "{shown} is shown after deletion"
        );
    }
}

#[test]
fn deletion_that_fails_before_its_first_step_keeps_its_steps() {
    // Without a requested worker, core fails before reporting a step and the saved
    // deployment stage comes back, but the card still presents a failed deletion.
    let ctx = egui::Context::default();
    let mut runtime = ready_bound_runtime();
    runtime.progress.begin_deletion();
    runtime.stage = Some(Stage::Provision);
    runtime.error = Some("No worker was requested".into());
    let early = action_texts(&ctx, &mut runtime);
    assert!(has(&early, "No worker was requested"));
    assert!(!has(&early, "Provision worker"));
    let status = super::super::status::of(
        &runtime,
        super::super::status::Occupancy::default(),
        std::time::SystemTime::now(),
    );
    assert_eq!(status.verb, "Deletion failed");
    assert_eq!(
        status.track.stages,
        &Stage::DELETION,
        "the deletion's steps, not the deployment's"
    );
    assert_eq!(status.track.current, Some(0));
    assert!(status.track.failed);
}

#[test]
fn a_stop_confirmation_is_the_only_thing_manage_shows_until_answered() {
    let ctx = egui::Context::default();
    let mut runtime = ready_bound_runtime();
    let idle = action_texts(&ctx, &mut runtime);
    assert!(idle.iter().any(|text| text == "Stop worker…"));
    assert!(!confirming_stop(&runtime));

    runtime.confirmation = Confirmation::Stop;
    assert!(confirming_stop(&runtime));
    let asking = action_texts(&ctx, &mut runtime);
    for shown in ["Stop this worker?", "Stop worker", "Keep running"] {
        assert!(has(&asking, shown), "{shown} is asked: {asking:?}");
    }
    for hidden in ["Reconnect cloud", "Delete cloud resources", "Rebuild image"] {
        assert!(!has(&asking, hidden), "{hidden} waits for the answer: {asking:?}");
    }
    assert!(
        !asking.iter().any(|text| text == "Stop worker…"),
        "the question replaces the button"
    );
}

#[test]
fn answering_the_stop_question_ends_it_so_a_failed_stop_shows_its_error() {
    use egui::{Event, PointerButton, RawInput};
    let ctx = egui::Context::default();
    let mut runtime = ready_bound_runtime();
    runtime.confirmation = Confirmation::Stop;
    let mut frame = |events: Vec<Event>| {
        let mut asked = None;
        let output = ctx
            .run_ui(
                RawInput {
                    events,
                    ..RawInput::default()
                },
                |ui| asked = runtime_actions(ui, 1, &mut runtime),
            )
            .discard_textures();
        let stop = output.shapes.iter().find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "Stop worker" => {
                Some(text.visual_bounding_rect().center())
            }
            _ => None,
        });
        (asked, stop)
    };
    let (_, stop) = frame(Vec::new());
    let stop = stop.expect("the question offers Stop worker");
    let press = |pressed| Event::PointerButton {
        pos: stop,
        button: PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let _ = frame(vec![Event::PointerMoved(stop)]);
    let _ = frame(vec![press(true)]);
    let (asked, _) = frame(vec![press(false)]);
    assert_eq!(asked, Some(Action::Stop), "the click asks for the stop");
    assert!(
        runtime.confirmation == Confirmation::None,
        "an answered question is over"
    );
    runtime.error = Some("Could not load cloud settings".into());
    let after = action_texts(&ctx, &mut runtime);
    assert!(
        has(&after, "Could not load cloud settings"),
        "the failure is shown: {after:?}"
    );
    assert!(after.iter().any(|text| text == "Stop worker…"), "Manage is back");
}

fn resumable_cloud(app: &mut crate::app::HorizonApp, with_panel: bool) {
    let workspace = app.board.create_workspace("Fixture");
    let mut group = CloudGroup::new(
        7,
        "Fixture".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        ".".into(),
        [0.0, 0.0],
    );
    group.remote = Some(CloudLaunch {
        publishing_selection: None,
        deployment_started: true,
        id: "fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: ready_bound_runtime().state.unwrap().profile,
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    if with_panel {
        let panel = app
            .board
            .create_panel(
                horizon_core::PanelOptions {
                    kind: horizon_core::PanelKind::Editor,
                    position: Some([14.0, 120.0]),
                    size: Some([120.0, 100.0]),
                    ..Default::default()
                },
                workspace,
            )
            .unwrap();
        group.attach(&mut app.board, panel);
    }
    app.cloud_prototype.groups.0.push(group);
    let runtime = super::super::super::Runtime {
        drawer: Some(Tab::Manage),
        ..Default::default()
    };
    app.cloud_prototype.production.runtimes.insert(7, runtime);
}

#[test]
fn resuming_shows_the_steps_instead_of_manage() {
    let ctx = egui::Context::default();
    let (_temp, mut app) = crate::app::test_support::test_app();
    resumable_cloud(&mut app, true);
    app.apply_card_action(7, Action::Resume, &ctx);
    assert_eq!(
        app.cloud_prototype.production.runtimes[&7].drawer,
        Some(Tab::Status),
        "with panels the steps open in Status"
    );

    let (_temp, mut app) = crate::app::test_support::test_app();
    resumable_cloud(&mut app, false);
    app.apply_card_action(7, Action::Resume, &ctx);
    assert_eq!(
        app.cloud_prototype.production.runtimes[&7].drawer, None,
        "without panels the steps are the body, so the drawer gives way"
    );

    let (_temp, mut app) = crate::app::test_support::test_app();
    resumable_cloud(&mut app, true);
    app.cloud_prototype.groups.0[0].set_collapsed(&mut app.board, true);
    app.apply_card_action(7, Action::Resume, &ctx);
    assert!(
        !app.cloud_prototype.groups.0[0].collapsed,
        "a collapsed cloud opens to show it"
    );
    assert_eq!(app.cloud_prototype.production.runtimes[&7].drawer, Some(Tab::Status));
}

#[test]
fn a_pending_stop_replaces_the_whole_manage_drawer() {
    use super::scrolling::{frame, label_pos, verbose_card};
    let (_temp, ctx, mut app) = verbose_card();
    let ready = ready_bound_runtime();
    {
        let runtime = app.cloud_prototype.production.runtimes.entry(901).or_default();
        runtime.stage = ready.stage;
        runtime.state = ready.state;
        runtime.drawer = Some(Tab::Manage);
    }
    let mut output = frame(&ctx, &mut app, 0.0, Pos2::ZERO, 0.0);
    for step in 1..4 {
        output = frame(&ctx, &mut app, f64::from(step) * 0.02, Pos2::ZERO, 0.0);
    }
    for shown in ["View", "Full screen", "Stop worker…"] {
        assert!(
            label_pos(&output, shown).is_some(),
            "{shown} is in the usual Manage tab"
        );
    }
    // Casting is Linux-only, and so is the row that opens it.
    assert_eq!(
        label_pos(&output, "Cast…").is_some(),
        cfg!(target_os = "linux"),
        "Cast… is beside Full screen where casting works"
    );

    app.cloud_prototype
        .production
        .runtimes
        .entry(901)
        .or_default()
        .confirmation = Confirmation::Stop;
    for step in 4..8 {
        output = frame(&ctx, &mut app, f64::from(step) * 0.02, Pos2::ZERO, 0.0);
    }
    for shown in ["Stop worker", "Keep running"] {
        assert!(label_pos(&output, shown).is_some(), "{shown} is asked");
    }
    for hidden in [
        "View",
        "Full screen",
        "Cast…",
        "Cloud",
        "Stop worker…",
        "Reconnect cloud",
    ] {
        assert!(
            label_pos(&output, hidden).is_none(),
            "{hidden} waits behind the question in the real Manage drawer"
        );
    }
}

#[test]
fn manage_offers_no_reconnect_or_second_check_while_a_provider_check_runs() {
    let ctx = egui::Context::default();
    let mut runtime = ready_bound_runtime();
    assert!(has(&action_texts(&ctx, &mut runtime), "Reconnect cloud"));
    let (_check, received) = std::sync::mpsc::channel();
    runtime.recovery_receiver = Some(received);
    let confirming = action_texts(&ctx, &mut runtime);
    assert!(has(&confirming, "Checking provider…"), "{confirming:?}");
    for hidden in ["Reconnect cloud", "Check provider", "Read record again"] {
        assert!(!has(&confirming, hidden), "{hidden} waits for the check");
    }
    runtime.recovery_receiver = None;
    assert!(has(&action_texts(&ctx, &mut runtime), "Reconnect cloud"));
}

#[test]
fn manage_keeps_the_layouts_only_for_a_header_too_narrow_to_show_them() {
    use super::scrolling::{frame, label_pos, verbose_card};
    for (width, in_manage) in [(1200.0, false), (560.0, true)] {
        let (_temp, ctx, mut app) = verbose_card();
        let ready = ready_bound_runtime();
        {
            let runtime = app.cloud_prototype.production.runtimes.entry(901).or_default();
            runtime.stage = ready.stage;
            runtime.state = ready.state;
            runtime.drawer = Some(Tab::Manage);
            // Connected, so the cloud is Ready and its layouts apply.
            runtime.receiver = Some(std::sync::mpsc::channel().1);
        }
        app.cloud_prototype.groups.0[0].size[0] = width;
        let mut output = frame(&ctx, &mut app, 0.0, Pos2::ZERO, 0.0);
        for step in 1..4 {
            output = frame(&ctx, &mut app, f64::from(step) * 0.02, Pos2::ZERO, 0.0);
        }
        assert_eq!(label_pos(&output, "Layout").is_some(), in_manage, "at {width}");
        if in_manage {
            assert!(
                label_pos(&output, "Rows").is_some(),
                "Manage offers the layouts at {width}"
            );
        }
    }
}
