use super::*;

#[test]
fn completed_gesture_rechecks_current_modal_and_fullscreen_state() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let pos = Pos2::new(500.0, 400.0);
    app.canvas_gesture.completed = Some(pos);
    app.toggle_settings();
    assert!(app.settings.is_some());
    app.handle_canvas_double_click(&ctx);
    assert!(app.pending_preset_pick.is_none());
    app.settings = None;
    app.handle_canvas_double_click(&ctx);
    assert!(app.pending_preset_pick.is_none());

    app.canvas_gesture.completed = Some(pos);
    app.fullscreen_panel = Some(horizon_core::PanelId(99));
    app.handle_canvas_double_click(&ctx);
    app.fullscreen_panel = None;
    app.handle_canvas_double_click(&ctx);
    assert!(app.pending_preset_pick.is_none());
}

#[test]
fn skipped_canvas_handler_cannot_reuse_a_previous_frames_completion() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    app.canvas_gesture.completed = Some(Pos2::new(500.0, 400.0));
    let mut raw = raw_input([1400.0, 900.0], None);
    app.filter_canvas_gesture(&ctx, &mut raw);
    app.handle_canvas_double_click(&ctx);
    assert!(app.pending_preset_pick.is_none());
}

#[test]
fn viewport_stabilization_blocks_recognition_and_discards_reserved_input() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    app.arm_root_viewport_stabilizer(true, [1400.0, 900.0]);
    app.startup_workspace_organization_pending = true;
    assert!(!app.canvas_gesture_enabled());
    let pos = Pos2::new(500.0, 400.0);
    for (time, pressed) in [(1.0, true), (1.05, false), (1.15, true), (1.20, false)] {
        let mut raw = raw_input([1400.0, 900.0], None);
        raw.time = Some(time);
        raw.events = vec![button(pos, pressed)];
        app.filter_canvas_gesture(&ctx, &mut raw);
        assert!(app.canvas_gesture.completed.is_none());
        assert!(app.canvas_gesture.pending.is_none());
    }
    // Suppression may become active after raw input has already been filtered.
    app.canvas_gesture.completed = Some(pos);
    app.canvas_gesture.queued.push_back(Frame {
        time: 1.3,
        events: vec![button(pos, true)],
    });
    app.suppress_root_viewport_interaction(&ctx);
    app.startup_workspace_organization_pending = false;
    app.root_viewport_stabilizer = None;
    assert!(app.canvas_gesture_enabled());
    assert!(app.canvas_gesture.queued.is_empty());
    app.handle_canvas_double_click(&ctx);
    assert!(app.pending_preset_pick.is_none());
    app.canvas_gesture.completed = Some(pos);
    app.handle_canvas_double_click(&ctx);
    assert!(app.pending_preset_pick.is_some());
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::CTRL,
    }
}

#[test]
fn pointer_exit_preserves_a_completed_single_click_before_forwarding_exit() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    assert!(feed(&mut gesture, 1.0, vec![button(pos, true)]).events.is_empty());
    assert!(feed(&mut gesture, 1.05, vec![button(pos, false)]).events.is_empty());
    let replay = feed(&mut gesture, 1.1, vec![Event::PointerGone]);
    assert_eq!(replay.events, vec![button(pos, true), button(pos, false)]);
    assert_eq!(feed(&mut gesture, 1.15, vec![]).events, vec![Event::PointerGone]);
    assert!(gesture.pending.is_none());
    assert!(gesture.take_completed().is_none());
}

#[test]
fn pointer_exit_forwards_a_reserved_press_for_drag_cleanup() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    feed(&mut gesture, 1.0, vec![button(pos, true)]);
    assert_eq!(
        feed(&mut gesture, 1.1, vec![Event::PointerGone]).events,
        vec![button(pos, true), Event::PointerGone]
    );
    assert_eq!(
        feed(&mut gesture, 1.15, vec![button(pos, false)]).events,
        vec![button(pos, false)]
    );
    assert!(!gesture.forwarded_down);
}

fn feed(gesture: &mut CanvasGesture, time: f64, events: Vec<Event>) -> RawInput {
    let mut raw = RawInput {
        time: Some(time),
        events,
        ..RawInput::default()
    };
    gesture.replayed_origin = None;
    gesture.filter(&mut raw, &InputOptions::default(), |_| true);
    raw
}

