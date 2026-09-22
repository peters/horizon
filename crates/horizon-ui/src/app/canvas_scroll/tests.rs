use egui::{Context, Event, InputOptions, Modifiers, MouseWheelUnit, Pos2, RawInput, TouchPhase, Vec2, ViewportId};
use horizon_core::{RuntimeState, StartupDecision};
use tempfile::TempDir;

use super::super::HorizonApp;
use super::super::test_support::{editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup};
use super::{ScrollGesture, ScrollTarget, WheelStep, canvas_zoom_delta, route_canvas_scroll};
use crate::test_egui::DiscardTextures;

/// The gesture only needs to know whether a panel is under the pointer, so the
/// tests use one stand-in id for "some panel".
const PANEL: horizon_core::PanelId = horizon_core::PanelId(1);
const OTHER_PANEL: horizon_core::PanelId = horizon_core::PanelId(2);

fn panel(on_canvas: bool) -> ScrollTarget {
    if on_canvas {
        ScrollTarget::Canvas
    } else {
        ScrollTarget::Panel(PANEL)
    }
}

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
fn ending_gestures_apply_their_tail_without_stealing_a_new_contact() {
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
            // The claimed move batched with its end still pans, once.
            assert_offset(app.canvas_view.pan_offset, [before[0], before[1] - 5.0]);
            assert!(app.canvas_pan_input_claimed);
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
    gesture
        .route(&input, &InputOptions::default(), panel(on_canvas), true, &mut |_, _| {
            false
        })
        .pans_canvas
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
        assert!(
            !gesture
                .route(&input, &InputOptions::default(), panel(false), !focused, &mut |_, _| {
                    false
                })
                .pans_canvas
        );
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
                    route_canvas_scroll(ui.ctx(), panel(starts_on_canvas), true, |_, _| false).pans_canvas,
                    expected
                );
            })
            .discard_textures();
    }
}

fn route(gesture: &mut ScrollGesture, time: f64, on_canvas: bool, chain: bool, events: Vec<Event>) -> bool {
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
    gesture
        .route(&input, &InputOptions::default(), panel(on_canvas), true, &mut |_, _| {
            chain
        })
        .pans_canvas
}

#[test]
fn an_exhausted_panel_chains_its_gesture_to_the_canvas_and_keeps_it() {
    let mut gesture = ScrollGesture::default();
    let scroll = || vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)];
    // The panel absorbs while it can still scroll.
    assert!(!route(&mut gesture, 1.0, false, false, scroll()));
    // At its extent the gesture moves to the canvas...
    assert!(route(&mut gesture, 1.016, false, true, scroll()));
    // ...and stays there even though the panel could scroll again.
    assert!(route(&mut gesture, 1.032, false, false, scroll()));
    // A new contact starts over the panel again.
    route(
        &mut gesture,
        1.048,
        false,
        false,
        vec![wheel(Vec2::ZERO, TouchPhase::End)],
    );
    assert!(!route(&mut gesture, 1.064, false, false, scroll()));
}

#[test]
fn chaining_never_steals_a_gesture_that_started_on_the_canvas() {
    let mut gesture = ScrollGesture::default();
    let scroll = || vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)];
    assert!(route(&mut gesture, 1.0, true, false, scroll()));
    assert!(route(&mut gesture, 1.016, true, true, scroll()));
}

#[test]
fn chaining_follows_the_gesture_owner_not_the_hover_target() {
    // A gesture latched to one panel must not chain because some *other*
    // panel the pointer drifted over happens to be exhausted.
    let mut gesture = ScrollGesture::default();
    let scroll = || vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)];
    let mut only_other_is_exhausted = |id: horizon_core::PanelId, _: WheelStep| id == OTHER_PANEL;

    let input = |time: f64| {
        egui::InputState::default().begin_pass(
            RawInput {
                time: Some(time),
                events: scroll(),
                ..RawInput::default()
            },
            false,
            1.0,
            egui::InputOptions::default(),
        )
    };

    // Latches to PANEL, which can still scroll.
    assert!(
        !gesture
            .route(
                &input(1.0),
                &InputOptions::default(),
                ScrollTarget::Panel(PANEL),
                true,
                &mut only_other_is_exhausted
            )
            .pans_canvas
    );
    // The pointer moves over OTHER_PANEL, which is exhausted. The owner still
    // is not, so the gesture stays with its panel.
    assert!(
        !gesture
            .route(
                &input(1.016),
                &InputOptions::default(),
                ScrollTarget::Panel(OTHER_PANEL),
                true,
                &mut only_other_is_exhausted
            )
            .pans_canvas
    );
    // Once the owner itself is exhausted, the gesture chains.
    assert!(
        gesture
            .route(
                &input(1.032),
                &InputOptions::default(),
                ScrollTarget::Panel(OTHER_PANEL),
                true,
                &mut |_, _| true
            )
            .pans_canvas
    );
}

