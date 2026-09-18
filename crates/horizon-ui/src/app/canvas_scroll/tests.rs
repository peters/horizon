use egui::{Context, Event, Modifiers, MouseWheelUnit, Pos2, RawInput, TouchPhase, Vec2, ViewportId};
use horizon_core::{RuntimeState, StartupDecision};
use tempfile::TempDir;

use super::super::HorizonApp;
use super::super::test_support::{editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup};
use super::{ScrollGesture, route_canvas_scroll};
use crate::test_egui::DiscardTextures;

fn assert_offset(actual: [f32; 2], expected: [f32; 2]) {
    assert!(
        (Vec2::from(actual) - Vec2::from(expected)).length() < 0.001,
        "offset {actual:?}, expected {expected:?}"
    );
}

fn app_fixture() -> (TempDir, Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState {
            workspaces: vec![editor_workspace_state("synthetic", [40.0, 80.0])],
            ..RuntimeState::default()
        }),
    });
    app.theme_applied = true;
    app.initial_pan_done = true;
    app.root_viewport_stabilizer = None;
    for time in [0.0, 0.016, 0.032] {
        let mut input = raw_input([1400.0, 900.0], None);
        input.time = Some(time);
        let _ = run_app_frame_with_input(&ctx, &mut app, input);
    }
    app.pan_target = None;
    (temp, ctx, app)
}

fn scroll_frame(time: f64, position: Pos2, delta: Vec2, phase: TouchPhase) -> RawInput {
    let mut input = raw_input([1400.0, 900.0], None);
    input.time = Some(time);
    input.events = vec![Event::PointerMoved(position), wheel(delta, phase)];
    input
}

fn wheel(delta: Vec2, phase: TouchPhase) -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta,
        phase,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn scroll_keeps_panning_when_a_panel_moves_under_the_pointer() {
    let (_temp, ctx, mut app) = app_fixture();
    let panel_rect = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)[0]
        .1
        .screen_rect;
    let pointer = panel_rect.center_top() - Vec2::new(0.0, 2.0);
    let before = app.canvas_view.pan_offset;
    for time in [1.0, 1.016, 1.032] {
        let _ = run_app_frame_with_input(
            &ctx,
            &mut app,
            scroll_frame(time, pointer, Vec2::new(0.0, -5.0), TouchPhase::Move),
        );
        assert!(app.canvas_pan_input_claimed);
        assert!(ctx.input(|input| {
            !input
                .events
                .iter()
                .any(|event| matches!(event, Event::MouseWheel { .. }))
        }));
        assert!(ctx.input(|input| input.smooth_scroll_delta == Vec2::ZERO));
        assert!(
            !app.terminal_keyboard_events
                .iter()
                .any(|input| matches!(input.event, Event::MouseWheel { .. }))
        );
    }
    assert_offset(app.canvas_view.pan_offset, [before[0], before[1] - 15.0]);
}

#[test]
fn opposite_deltas_still_belong_to_the_canvas_gesture() {
    let (_temp, ctx, mut app) = app_fixture();
    let panel_rect = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)[0]
        .1
        .screen_rect;
    let pointer = panel_rect.center_top() - Vec2::new(0.0, 2.0);
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        scroll_frame(1.0, pointer, Vec2::new(0.0, -5.0), TouchPhase::Move),
    );
    let before = app.canvas_view.pan_offset;
    let mut input = scroll_frame(1.016, pointer, Vec2::new(0.0, -5.0), TouchPhase::Move);
    input.events.push(wheel(Vec2::new(0.0, 5.0), TouchPhase::Move));
    let _ = run_app_frame_with_input(&ctx, &mut app, input);
    assert_offset(app.canvas_view.pan_offset, before);
    assert!(app.canvas_pan_input_claimed);
    assert!(ctx.input(|input| {
        !input
            .events
            .iter()
            .any(|event| matches!(event, Event::MouseWheel { .. }))
    }));
    assert!(
        !app.terminal_keyboard_events
            .iter()
            .any(|input| matches!(input.event, Event::MouseWheel { .. }))
    );
}