#[test]
fn double_click_never_reaches_panel_input() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    for (time, pressed) in [(0.0, true), (0.05, false), (0.15, true), (0.20, false)] {
        assert!(feed(&mut gesture, time, vec![button(pos, pressed)]).events.is_empty());
    }
    assert_eq!(gesture.take_completed(), Some(pos));
    assert_eq!(gesture.take_completed(), None);
}

#[test]
fn a_single_click_replays_with_original_modifiers_and_position() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    feed(&mut gesture, 0.0, vec![button(pos, true)]);
    feed(&mut gesture, 0.05, vec![button(pos, false)]);
    assert!(feed(&mut gesture, 0.2, vec![]).events.is_empty());
    assert_eq!(
        feed(&mut gesture, 0.36, vec![]).events,
        vec![button(pos, true), button(pos, false)]
    );
    assert_eq!(gesture.take_completed(), None);
}

#[test]
fn dragging_flushes_the_press_before_motion_and_release() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    feed(&mut gesture, 0.0, vec![button(pos, true)]);
    let moved = pos + egui::vec2(20.0, 0.0);
    assert_eq!(
        feed(&mut gesture, 0.05, vec![Event::PointerMoved(moved)]).events,
        vec![button(pos, true), Event::PointerMoved(moved)]
    );
    assert_eq!(
        feed(&mut gesture, 0.1, vec![button(moved, false)]).events,
        vec![button(moved, false)]
    );
}

#[test]
fn focus_loss_cancels_without_delivering_a_stale_press() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    feed(&mut gesture, 0.0, vec![button(pos, true)]);
    let mut raw = RawInput {
        focused: false,
        time: Some(0.1),
        ..RawInput::default()
    };
    gesture.filter(&mut raw, &InputOptions::default(), |_| true);
    assert!(feed(&mut gesture, 0.2, vec![button(pos, false)]).events.is_empty());
    assert!(feed(&mut gesture, 0.5, vec![]).events.is_empty());
}

#[test]
fn native_relative_motion_does_not_release_a_reserved_press() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    feed(&mut gesture, 0.0, vec![button(pos, true)]);
    let output = feed(
        &mut gesture,
        0.02,
        vec![
            Event::MouseMoved(egui::vec2(1.0, 1.0)),
            Event::PointerMoved(pos + egui::vec2(1.0, 1.0)),
        ],
    );
    assert!(
        !output
            .events
            .iter()
            .any(|event| matches!(event, Event::PointerButton { .. }))
    );
    feed(&mut gesture, 0.05, vec![button(pos, false)]);
    feed(&mut gesture, 0.15, vec![button(pos, true)]);
    assert_eq!(gesture.take_completed(), Some(pos));
}

#[test]
fn a_held_second_press_opens_once_without_replaying_a_synthetic_double_click() {
    use crate::test_egui::DiscardTextures;
    let ctx = egui::Context::default();
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    for (time, pressed) in [(0.0, true), (0.05, false), (0.15, true), (0.40, false)] {
        let raw = feed(&mut gesture, time, vec![button(pos, pressed)]);
        let _ = ctx
            .run_ui(raw, |ui| {
                assert!(!ui.input(|input| input.pointer.any_click() || input.pointer.any_pressed()));
            })
            .discard_textures();
    }
    assert_eq!(gesture.take_completed(), Some(pos));
}