#[test]
fn a_zero_delta_gesture_start_does_not_chain() {
    // A phased gesture opens with a zero-delta `Start`, which carries no
    // direction. Chaining on it would hand the canvas a gesture belonging to a
    // terminal that still has scrollback to give.
    let mut gesture = ScrollGesture::default();
    let start = route(
        &mut gesture,
        1.0,
        false,
        true,
        vec![wheel(Vec2::ZERO, TouchPhase::Start)],
    );
    assert!(!start);
    assert!(!route(
        &mut gesture,
        1.016,
        false,
        false,
        vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]
    ));
}

#[test]
fn coalesced_opposing_wheels_are_judged_one_event_at_a_time() {
    // A terminal in the middle of its scrollback can absorb either vertical
    // direction; only a horizontal-only event finds it exhausted. egui can
    // coalesce opposing events into a zero frame delta, which must not read as
    // a horizontal swipe and chain the gesture away from the terminal.
    let mut mid_history = |_: horizon_core::PanelId, step: WheelStep| step.delta.y == 0.0;
    let input = egui::InputState::default().begin_pass(
        RawInput {
            time: Some(1.0),
            events: vec![
                wheel(Vec2::new(0.0, -5.0), TouchPhase::Move),
                wheel(Vec2::new(0.0, 5.0), TouchPhase::Move),
            ],
            ..RawInput::default()
        },
        false,
        1.0,
        egui::InputOptions::default(),
    );
    assert_eq!(input.smooth_scroll_delta, Vec2::ZERO);
    let mut gesture = ScrollGesture::default();
    let routing = gesture.route(
        &input,
        &InputOptions::default(),
        ScrollTarget::Panel(PANEL),
        true,
        &mut mid_history,
    );
    assert!(!routing.pans_canvas);
    assert!(routing.claimed_wheels.is_empty());
    assert_eq!(gesture.canvas_owned, Some(false));
}

#[test]
fn a_surface_without_a_scroll_extent_keeps_its_gesture() {
    // A canvas-drawn surface such as a cloud runtime card has no panel to ask
    // about its extent, so a gesture it starts can never chain to the canvas.
    let mut gesture = ScrollGesture::default();
    let input = egui::InputState::default().begin_pass(
        RawInput {
            time: Some(1.0),
            events: vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)],
            ..RawInput::default()
        },
        false,
        1.0,
        egui::InputOptions::default(),
    );
    let routing = gesture.route(
        &input,
        &InputOptions::default(),
        ScrollTarget::Surface,
        true,
        &mut |_, _| true,
    );
    assert!(!routing.pans_canvas);
    assert!(routing.claimed_wheels.is_empty());
}

fn pass(time: f64, events: Vec<Event>) -> egui::InputState {
    egui::InputState::default().begin_pass(
        RawInput {
            time: Some(time),
            events,
            ..RawInput::default()
        },
        false,
        1.0,
        InputOptions::default(),
    )
}

#[test]
fn a_chaining_frame_pans_only_by_the_events_the_canvas_claimed() {
    // The terminal absorbs the first event; the second finds it exhausted and
    // chains. egui's frame-wide scroll nets both to zero, but the canvas must
    // move by exactly the event it claimed.
    let mut gesture = ScrollGesture::default();
    let mut asked = 0;
    let events = vec![
        wheel(Vec2::new(0.0, 5.0), TouchPhase::Move),
        wheel(Vec2::new(0.0, -5.0), TouchPhase::Move),
    ];
    let routing = gesture.route(
        &pass(1.0, events),
        &InputOptions::default(),
        ScrollTarget::Panel(PANEL),
        true,
        &mut |_, _| {
            asked += 1;
            asked == 2
        },
    );
    assert_eq!(asked, 2, "each moving event is judged in order");
    assert_eq!(routing.claimed_wheels, vec![1]);
    assert!(routing.pans_canvas);
    assert_eq!(routing.pan, Vec2::new(0.0, -5.0));
}

