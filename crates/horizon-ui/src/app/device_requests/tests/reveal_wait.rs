use std::time::{Duration, Instant};

use super::*;
use crate::app::test_support::{raw_input, run_app_frame_with_input};
use crate::device_widget::DeviceUiState;
use horizon_core::browser::manifest::device::{Connection, HostExclusion, Presentation};

/// A connected viewer far off the canvas, as after unrelated navigation.
fn off_canvas_viewer() -> (tempfile::TempDir, Context, HorizonApp, PanelId, String) {
    let (temp, ctx, mut app) = app();
    app.root_viewport_stabilizer = None;
    let create = request(
        &app,
        Operation::Create {
            endpoint: "127.0.0.1:5900".into(),
            identity: None,
            ssh: None,
        },
    );
    let local = one(app.apply_device_request(&create, &ctx)).panel_id;
    let id = app.board.panel_id_by_local_id(&local).unwrap();
    app.panel_render_caches
        .device_ui_state
        .insert(id, DeviceUiState::connected_fixture(&create.actor));
    app.board.panel_mut(id).unwrap().layout.position = [100_000.0, 5000.0];
    frame(&ctx, &mut app);
    (temp, ctx, app, id, local)
}

/// One host frame. Held reveals stay out of it so nothing is published to
/// the real runtime directory; the next frame's `begin_frame` is replayed.
fn frame(ctx: &Context, app: &mut HorizonApp) {
    let awaiting = std::mem::take(&mut app.panel_render_caches.awaiting_device_reveals);
    run_app_frame_with_input(ctx, app, raw_input([1400.0, 900.0], None));
    app.panel_render_caches.awaiting_device_reveals = awaiting;
    for state in app.panel_render_caches.device_ui_state.values_mut() {
        state.begin_frame();
    }
}

fn reveal(app: &mut HorizonApp, ctx: &Context, local: &str) -> Request {
    let reveal = request(app, Operation::Reveal { panel_id: local.into() });
    let outcome = app.apply_device_request(&reveal, ctx);
    assert!(
        app.defer_device_reveal(&reveal, outcome, None).is_none(),
        "a successful reveal answers after the host draws it"
    );
    reveal
}

/// A viewer's first frame back on the canvas can still be clipped while
/// egui settles its layout, so the answer may take a few frames.
fn frames_until_settled(ctx: &Context, app: &mut HorizonApp) -> Vec<Outcome> {
    for _ in 0..5 {
        frame(ctx, app);
        let outcomes = settled(app, Instant::now());
        if !outcomes.is_empty() {
            assert_eq!(outcomes.len(), 1);
            return outcomes;
        }
    }
    panic!("reveal did not settle within five frames");
}

fn settled(app: &mut HorizonApp, now: Instant) -> Vec<Outcome> {
    app.take_settled_device_reveals(now)
        .into_iter()
        .map(|answer| answer.outcome)
        .collect()
}

#[test]
fn reveal_answers_once_the_viewer_is_drawn() {
    let (_temp, ctx, mut app, id, local) = off_canvas_viewer();
    assert!(!app.panel_render_caches.device_ui_state[&id].was_rendered());
    reveal(&mut app, &ctx, &local);
    assert!(settled(&mut app, Instant::now()).is_empty());
    let mut outcomes = frames_until_settled(&ctx, &mut app);
    let panel = one(outcomes.remove(0));
    assert_eq!(panel.connection, Connection::Connected);
    assert!(panel.image.image_displayed);
    assert!(panel.image.frame_sequence > 0);
    let diagnostics = panel.diagnostics.unwrap();
    assert_eq!(diagnostics.presentation, Presentation::Displayed);
    let host = diagnostics.host.unwrap();
    assert_eq!(host.applied_reveal_request, 1);
    assert_eq!(host.exclusion, None);
    assert!(app.panel_render_caches.awaiting_device_reveals.is_empty());
}

#[test]
fn a_display_from_before_the_reveal_is_not_proof() {
    let (_temp, ctx, mut app, id, local) = off_canvas_viewer();
    app.board.panel_mut(id).unwrap().layout.position = [0.0, 0.0];
    for _ in 0..3 {
        frame(&ctx, &mut app);
    }
    assert!(app.device_observation(id, "any").unwrap().image.image_displayed);
    reveal(&mut app, &ctx, &local);
    assert!(
        settled(&mut app, Instant::now()).is_empty(),
        "the frame drawn before the reveal was applied must not answer it"
    );
    frames_until_settled(&ctx, &mut app);
}