#[test]
fn ending_gestures_consume_their_tail_without_stealing_a_new_contact() {
    for phase in [TouchPhase::End, TouchPhase::Cancel] {
        for restart_on_panel in [false, true] {
            let (_temp, ctx, mut app) = app_fixture();
            let panel_rect = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)[0]
                .1
                .screen_rect;
            let pointer = panel_rect.center_top() - Vec2::new(0.0, 2.0);
            let _ = run_app_frame_with_input(
                &ctx,
                &mut app,
                scroll_frame(1.0, pointer, Vec2::new(0.0, -5.0), TouchPhase::Move),
            );
            let before = app.canvas_view.pan_offset;
            let mut input = scroll_frame(1.016, pointer, Vec2::new(0.0, -5.0), TouchPhase::Move);
            input.events.push(wheel(Vec2::ZERO, phase));
            let expected_wheels = if restart_on_panel {
                vec![
                    wheel(Vec2::ZERO, TouchPhase::Start),
                    wheel(Vec2::new(0.0, 3.0), TouchPhase::Move),
                ]
            } else {
                Vec::new()
            };
            input.events.extend(expected_wheels.clone());
            let _ = run_app_frame_with_input(&ctx, &mut app, input);
            assert_offset(app.canvas_view.pan_offset, before);
            assert!(!app.canvas_pan_input_claimed);
            let wheels = ctx.input(|input| {
                input
                    .events
                    .iter()
                    .filter(|event| matches!(event, Event::MouseWheel { .. }))
                    .cloned()
                    .collect::<Vec<_>>()
            });
            assert_eq!(wheels, expected_wheels);
            let terminal_wheels = app
                .terminal_keyboard_events
                .iter()
                .filter(|input| matches!(input.event, Event::MouseWheel { .. }))
                .map(|input| input.event.clone())
                .collect::<Vec<_>>();
            assert_eq!(terminal_wheels, expected_wheels);
        }
    }
}

#[test]
fn panel_scroll_does_not_become_a_pan_until_a_new_gesture() {
    let (_temp, ctx, mut app) = app_fixture();
    let panel_rect = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)[0]
        .1
        .screen_rect;
    let before = app.canvas_view.pan_offset;
    for (time, position) in [(1.0, panel_rect.center()), (1.016, Pos2::new(1200.0, 700.0))] {
        let _ = run_app_frame_with_input(
            &ctx,
            &mut app,
            scroll_frame(time, position, Vec2::new(0.0, -5.0), TouchPhase::Move),
        );
        assert!(!app.canvas_pan_input_claimed);
        assert_offset(app.canvas_view.pan_offset, before);
    }
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        scroll_frame(1.5, Pos2::new(1200.0, 700.0), Vec2::new(0.0, -5.0), TouchPhase::Move),
    );
    assert_offset(app.canvas_view.pan_offset, [before[0], before[1] - 5.0]);
}

#[test]
fn coalesced_panel_end_and_canvas_start_only_pan_by_the_new_gesture() {
    let (_temp, ctx, mut app) = app_fixture();
    let panel_rect = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)[0]
        .1
        .screen_rect;
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        scroll_frame(1.0, panel_rect.center(), Vec2::ZERO, TouchPhase::Start),
    );
    let before = app.canvas_view.pan_offset;
    let pointer = panel_rect.center_top() - Vec2::new(0.0, 20.0);
    let mut input = scroll_frame(1.016, pointer, Vec2::new(0.0, -5.0), TouchPhase::Move);
    input.events.extend([
        wheel(Vec2::ZERO, TouchPhase::End),
        wheel(Vec2::ZERO, TouchPhase::Start),
        wheel(Vec2::new(0.0, -3.0), TouchPhase::Move),
    ]);
    let _ = run_app_frame_with_input(&ctx, &mut app, input);
    assert_offset(app.canvas_view.pan_offset, [before[0], before[1] - 3.0]);
    let wheels = ctx.input(|input| {
        input
            .events
            .iter()
            .filter(|event| matches!(event, Event::MouseWheel { .. }))
            .cloned()
            .collect::<Vec<_>>()
    });
    assert_eq!(
        wheels,
        vec![
            wheel(Vec2::new(0.0, -5.0), TouchPhase::Move),
            wheel(Vec2::ZERO, TouchPhase::End)
        ]
    );
}

