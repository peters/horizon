use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use horizon_core::{RuntimeState, StartupDecision};
use tempfile::TempDir;

use super::super::HorizonApp;
use super::super::test_support::{editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup};

fn fixture() -> (TempDir, Context, HorizonApp) {
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
        let _ = run_app_frame_with_input(&ctx, &mut app, frame(time, Vec::new(), Modifiers::NONE));
    }
    app.pan_target = None;
    (temp, ctx, app)
}

fn frame(time: f64, events: Vec<Event>, modifiers: Modifiers) -> RawInput {
    let mut input = raw_input([1400.0, 900.0], None);
    input.time = Some(time);
    input.events = vec![Event::ModifiersChanged(modifiers)];
    input.events.extend(events);
    input
}

fn button(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    }
}

fn assert_offset(actual: [f32; 2], expected: Vec2) {
    assert!(
        (Vec2::from(actual) - expected).length() < 0.01,
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn empty_canvas_drag_keeps_ownership_across_a_panel_then_releases() {
    let (_temp, ctx, mut app) = fixture();
    let panel = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)[0]
        .1
        .screen_rect;
    let start = panel.center_top() - Vec2::new(0.0, 20.0);
    let before = Vec2::from(app.canvas_view.pan_offset);
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        frame(
            1.0,
            vec![Event::PointerMoved(start), button(start, true, Modifiers::NONE)],
            Modifiers::NONE,
        ),
    );
    assert!(!app.canvas_pan_input_claimed);
    for (time, movement) in [(1.016, Vec2::new(0.0, 60.0)), (1.032, Vec2::new(30.0, 90.0))] {
        let _ = run_app_frame_with_input(
            &ctx,
            &mut app,
            frame(time, vec![Event::PointerMoved(start + movement)], Modifiers::NONE),
        );
        assert!(app.canvas_pan_input_claimed);
        assert_offset(app.canvas_view.pan_offset, before + movement);
    }
    let end = start + Vec2::new(40.0, 110.0);
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        frame(
            1.048,
            vec![
                Event::PointerMoved(end),
                button(end, false, Modifiers::NONE),
                Event::PointerMoved(end + Vec2::splat(40.0)),
            ],
            Modifiers::NONE,
        ),
    );
    assert_offset(app.canvas_view.pan_offset, before + Vec2::new(40.0, 110.0));
    let _ = run_app_frame_with_input(&ctx, &mut app, frame(1.064, Vec::new(), Modifiers::NONE));
    assert!(!app.canvas_pan_input_claimed);
}

#[test]
fn same_frame_drag_excludes_pointer_movement_before_the_press() {
    let (_temp, ctx, mut app) = fixture();
    let start = Pos2::new(1200.0, 180.0);
    let movement = Vec2::new(40.0, 50.0);
    let before = Vec2::from(app.canvas_view.pan_offset);
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        frame(
            1.0,
            vec![
                Event::PointerMoved(start),
                button(start, true, Modifiers::NONE),
                Event::PointerMoved(start + movement),
                button(start + movement, false, Modifiers::NONE),
                Event::PointerMoved(start + movement + Vec2::splat(80.0)),
            ],
            Modifiers::NONE,
        ),
    );
    assert_offset(app.canvas_view.pan_offset, before + movement);
}

#[test]
fn coalesced_release_and_new_press_keep_their_own_motion() {
    let (_temp, ctx, mut app) = fixture();
    let start = Pos2::new(1200.0, 180.0);
    let before = Vec2::from(app.canvas_view.pan_offset);
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        frame(
            1.0,
            vec![Event::PointerMoved(start), button(start, true, Modifiers::NONE)],
            Modifiers::NONE,
        ),
    );
    let end = start + Vec2::splat(40.0);
    let next_start = start + Vec2::splat(100.0);
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        frame(
            1.016,
            vec![
                Event::PointerMoved(end),
                button(end, false, Modifiers::NONE),
                Event::PointerMoved(next_start),
                button(next_start, true, Modifiers::NONE),
            ],
            Modifiers::NONE,
        ),
    );
    assert_offset(app.canvas_view.pan_offset, before + Vec2::splat(40.0));
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        frame(
            1.032,
            vec![Event::PointerMoved(next_start + Vec2::new(20.0, 0.0))],
            Modifiers::NONE,
        ),
    );
    assert_offset(app.canvas_view.pan_offset, before + Vec2::new(60.0, 40.0));
}

#[test]
fn space_and_middle_panning_do_not_replay_primary_drag_threshold_motion() {
    let start = Pos2::new(1200.0, 180.0);
    for mode in [
        Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        },
        Event::PointerButton {
            pos: start,
            button: PointerButton::Middle,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ] {
        let (_temp, ctx, mut app) = fixture();
        let _ = run_app_frame_with_input(
            &ctx,
            &mut app,
            frame(0.5, vec![Event::PointerMoved(start)], Modifiers::NONE),
        );
        let before = Vec2::from(app.canvas_view.pan_offset);
        let _ = run_app_frame_with_input(
            &ctx,
            &mut app,
            frame(1.0, vec![mode, button(start, true, Modifiers::NONE)], Modifiers::NONE),
        );
        for (time, movement) in [(1.016, 3.0), (1.032, 9.0)] {
            let offset = Vec2::new(movement, 0.0);
            let _ = run_app_frame_with_input(
                &ctx,
                &mut app,
                frame(time, vec![Event::PointerMoved(start + offset)], Modifiers::NONE),
            );
            assert_offset(app.canvas_view.pan_offset, before + offset);
        }
    }
}