#[test]
fn a_claimed_mouse_wheel_notch_is_eased_in_like_egui_scrolls_it() {
    let options = InputOptions::default();
    let mut gesture = ScrollGesture::default();
    let notch = Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: Vec2::new(0.0, -1.0),
        phase: TouchPhase::Move,
        modifiers: Modifiers::NONE,
    };
    let mut route = |time: f64, events: Vec<Event>| {
        gesture
            .route(
                &pass(time, events),
                &options,
                ScrollTarget::Canvas,
                true,
                &mut |_, _| false,
            )
            .pan
    };
    let first = route(1.0, vec![notch]);
    assert!(first.y < 0.0 && first.y > -options.line_scroll_speed, "{first:?}");
    let mut total = first;
    for frame in 1..60 {
        total += route(1.0 + f64::from(frame) / 60.0, Vec::new());
    }
    assert!((total.y + options.line_scroll_speed).abs() < 0.001, "{total:?}");
    assert!(total.x.abs() < f32::EPSILON);
}

#[test]
fn a_terminal_is_judged_where_the_frames_earlier_events_leave_it() {
    use horizon_core::{Panel, PanelId, PanelKind, PanelOptions, WorkspaceId};
    use std::fmt::Write as _;

    let (_temp, ctx, mut app) = app_fixture();
    let transcripts = tempfile::tempdir().expect("transcript tempdir");
    let mut replay = String::new();
    for index in 0..120 {
        write!(replay, "history {index:03}\r\n").expect("format history line");
    }
    std::fs::write(transcripts.path().join("history-panel.bin"), replay).expect("write transcript");
    let panel = Panel::spawn(
        PanelId(99),
        WorkspaceId(7),
        PanelOptions {
            kind: PanelKind::Ssh,
            rows: 10,
            cols: 40,
            local_id: Some("history-panel".to_string()),
            transcript_root: Some(transcripts.path().to_path_buf()),
            restore_as_disconnected_snapshot: true,
            ..PanelOptions::default()
        },
    )
    .expect("spawn history snapshot");
    assert!(panel.terminal().expect("terminal").history_size() > 3);
    app.board.panels.push(panel);

    let cell = crate::terminal_widget::wheel_cell_size(&ctx);
    let lines = |lines: f32| WheelStep {
        delta: Vec2::new(0.0, lines),
        unit: MouseWheelUnit::Line,
        modifiers: Modifiers::NONE,
    };
    let mut pending = None;
    let mut exhausted = |step| app.panel_scroll_exhausted(PanelId(99), step, cell, &mut pending);
    // At the bottom, a coalesced up-then-down pair belongs to the terminal:
    // the first event moves it off the bottom before the second applies.
    assert!(!exhausted(lines(2.0)));
    assert!(!exhausted(lines(-2.0)));
    // Back at the bottom, the next downward event is at the extent.
    assert!(exhausted(lines(-1.0)));
}

#[test]
fn a_move_batched_with_its_end_still_pans_in_full() {
    let options = InputOptions::default();
    let notch = |phase| Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: Vec2::new(0.0, if phase == TouchPhase::Move { -1.0 } else { 0.0 }),
        phase,
        modifiers: Modifiers::NONE,
    };
    for end in [TouchPhase::End, TouchPhase::Cancel] {
        let mut gesture = ScrollGesture::default();
        let routing = gesture.route(
            &pass(1.0, vec![notch(TouchPhase::Move), notch(end)]),
            &options,
            ScrollTarget::Canvas,
            true,
            &mut |_, _| false,
        );
        assert!(routing.pans_canvas);
        assert!(
            (routing.pan.y + options.line_scroll_speed).abs() < 0.001,
            "{:?}",
            routing.pan
        );
        assert_eq!(gesture.canvas_owned, None, "ownership ends with the gesture");
    }
}