fn claim(gesture: &mut ScrollGesture, time: f64, on_canvas: bool, events: Vec<Event>) -> bool {
    let input = egui::InputState::default().begin_pass(
        RawInput {
            time: Some(time),
            events,
            ..RawInput::default()
        },
        false,
        1.0,
        egui::InputOptions::default(),
    );
    gesture.route(&input, on_canvas, true).pans_canvas
}

#[test]
fn explicit_touch_phases_keep_ownership_across_long_gaps() {
    let mut gesture = ScrollGesture::default();
    assert!(!claim(
        &mut gesture,
        1.0,
        true,
        vec![wheel(Vec2::ZERO, TouchPhase::Start)]
    ));
    assert!(claim(
        &mut gesture,
        2.0,
        false,
        vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]
    ));
    assert!(!claim(
        &mut gesture,
        2.1,
        false,
        vec![wheel(Vec2::ZERO, TouchPhase::End)]
    ));
    assert!(!claim(
        &mut gesture,
        2.116,
        false,
        vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]
    ));
}

#[test]
fn a_new_contact_in_the_end_frame_uses_its_own_target() {
    let mut gesture = ScrollGesture::default();
    assert!(claim(
        &mut gesture,
        1.0,
        true,
        vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]
    ));
    assert!(!claim(
        &mut gesture,
        1.016,
        false,
        vec![
            wheel(Vec2::ZERO, TouchPhase::End),
            wheel(Vec2::ZERO, TouchPhase::Start),
            wheel(Vec2::new(0.0, -5.0), TouchPhase::Move),
        ]
    ));
}

#[test]
fn zero_delta_events_do_not_extend_an_x11_gesture() {
    let mut gesture = ScrollGesture::default();
    assert!(claim(
        &mut gesture,
        1.0,
        true,
        vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]
    ));
    assert!(!claim(
        &mut gesture,
        1.14,
        false,
        vec![wheel(Vec2::ZERO, TouchPhase::Move)]
    ));
    assert!(!claim(
        &mut gesture,
        1.2,
        false,
        vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]
    ));
}

#[test]
fn focus_loss_and_disallowed_input_release_the_gesture() {
    for focused in [true, false] {
        let mut gesture = ScrollGesture::default();
        assert!(claim(
            &mut gesture,
            1.0,
            true,
            vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]
        ));
        let mut input = egui::InputState::default();
        input.focused = focused;
        assert!(!gesture.route(&input, false, !focused).pans_canvas);
        assert!(!claim(
            &mut gesture,
            1.016,
            false,
            vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]
        ));
    }
}

#[test]
fn root_and_detached_viewports_have_separate_scroll_owners() {
    let ctx = Context::default();
    let detached = ViewportId::from_hash_of("detached-scroll-test");
    for (viewport_id, starts_on_canvas, expected) in [
        (ViewportId::ROOT, true, true),
        (detached, false, false),
        (ViewportId::ROOT, false, true),
    ] {
        let mut input = scroll_frame(1.0, Pos2::new(900.0, 600.0), Vec2::new(0.0, -5.0), TouchPhase::Move);
        input.viewport_id = viewport_id;
        input.viewports.entry(viewport_id).or_default();
        let _ = ctx
            .run_ui(input, |ui| {
                assert_eq!(
                    route_canvas_scroll(ui.ctx(), starts_on_canvas, true).pans_canvas,
                    expected
                );
            })
            .discard_textures();
    }
}