#[test]
fn raw_hook_prevents_panel_focus_and_title_rename_before_opening_the_menu() {
    use crate::app::test_support::{
        editor_panel_state, editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup,
    };
    use eframe::App as _;
    use horizon_core::{RuntimeState, StartupDecision};
    let mut workspace = editor_workspace_state("fixture", [0.0, 0.0]);
    workspace.panels.push(editor_panel_state("second", [400.0, 60.0]));
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState {
            workspaces: vec![workspace],
            ..RuntimeState::default()
        }),
    });
    for _ in 0..3 {
        run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    }
    let panels = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None);
    assert_eq!(panels.len(), 2);
    let first = panels[0].0;
    app.board.focus(first);
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    assert_eq!(app.board.focused, Some(first), "baseline focus must settle");
    let pos = panels[1].1.screen_rect.center_top() + egui::vec2(0.0, 12.0);
    assert!(
        !app.overlay_exclusion_zones(&ctx).contains(pos),
        "test target must be outside overlays"
    );
    assert!(
        app.fullscreen_panel.is_none()
            && !app.host_dialog_open()
            && app.settings.is_none()
            && app.session_manager.is_none()
            && app.startup_chooser.is_none()
            && app.command_palette.is_none()
            && !app
                .search_overlay
                .as_ref()
                .is_some_and(crate::search_overlay::SearchOverlay::input_focused)
            && app.dir_picker.is_none(),
        "test must have no modal: fullscreen={} host={} settings={} sessions={} startup={} palette={} search={} dir={}",
        app.fullscreen_panel.is_some(),
        app.host_dialog_open(),
        app.settings.is_some(),
        app.session_manager.is_some(),
        app.startup_chooser.is_some(),
        app.command_palette.is_some(),
        app.search_overlay.is_some(),
        app.dir_picker.is_some()
    );
    for (index, (time, pressed)) in [(1.0, true), (1.05, false), (1.15, true), (1.20, false)]
        .into_iter()
        .enumerate()
    {
        let mut raw = raw_input([1400.0, 900.0], None);
        raw.time = Some(time);
        if index == 0 {
            raw.events.push(Event::ModifiersChanged(egui::Modifiers::CTRL));
        }
        raw.events.extend([Event::PointerMoved(pos), button(pos, pressed)]);
        app.raw_input_hook(&ctx, &mut raw);
        assert!(
            !raw.events
                .iter()
                .any(|event| matches!(event, Event::PointerButton { .. })),
            "reserved event leaked at step {index}; target {pos:?}, canvas {:?}",
            app.canvas_rect(&ctx)
        );
        run_app_frame_with_input(&ctx, &mut app, raw);
        assert_eq!(app.board.focused, Some(first));
        assert!(app.renaming_panel.is_none());
        assert!(app.renaming_workspace.is_none());
    }
    assert!(app.pending_preset_pick.is_some());
}

#[test]
fn delayed_single_clicks_do_not_compress_into_an_egui_double_click() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};
    use crate::test_egui::DiscardTextures;
    use eframe::App as _;
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let pos = Pos2::new(500.0, 400.0);
    let mut clicks = 0;
    for (time, events) in [
        (
            1.0,
            vec![Event::ModifiersChanged(egui::Modifiers::CTRL), button(pos, true)],
        ),
        (1.05, vec![button(pos, false)]),
        (1.36, vec![]),
        (1.40, vec![button(pos, true)]),
        (1.45, vec![button(pos, false)]),
        (1.46, vec![Event::ModifiersChanged(egui::Modifiers::NONE)]),
    ] {
        let mut raw = raw_input([1400.0, 900.0], None);
        raw.time = Some(time);
        raw.events = events;
        app.raw_input_hook(&ctx, &mut raw);
        let _ = ctx
            .run_ui(raw, |ui| {
                ui.input(|input| {
                    assert!(!input.pointer.button_double_clicked(PointerButton::Primary));
                    clicks += usize::from(input.pointer.primary_clicked());
                });
            })
            .discard_textures();
    }
    assert_eq!(clicks, 2);
    assert!(app.canvas_gesture.take_completed().is_none());
    assert!(!ctx.input(|input| input.pointer.primary_down()));
}

#[test]
fn a_deferred_click_does_not_turn_the_next_plain_click_into_a_double() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};
    use crate::test_egui::DiscardTextures;
    use eframe::App as _;
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let pos = Pos2::new(500.0, 400.0);
    let plain = |pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    for (time, events, expected_double) in [
        (
            1.0,
            vec![Event::ModifiersChanged(egui::Modifiers::CTRL), button(pos, true)],
            false,
        ),
        (1.05, vec![button(pos, false)], false),
        (1.36, vec![], false),
        (1.37, vec![Event::ModifiersChanged(egui::Modifiers::NONE)], false),
        (1.40, vec![plain(true)], false),
        (1.45, vec![plain(false)], false),
        (1.50, vec![plain(true)], false),
        (1.55, vec![plain(false)], true),
    ] {
        let mut raw = raw_input([1400.0, 900.0], None);
        raw.time = Some(time);
        raw.events = events;
        app.raw_input_hook(&ctx, &mut raw);
        let _ = ctx
            .run_ui(raw, |ui| {
                assert_eq!(
                    ui.input(|input| input.pointer.button_double_clicked(PointerButton::Primary)),
                    expected_double,
                    "at {time}"
                );
            })
            .discard_textures();
    }
}

