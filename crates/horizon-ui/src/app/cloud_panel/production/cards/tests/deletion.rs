use super::*;
use horizon_core::cloud_panel::CloudGroup;

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
fn manage_offers_no_reconnect_or_second_check_while_the_idle_watch_asks_the_provider() {
    use super::super::super::idle::Report;
    let ctx = egui::Context::default();
    let (reports, received) = std::sync::mpsc::channel();
    let mut runtime = ready_bound_runtime();
    runtime.idle_reports = Some(received);
    assert!(has(&action_texts(&ctx, &mut runtime), "Reconnect cloud"));
    reports.send(Report::Confirming).unwrap();
    runtime.poll_idle();
    let confirming = action_texts(&ctx, &mut runtime);
    assert!(has(&confirming, "Checking provider…"), "{confirming:?}");
    for hidden in ["Reconnect cloud", "Check provider", "Read record again"] {
        assert!(!has(&confirming, hidden), "{hidden} waits for the check");
    }
    reports.send(Report::Confirmed).unwrap();
    runtime.poll_idle();
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