#[test]
fn bounded_wait_reports_why_the_host_did_not_draw_the_viewer() {
    let (_temp, ctx, mut app, id, local) = off_canvas_viewer();
    reveal(&mut app, &ctx, &local);
    // Ordinary host navigation after the reveal takes the whole canvas.
    app.fullscreen_panel = Some(app.board.panels[1].id);
    frame(&ctx, &mut app);
    assert!(settled(&mut app, Instant::now()).is_empty());
    let mut outcomes = settled(&mut app, Instant::now() + Duration::from_secs(4));
    let panel = one(outcomes.remove(0));
    assert!(!panel.image.image_displayed);
    let diagnostics = panel.diagnostics.unwrap();
    assert_eq!(diagnostics.presentation, Presentation::NotRendered);
    let host = diagnostics.host.unwrap();
    assert_eq!(host.exclusion, Some(HostExclusion::OtherPanelFullscreen));
    assert_eq!((host.reveal_requests, host.applied_reveal_request), (1, 1));
    assert!(!app.panel_render_caches.device_ui_state[&id].was_rendered());
}

#[test]
fn stopped_and_closed_viewers_answer_without_waiting() {
    let (_temp, ctx, mut app, id, local) = off_canvas_viewer();
    let mut stopped = DeviceUiState::default();
    stopped
        .owner
        .clone_from(&app.panel_render_caches.device_ui_state[&id].owner);
    app.panel_render_caches.device_ui_state.insert(id, stopped);
    let stopped = request(
        &app,
        Operation::Reveal {
            panel_id: local.clone(),
        },
    );
    let outcome = app.apply_device_request(&stopped, &ctx);
    let answered = app
        .defer_device_reveal(&stopped, outcome, None)
        .expect("a stopped viewer answers without another frame");
    assert_eq!(one(answered).connection, Connection::Stopped);
    assert!(app.panel_render_caches.awaiting_device_reveals.is_empty());

    app.panel_render_caches
        .device_ui_state
        .insert(id, DeviceUiState::connected_fixture(&stopped.actor));

    reveal(&mut app, &ctx, &local);
    assert!(app.take_closed_device_reveals().is_empty());
    let close = request(&app, Operation::Close { panel_id: local });
    app.apply_device_request(&close, &ctx);
    let outcomes: Vec<_> = app
        .take_closed_device_reveals()
        .into_iter()
        .map(|answer| answer.outcome)
        .collect();
    assert!(matches!(&outcomes[..], [Outcome::Failed { code, .. }] if code == "panel_unavailable"));
    assert!(app.panel_render_caches.awaiting_device_reveals.is_empty());
}

#[test]
fn only_successful_reveals_are_held() {
    let (_temp, ctx, mut app, _id, local) = off_canvas_viewer();
    let inspect = request(&app, Operation::Inspect { panel_id: local });
    let outcome = app.apply_device_request(&inspect, &ctx);
    assert!(app.defer_device_reveal(&inspect, outcome, None).is_some());
    let missing = request(
        &app,
        Operation::Reveal {
            panel_id: "missing".into(),
        },
    );
    let outcome = app.apply_device_request(&missing, &ctx);
    assert!(matches!(
        app.defer_device_reveal(&missing, outcome, None),
        Some(Outcome::Failed { .. })
    ));
    assert!(app.panel_render_caches.awaiting_device_reveals.is_empty());
}

#[test]
fn a_discarded_pass_is_not_presentation_evidence() {
    let (_temp, ctx, mut app, id, local) = off_canvas_viewer();
    reveal(&mut app, &ctx, &local);
    for _ in 0..5 {
        frame(&ctx, &mut app);
        let state = app.panel_render_caches.device_ui_state.get_mut(&id).unwrap();
        state.host.finish(None, true, 0, true);
        assert!(
            settled(&mut app, Instant::now()).is_empty(),
            "egui discarded the pass that drew the viewer"
        );
    }
    frames_until_settled(&ctx, &mut app);
}