#[test]
fn replay_preserves_queued_motion_modifiers_and_text_order() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};
    use crate::test_egui::DiscardTextures;
    use eframe::App as _;
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let origin = Pos2::new(500.0, 400.0);
    let moved = origin + egui::vec2(40.0, 0.0);
    let mut delivered = Vec::new();
    for (time, events) in [
        (
            1.0,
            vec![Event::ModifiersChanged(egui::Modifiers::CTRL), button(origin, true)],
        ),
        (1.05, vec![button(origin, false)]),
        (
            1.1,
            vec![
                Event::PointerMoved(moved),
                Event::ModifiersChanged(egui::Modifiers::NONE),
                Event::Text("x".into()),
            ],
        ),
        (1.11, vec![]),
        (1.12, vec![]),
    ] {
        let mut raw = raw_input([1400.0, 900.0], None);
        raw.time = Some(time);
        raw.events = events;
        app.raw_input_hook(&ctx, &mut raw);
        delivered.extend(raw.events.clone());
        let _ = ctx.run_ui(raw, |_| {}).discard_textures();
    }
    assert_eq!(ctx.input(|input| input.pointer.latest_pos()), Some(moved));
    assert_eq!(ctx.input(|input| input.modifiers), egui::Modifiers::NONE);
    let release = delivered
        .iter()
        .position(|event| *event == button(origin, false))
        .unwrap();
    let motion = delivered
        .iter()
        .position(|event| *event == Event::PointerMoved(moved))
        .unwrap();
    let text = delivered
        .iter()
        .position(|event| *event == Event::Text("x".into()))
        .unwrap();
    assert!(release < motion && motion < text);
}

#[test]
fn completed_replay_separates_same_batch_plain_clicks() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};
    use crate::test_egui::DiscardTextures;
    use eframe::App as _;
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let pos = Pos2::new(500.0, 400.0);
    let plain = |pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut clicks = 0;
    for (time, events) in [
        (1.0, vec![button(pos, true)]),
        (1.05, vec![button(pos, false)]),
        (1.36, vec![plain(true), plain(false)]),
        (1.37, vec![]),
        (1.38, vec![]),
    ] {
        let mut raw = raw_input([1400.0, 900.0], None);
        raw.time = Some(time);
        raw.events = events;
        app.raw_input_hook(&ctx, &mut raw);
        let _ = ctx
            .run_ui(raw, |ui| {
                ui.input(|input| {
                    assert!(!input.pointer.button_double_clicked(PointerButton::Primary));
                    assert!(!input.pointer.button_triple_clicked(PointerButton::Primary));
                    clicks += usize::from(input.pointer.primary_clicked());
                });
            })
            .discard_textures();
    }
    assert_eq!(clicks, 2);
}

#[test]
fn queued_clicks_keep_their_original_arrival_times() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    gesture.queued.extend([
        Frame {
            time: 1.0,
            events: vec![button(pos, true), button(pos, false)],
        },
        Frame {
            time: 1.5,
            events: vec![button(pos, true), button(pos, false)],
        },
    ]);
    assert!(feed(&mut gesture, 2.0, vec![]).events.is_empty());
    assert_eq!(
        feed(&mut gesture, 2.5, vec![]).events,
        vec![button(pos, true), button(pos, false)]
    );
    assert!(feed(&mut gesture, 2.51, vec![]).events.is_empty());
    assert_eq!(gesture.take_completed(), None);
}

#[test]
fn repeated_native_modifier_snapshots_do_not_release_reserved_clicks() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    for time in [1.0, 1.1] {
        let unchanged = Event::ModifiersChanged(egui::Modifiers::CTRL);
        let output = feed(
            &mut gesture,
            time,
            vec![
                unchanged.clone(),
                button(pos, true),
                unchanged.clone(),
                unchanged.clone(),
                button(pos, false),
                unchanged,
            ],
        );
        assert!(
            output
                .events
                .iter()
                .all(|event| matches!(event, Event::ModifiersChanged(_)))
        );
    }
    assert_eq!(gesture.take_completed(), Some(pos));
    assert!(gesture.pending.is_none());
}