#[test]
fn zoom_modified_wheels_are_judged_per_event() {
    // One frame can carry a plain wheel and a Ctrl/Cmd one. The plain event
    // pans; the modified one is a zoom step and stays in the stream.
    // `COMMAND` is what egui reports for Cmd on macOS and Ctrl elsewhere.
    for zoom_modifier in [Modifiers::CTRL, Modifiers::COMMAND] {
        let zoom_wheel = Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: Vec2::new(0.0, -5.0),
            phase: TouchPhase::Move,
            modifiers: zoom_modifier,
        };
        let mut gesture = ScrollGesture::default();
        let routing = gesture.route(
            &pass(
                1.0,
                vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move), zoom_wheel.clone()],
            ),
            &InputOptions::default(),
            ScrollTarget::Canvas,
            true,
            &mut |_, _| false,
        );
        assert_eq!(routing.claimed_wheels, vec![0]);
        assert_eq!(routing.pan, Vec2::new(0.0, -5.0));
        // A lone modified wheel never pans, whoever owns the gesture.
        let routing = gesture.route(
            &pass(1.016, vec![zoom_wheel]),
            &InputOptions::default(),
            ScrollTarget::Canvas,
            true,
            &mut |_, _| false,
        );
        assert!(routing.claimed_wheels.is_empty());
        assert!(!routing.pans_canvas);
    }
}

#[test]
fn a_frame_mixing_plain_and_zoom_wheels_pans_and_zooms_in_either_order() {
    let plain = wheel(Vec2::new(0.0, -5.0), TouchPhase::Move);
    let zoom = Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -5.0),
        phase: TouchPhase::Move,
        modifiers: Modifiers::COMMAND,
    };
    let pointer = Pos2::new(1200.0, 700.0);
    let frame = |time: f64, wheels: Vec<Event>| {
        let mut input = raw_input([1400.0, 900.0], None);
        input.time = Some(time);
        input.events = std::iter::once(Event::PointerMoved(pointer)).chain(wheels).collect();
        input
    };
    // Reference: the zoom step and the plain step in frames of their own.
    let (_temp, ctx, mut reference) = app_fixture();
    let before = (reference.canvas_view.zoom, reference.canvas_view.pan_offset);
    let _ = run_app_frame_with_input(&ctx, &mut reference, frame(1.0, vec![zoom.clone()]));
    let _ = run_app_frame_with_input(&ctx, &mut reference, frame(1.3, vec![plain.clone()]));
    assert!(reference.canvas_view.zoom < before.0, "the Ctrl/Cmd wheel zooms out");
    for wheels in [vec![plain.clone(), zoom.clone()], vec![zoom.clone(), plain.clone()]] {
        let (_temp, ctx, mut app) = app_fixture();
        let _ = run_app_frame_with_input(&ctx, &mut app, frame(1.0, wheels));
        assert!((app.canvas_view.zoom - reference.canvas_view.zoom).abs() < 1e-5);
        assert_offset(app.canvas_view.pan_offset, reference.canvas_view.pan_offset);
    }
}

#[test]
fn a_zoom_wheel_notch_eases_in_to_exactly_one_line_of_egui_zoom() {
    let options = InputOptions::default();
    let ctx = Context::default();
    let notch = Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: Vec2::new(0.0, -1.0),
        phase: TouchPhase::Move,
        modifiers: Modifiers::COMMAND,
    };
    let zoom_at = |time: f64, events: Vec<Event>| {
        let mut zoom = 1.0;
        let input = RawInput {
            time: Some(time),
            events,
            ..RawInput::default()
        };
        let _ = ctx
            .run_ui(input, |ui| zoom = canvas_zoom_delta(ui.ctx()))
            .discard_textures();
        zoom
    };
    let first = zoom_at(1.0, vec![notch]);
    let target = (options.scroll_zoom_speed * -options.line_scroll_speed).exp();
    assert!(first < 1.0 && first > target, "eased, not applied at once: {first}");
    let mut total = first;
    for frame in 1..60 {
        total *= zoom_at(1.0 + f64::from(frame) / 60.0, Vec::new());
    }
    assert!((total - target).abs() < 1e-4, "{total} vs {target}");
    // Plain wheels never zoom the canvas.
    assert!((zoom_at(3.0, vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]) - 1.0).abs() < f32::EPSILON);
    // The router skips Ctrl and Cmd wheels alike as zoom steps, so a lone
    // Ctrl, which is not `command` on macOS, must zoom here too.
    let ctrl = Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -5.0),
        phase: TouchPhase::Move,
        modifiers: Modifiers::CTRL,
    };
    let expected = (options.scroll_zoom_speed * -5.0).exp();
    assert!((zoom_at(4.0, vec![ctrl]) - expected).abs() < 1e-6);
}