#[test]
fn panel_origin_drags_do_not_become_canvas_drags() {
    let (_temp, ctx, mut app) = fixture();
    let panel = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)[0]
        .1
        .screen_rect;
    let start = panel.center();
    let before = Vec2::from(app.canvas_view.pan_offset);
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        frame(
            1.0,
            vec![Event::PointerMoved(start), button(start, true, Modifiers::NONE)],
            Modifiers::NONE,
        ),
    );
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        frame(
            1.016,
            vec![Event::PointerMoved(Pos2::new(1200.0, 700.0))],
            Modifiers::NONE,
        ),
    );
    assert!(!app.canvas_pan_input_claimed);
    assert_offset(app.canvas_view.pan_offset, before);
}

#[test]
fn modified_clicks_and_focus_loss_do_not_start_or_resume_plain_drag() {
    for modifiers in [Modifiers::CTRL, Modifiers::SHIFT, Modifiers::ALT] {
        let (_temp, ctx, mut app) = fixture();
        let start = Pos2::new(1200.0, 180.0);
        let before = Vec2::from(app.canvas_view.pan_offset);
        let _ = run_app_frame_with_input(
            &ctx,
            &mut app,
            frame(
                1.0,
                vec![
                    Event::PointerMoved(start),
                    button(start, true, modifiers),
                    Event::ModifiersChanged(Modifiers::NONE),
                ],
                Modifiers::NONE,
            ),
        );
        let _ = run_app_frame_with_input(
            &ctx,
            &mut app,
            frame(
                1.016,
                vec![Event::PointerMoved(start + Vec2::splat(40.0))],
                Modifiers::NONE,
            ),
        );
        assert!(!app.canvas_pan_input_claimed);
        assert_offset(app.canvas_view.pan_offset, before);
    }
    let (_temp, ctx, mut app) = fixture();
    let start = Pos2::new(1200.0, 180.0);
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        frame(
            1.0,
            vec![Event::PointerMoved(start), button(start, true, Modifiers::NONE)],
            Modifiers::NONE,
        ),
    );
    let mut lost_focus = frame(1.016, Vec::new(), Modifiers::NONE);
    lost_focus.focused = false;
    let _ = run_app_frame_with_input(&ctx, &mut app, lost_focus);
    let before = Vec2::from(app.canvas_view.pan_offset);
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        frame(
            1.032,
            vec![Event::PointerMoved(start + Vec2::splat(40.0))],
            Modifiers::NONE,
        ),
    );
    assert!(!app.canvas_pan_input_claimed);
    assert_offset(app.canvas_view.pan_offset, before);
}

#[test]
fn modifier_press_and_release_in_one_frame_cancels_an_owned_drag() {
    let (_temp, ctx, mut app) = fixture();
    let start = Pos2::new(1200.0, 180.0);
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        frame(
            1.0,
            vec![Event::PointerMoved(start), button(start, true, Modifiers::NONE)],
            Modifiers::NONE,
        ),
    );
    let before = Vec2::from(app.canvas_view.pan_offset);
    for (time, events) in [
        (
            1.016,
            vec![
                Event::ModifiersChanged(Modifiers::CTRL),
                Event::PointerMoved(start + Vec2::splat(40.0)),
                Event::ModifiersChanged(Modifiers::NONE),
            ],
        ),
        (1.032, vec![Event::PointerMoved(start + Vec2::splat(60.0))]),
    ] {
        let _ = run_app_frame_with_input(&ctx, &mut app, frame(time, events, Modifiers::NONE));
        assert!(!app.canvas_pan_input_claimed);
        assert_offset(app.canvas_view.pan_offset, before);
    }
}

#[test]
fn foreground_controls_cannot_start_canvas_drag() {
    let ctx = Context::default();
    let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0));
    let start = Pos2::new(50.0, 50.0);
    let mut active = false;
    for (time, events) in [
        (0.0, Vec::new()),
        (0.016, Vec::new()),
        (
            1.0,
            vec![Event::PointerMoved(start), button(start, true, Modifiers::NONE)],
        ),
        (1.016, vec![Event::PointerMoved(Pos2::new(250.0, 250.0))]),
    ] {
        let output = ctx.run_ui(frame(time, events, Modifiers::NONE), |ctx| {
            let events = ctx.input(|input| input.events.clone());
            active = super::canvas_drag_delta(ctx, canvas, &std::iter::empty(), &events).is_some();
            egui::Area::new(egui::Id::new("control"))
                .fixed_pos(Pos2::new(20.0, 20.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    ui.allocate_exact_size(Vec2::splat(100.0), egui::Sense::drag());
                });
        });
        let _ = crate::test_egui::DiscardTextures::discard_textures(output);
    }
    assert!(!active);
}