#[test]
fn focus_loss_after_a_forwarded_drag_allows_the_next_modified_double_click() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    feed(&mut gesture, 0.0, vec![button(pos, true)]);
    feed(
        &mut gesture,
        0.05,
        vec![Event::PointerMoved(pos + egui::vec2(20.0, 0.0))],
    );
    assert!(gesture.forwarded_down);
    feed(&mut gesture, 0.1, vec![Event::WindowFocused(false)]);
    assert!(!gesture.forwarded_down);
    feed(&mut gesture, 0.2, vec![Event::WindowFocused(true), button(pos, false)]);
    for (time, pressed) in [(1.0, true), (1.05, false), (1.15, true), (1.20, false)] {
        assert!(feed(&mut gesture, time, vec![button(pos, pressed)]).events.is_empty());
    }
    assert_eq!(gesture.take_completed(), Some(pos));
}

#[test]
fn later_secondary_click_replays_completed_primary_first() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    feed(&mut gesture, 1.0, vec![button(pos, true)]);
    feed(&mut gesture, 1.05, vec![button(pos, false)]);
    let secondary = Event::PointerButton {
        pos,
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: egui::Modifiers::CTRL,
    };
    assert_eq!(
        feed(&mut gesture, 1.1, vec![secondary.clone()]).events,
        vec![button(pos, true), button(pos, false)]
    );
    assert_eq!(feed(&mut gesture, 1.15, vec![]).events, vec![secondary]);
    assert!(feed(&mut gesture, 1.2, vec![button(pos, true)]).events.is_empty());
    let primary = feed(&mut gesture, 1.25, vec![]);
    assert_eq!(primary.events, vec![button(pos, true)]);
    assert!(gesture.pending.is_none());
}

#[test]
fn ordinary_input_keeps_its_existing_buffer_and_tracks_other_buttons() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    let mut raw = RawInput {
        time: Some(1.0),
        events: vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Middle,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        ..RawInput::default()
    };
    let allocation = raw.events.as_ptr();
    assert!(gesture.filter(&mut raw, &InputOptions::default(), |_| true).is_none());
    assert_eq!(raw.events.as_ptr(), allocation);
    assert!(gesture.queued.is_empty());
    assert_eq!(
        feed(&mut gesture, 1.1, vec![button(pos, true)]).events,
        vec![button(pos, true)]
    );
    assert!(gesture.pending.is_none());
}

#[test]
fn same_frame_secondary_press_then_focus_loss_does_not_block_future_gestures() {
    let mut gesture = CanvasGesture::default();
    let pos = Pos2::new(300.0, 300.0);
    feed(
        &mut gesture,
        1.0,
        vec![
            Event::PointerButton {
                pos,
                button: PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            Event::WindowFocused(false),
        ],
    );
    feed(&mut gesture, 1.1, vec![Event::WindowFocused(true)]);
    feed(&mut gesture, 1.2, vec![button(pos, true)]);
    feed(&mut gesture, 1.25, vec![button(pos, false)]);
    feed(&mut gesture, 1.3, vec![button(pos, true)]);
    assert_eq!(gesture.take_completed(), Some(pos));
}

#[test]
fn paced_queued_plain_clicks_preserve_egui_classification_and_wall_time() {
    for (gap, second_delivery, expected_double) in [(0.5, 2.5, false), (0.1, 2.1, true)] {
        let mut gesture = CanvasGesture::default();
        let pos = Pos2::new(300.0, 300.0);
        let click = || {
            [true, false]
                .map(|pressed| Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                })
                .to_vec()
        };
        gesture.queued.extend([
            Frame {
                time: 1.0,
                events: click(),
            },
            Frame {
                time: 1.0 + gap,
                events: click(),
            },
        ]);
        let ctx = egui::Context::default();
        for (time, expected_click) in [(2.0, true), (2.01, false), (second_delivery, true)] {
            let raw = feed(&mut gesture, time, vec![]);
            assert_eq!(raw.time, Some(time));
            ctx.begin_pass(raw);
            ctx.input(|input| {
                assert!((input.time - time).abs() < f64::EPSILON);
                assert_eq!(input.pointer.any_click(), expected_click);
                assert_eq!(
                    input.pointer.button_double_clicked(PointerButton::Primary),
                    expected_click && time > 2.0 && expected_double
                );
            });
            ctx.end_pass().textures_delta.clear();
        }
    }
}