#[test]
fn a_superseded_reveal_answers_without_waiting() {
    let (_temp, ctx, mut app, first, first_local) = off_canvas_viewer();
    let create = request(
        &app,
        Operation::Create {
            endpoint: "127.0.0.1:5900".into(),
            identity: None,
            ssh: None,
        },
    );
    let second_local = one(app.apply_device_request(&create, &ctx)).panel_id;
    let second = app.board.panel_id_by_local_id(&second_local).unwrap();
    app.panel_render_caches
        .device_ui_state
        .insert(second, DeviceUiState::connected_fixture(&create.actor));
    reveal(&mut app, &ctx, &first_local);
    reveal(&mut app, &ctx, &second_local);
    frame(&ctx, &mut app);
    let answered = app.take_settled_device_reveals(Instant::now());
    let outcome = answered
        .into_iter()
        .find(|answer| matches!(&answer.request.operation, Operation::Reveal { panel_id } if *panel_id == first_local))
        .expect("the superseded reveal answers at once")
        .outcome;
    let host = one(outcome).diagnostics.unwrap().host.unwrap();
    assert_eq!((host.reveal_requests, host.applied_reveal_request), (1, 0));
    assert!(
        app.panel_render_caches.device_ui_state[&first]
            .host
            .applied_at(1)
            .is_none()
    );
}

#[test]
fn held_reveals_are_answered_before_panels_are_renumbered() {
    let (_temp, ctx, mut app, _id, local) = off_canvas_viewer();
    reveal(&mut app, &ctx, &local);
    let abandoned: Vec<_> = app
        .take_abandoned_device_reveals("Device panel closed by a session switch")
        .into_iter()
        .map(|answer| answer.outcome)
        .collect();
    assert!(matches!(&abandoned[..], [Outcome::Failed { code, .. }] if code == "panel_unavailable"));
    assert!(app.panel_render_caches.awaiting_device_reveals.is_empty());
}

#[test]
fn shutdown_answers_held_reveals_before_viewer_state_is_dropped() {
    let (_temp, ctx, mut app, id, local) = off_canvas_viewer();
    reveal(&mut app, &ctx, &local);
    let held = std::mem::take(&mut app.panel_render_caches.awaiting_device_reveals);
    app.begin_shutdown();
    assert!(app.panel_render_caches.awaiting_device_reveals.is_empty());
    assert!(!app.panel_render_caches.device_ui_state.contains_key(&id));
    // A reveal left over from before the state was dropped is not answered
    // with a defaulted, stopped viewer.
    app.panel_render_caches.awaiting_device_reveals = held;
    let outcomes = settled(&mut app, Instant::now());
    assert!(matches!(&outcomes[..], [Outcome::Failed { code, .. }] if code == "panel_unavailable"));
    let late = request(&app, Operation::Reveal { panel_id: local });
    let outcome = app.apply_device_request(&late, &ctx);
    assert!(app.defer_device_reveal(&late, outcome, None).is_some());
}

#[test]
fn a_reveal_claimed_while_no_frame_runs_still_answers_in_its_queue() {
    let root = tempfile::tempdir().expect("temp dir");
    let (_temp, ctx, mut app, _id, local) = off_canvas_viewer();
    let actor = format!("horizon:{}", app.board.panels[0].local_id);
    let identity = manifest::AgentIdentity::new(&actor, Some(manifest::host_instance()));
    let enqueue =
        |operation| device::enqueue_at(root.path(), identity, operation, Duration::from_secs(5)).expect("enqueue");
    let held = enqueue(Operation::Reveal {
        panel_id: local.clone(),
    });
    app.root_viewport_stabilizer = None;
    let bridge = crate::app::DeviceRequestBridge::with_root(root.path().to_path_buf());
    bridge.install(app, ctx);
    assert!(bridge.poll_on_ui_thread());
    assert!(
        device::take_result_at(root.path(), &held).expect("result").is_none(),
        "the reveal waits for the viewer to be drawn"
    );
    let close = enqueue(Operation::Close { panel_id: local });
    assert!(bridge.poll_on_ui_thread());
    assert!(matches!(
        device::take_result_at(root.path(), &close).expect("result"),
        Some(Outcome::Closed { .. })
    ));
    assert!(matches!(
        device::take_result_at(root.path(), &held).expect("result"),
        Some(Outcome::Failed { code, .. }) if code == "panel_unavailable"
    ));
}