#[test]
fn a_second_stall_does_not_compress_the_remaining_queue() {
    let mut gesture = CanvasGesture::default();
    gesture.queued.extend([1.0, 1.5, 2.0].map(|time| Frame {
        time,
        events: vec![Event::Text(time.to_string())],
    }));
    assert_eq!(feed(&mut gesture, 3.0, vec![]).events, vec![Event::Text("1".into())]);
    assert_eq!(feed(&mut gesture, 4.0, vec![]).events, vec![Event::Text("1.5".into())]);
    assert!(feed(&mut gesture, 4.01, vec![]).events.is_empty());
    assert_eq!(feed(&mut gesture, 4.5, vec![]).events, vec![Event::Text("2".into())]);
}

#[test]
fn remote_host_modal_does_not_reserve_clicks_or_consume_completed_gestures() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    app.remote_hosts_overlay = Some(crate::remote_hosts_overlay::RemoteHostsOverlay::new());
    assert!(!app.canvas_gesture_enabled());
    let pos = Pos2::new(500.0, 400.0);
    let mut raw = raw_input([1400.0, 900.0], None);
    raw.events = vec![button(pos, true)];
    app.filter_canvas_gesture(&ctx, &mut raw);
    assert_eq!(raw.events, vec![button(pos, true)]);
    app.canvas_gesture.completed = Some(pos);
    app.handle_canvas_double_click(&ctx);
    assert!(app.pending_preset_pick.is_none());
}

#[test]
fn drained_backlog_expires_without_manufacturing_delayed_empty_frames() {
    let mut gesture = CanvasGesture::default();
    gesture.queued.push_back(Frame {
        time: 1.0,
        events: vec![Event::Text("queued".into())],
    });
    assert_eq!(
        feed(&mut gesture, 2.0, vec![]).events,
        vec![Event::Text("queued".into())]
    );
    for time in [2.01, 2.1, 3.0] {
        assert!(feed(&mut gesture, time, vec![]).events.is_empty());
        assert!(gesture.queued.is_empty());
    }
    assert!(gesture.delivery.is_none());
    assert_eq!(
        feed(&mut gesture, 3.01, vec![Event::Text("live".into())]).events,
        vec![Event::Text("live".into())]
    );
}

#[test]
fn focus_loss_discards_inherited_replay_pacing() {
    let mut gesture = CanvasGesture {
        delivery: Some(Delivery {
            source_time: 1.0,
            delivered_at: 2.0,
        }),
        ..CanvasGesture::default()
    };
    gesture.queued.push_back(Frame {
        time: 1.1,
        events: vec![button(Pos2::new(300.0, 300.0), true)],
    });
    gesture.queued.push_back(Frame {
        time: 1.2,
        events: vec![Event::Text("preserved".into())],
    });
    let mut raw = RawInput {
        time: Some(2.01),
        focused: false,
        events: vec![Event::WindowFocused(false)],
        ..RawInput::default()
    };
    gesture.filter(&mut raw, &InputOptions::default(), |_| false);
    assert!(gesture.delivery.is_none());
    assert!(gesture.queued.is_empty());
    assert_eq!(
        raw.events,
        vec![Event::Text("preserved".into()), Event::WindowFocused(false)]
    );
}

#[test]
fn startup_loading_and_recovery_controls_receive_modified_clicks_directly() {
    use crate::app::{
        StartupBootstrapFailure,
        test_support::{raw_input, run_app_frame_with_input, test_app},
    };
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let (_sender, receiver) = std::sync::mpsc::channel();
    app.startup_receiver = Some(receiver);
    let pos = Pos2::new(500.0, 400.0);
    for failed in [false, true] {
        if failed {
            app.startup_receiver = None;
            app.startup_bootstrap_failure = Some(StartupBootstrapFailure::WorkerDisconnected);
        }
        assert!(!app.canvas_gesture_enabled());
        let mut raw = raw_input([1400.0, 900.0], None);
        raw.events = vec![
            button(pos, true),
            button(pos, false),
            button(pos, true),
            button(pos, false),
        ];
        let original = raw.events.clone();
        app.filter_canvas_gesture(&ctx, &mut raw);
        assert_eq!(raw.events, original);
        assert!(app.canvas_gesture.pending.is_none());
        app.canvas_gesture.completed = Some(pos);
        app.handle_canvas_double_click(&ctx);
        assert!(app.pending_preset_pick.is_none());
    }
}
