use super::*;

#[test]
fn mixed_plain_then_zoom_wheels_scroll_a_panel_by_plain_motion_only() {
    assert_mixed_scroll(false);
}

#[test]
fn mixed_zoom_then_plain_wheels_scroll_a_panel_by_plain_motion_only() {
    assert_mixed_scroll(true);
}

#[test]
fn modified_boundaries_preserve_plain_panel_motion_and_queued_notches() {
    for phase in [TouchPhase::End, TouchPhase::Cancel] {
        for modifiers in [Modifiers::NONE, Modifiers::CTRL, Modifiers::COMMAND] {
            for unit in [MouseWheelUnit::Point, MouseWheelUnit::Line] {
                let ctx = Context::default();
                let target = ScrollTarget::Panel(PANEL);
                for time in [0.0, 0.016, 0.032] {
                    let _ = scroll_area_frame(&ctx, time, target, Vec::new());
                }
                let plain = |unit, delta| Event::MouseWheel {
                    unit,
                    delta: Vec2::new(0.0, delta),
                    phase: TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                };
                let before = scroll_area_frame(&ctx, 1.0, target, vec![plain(MouseWheelUnit::Line, -1.0)]).0;
                let delta = if unit == MouseWheelUnit::Line { -1.0 } else { -5.0 };
                let boundary = Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: if modifiers == Modifiers::NONE {
                        Vec2::ZERO
                    } else {
                        Vec2::new(0.0, -3.0)
                    },
                    phase,
                    modifiers,
                };
                let actual = scroll_area_frame(&ctx, 1.016, target, vec![plain(unit, delta), boundary]).0;
                let line = InputOptions::default().line_scroll_speed;
                let expected = if modifiers == Modifiers::NONE {
                    before
                } else {
                    line + if unit == MouseWheelUnit::Line { line } else { 5.0 }
                };
                assert!(
                    (actual - expected).abs() < 0.001,
                    "{phase:?} {modifiers:?} {unit:?}: {actual} vs {expected}"
                );
                let idle = scroll_area_frame(&ctx, 1.032, target, Vec::new()).0;
                assert!((idle - actual).abs() < 0.001, "the boundary leaves no queued motion");
            }
        }
    }
}

fn assert_mixed_scroll(zoom_first: bool) {
    for unit in [MouseWheelUnit::Point, MouseWheelUnit::Line] {
        let mixed = Context::default();
        let plain_only = Context::default();
        let target = ScrollTarget::Panel(OTHER_PANEL);
        for time in [0.0, 0.016, 0.032] {
            let _ = scroll_area_frame(&mixed, time, target, Vec::new());
            let _ = scroll_area_frame(&plain_only, time, target, Vec::new());
        }
        // A prior plain notch must keep easing while this frame splits plain
        // and modified wheel input.
        let prior = Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: Vec2::new(0.0, -1.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
        };
        let _ = scroll_area_frame(&mixed, 0.984, target, vec![prior.clone()]);
        let _ = scroll_area_frame(&plain_only, 0.984, target, vec![prior]);
        let delta = Vec2::new(0.0, if unit == MouseWheelUnit::Line { -1.0 } else { -5.0 });
        let event = |modifiers| Event::MouseWheel {
            unit,
            delta,
            phase: TouchPhase::Move,
            modifiers,
        };
        let plain = event(Modifiers::NONE);
        let zoom = event(Modifiers::CTRL);
        let events = if zoom_first {
            vec![zoom, plain.clone()]
        } else {
            vec![plain.clone(), zoom]
        };
        let (actual, mut total_zoom) = scroll_area_frame(&mixed, 1.0, target, events);
        let expected = scroll_area_frame(&plain_only, 1.0, target, vec![plain]).0;
        assert!(
            (actual - expected).abs() < 0.001,
            "{unit:?} zoom_first={zoom_first}: {actual} vs {expected}"
        );
        for step in 1..40 {
            let time = 1.0 + f64::from(step) * 0.016;
            let (actual, zoom) = scroll_area_frame(&mixed, time, target, Vec::new());
            total_zoom *= zoom;
            let expected = scroll_area_frame(&plain_only, time, target, Vec::new()).0;
            assert!(
                (actual - expected).abs() < 0.001,
                "idle frame {step}: {actual} vs {expected}"
            );
        }
        let options = InputOptions::default();
        let points = if unit == MouseWheelUnit::Line {
            -options.line_scroll_speed
        } else {
            -5.0
        };
        assert!((total_zoom - (points * options.scroll_zoom_speed).exp()).abs() < 0.0001);
    }
}

#[test]
fn displaced_panel_wheels_do_not_reach_another_panels_scroll_area() {
    assert_displaced_wheels_stay_with_owner(ScrollTarget::Panel(PANEL));
}

#[test]
fn displaced_surface_wheels_do_not_reach_a_panels_scroll_area() {
    assert_displaced_wheels_stay_with_owner(ScrollTarget::Surface);
}

#[test]
fn modified_boundaries_discard_displaced_backlog_but_keep_fresh_panel_motion() {
    for phase in [TouchPhase::Start, TouchPhase::End, TouchPhase::Cancel] {
        for modifiers in [Modifiers::CTRL, Modifiers::COMMAND] {
            for fresh_motion in [false, true] {
                let ctx = Context::default();
                for time in [0.0, 0.016, 0.032] {
                    let _ = scroll_area_frame(&ctx, time, ScrollTarget::Panel(PANEL), Vec::new());
                }
                let notch = Event::MouseWheel {
                    unit: MouseWheelUnit::Line,
                    delta: Vec2::new(0.0, -1.0),
                    phase: TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                };
                let before = scroll_area_frame(&ctx, 1.0, ScrollTarget::Panel(PANEL), vec![notch]).0;
                let mut events = vec![Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: Vec2::ZERO,
                    phase,
                    modifiers,
                }];
                if fresh_motion {
                    events.push(wheel(Vec2::new(0.0, -5.0), TouchPhase::Move));
                }
                let target = ScrollTarget::Panel(OTHER_PANEL);
                let actual = scroll_area_frame(&ctx, 1.016, target, events).0;
                let expected = before + if fresh_motion { 5.0 } else { 0.0 };
                assert!(
                    (actual - expected).abs() < 0.001,
                    "{phase:?} {modifiers:?} fresh={fresh_motion}: {actual} vs {expected}"
                );
                for step in 2..20 {
                    let idle = scroll_area_frame(&ctx, 1.0 + f64::from(step) * 0.016, target, Vec::new()).0;
                    assert!((idle - expected).abs() < 0.001, "the displaced tail must not replay");
                }
            }
        }
    }
}

fn assert_displaced_wheels_stay_with_owner(owner: ScrollTarget) {
    let ctx = Context::default();
    let frame = |time, target, events| scroll_area_frame(&ctx, time, target, events).0;
    for time in [0.0, 0.016, 0.032] {
        assert!(frame(time, ScrollTarget::Panel(OTHER_PANEL), Vec::new()).abs() < f32::EPSILON);
    }
    assert!(frame(1.0, owner, vec![wheel(Vec2::new(0.0, 1.0), TouchPhase::Move)]).abs() < f32::EPSILON);
    let notch = Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: Vec2::new(0.0, -1.0),
        phase: TouchPhase::Move,
        modifiers: Modifiers::NONE,
    };
    assert!(
        frame(1.016, ScrollTarget::Panel(OTHER_PANEL), vec![notch]).abs() < f32::EPSILON,
        "a discarded owner wheel must not scroll the hovered editor"
    );
    for step in 2..20 {
        assert!(
            frame(
                1.0 + f64::from(step) * 0.016,
                ScrollTarget::Panel(OTHER_PANEL),
                Vec::new()
            )
            .abs()
                < f32::EPSILON,
            "the discarded notch's easing must not leak on idle frame {step}"
        );
    }
    assert!(
        frame(
            2.0,
            ScrollTarget::Panel(OTHER_PANEL),
            vec![wheel(Vec2::ZERO, TouchPhase::Start)]
        )
        .abs()
            < f32::EPSILON
    );
    assert!(
        frame(
            2.016,
            ScrollTarget::Panel(OTHER_PANEL),
            vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]
        ) > 0.0,
        "a fresh gesture reaches the hovered editor normally"
    );
}

fn scroll_area_frame(ctx: &Context, time: f64, target: ScrollTarget, events: Vec<Event>) -> (f32, f32) {
    scroll_area_frame_with_routing(ctx, time, target, events, Some(true))
}

fn scroll_area_frame_with_routing(
    ctx: &Context,
    time: f64,
    target: ScrollTarget,
    events: Vec<Event>,
    routing_allowed: Option<bool>,
) -> (f32, f32) {
    let mut input = raw_input([400.0, 300.0], None);
    input.time = Some(time);
    input.events = std::iter::once(Event::PointerMoved(Pos2::new(100.0, 100.0)))
        .chain(events)
        .collect();
    let mut offset = 0.0;
    let mut zoom = 1.0;
    let _ = ctx
        .run_ui(input, |ui| {
            if let Some(allowed) = routing_allowed {
                zoom = canvas_zoom_delta(ui.ctx(), allowed);
                let mut routing = route_canvas_scroll(ui.ctx(), target, allowed, |_, _| false);
                routing.discard_displaced_wheels(target);
                routing.defer_for_panel_delivery(ui.ctx());
                routing.consume(ui.ctx(), &mut Vec::new());
            }
            // Editor panes consume egui's smooth scroll, not raw wheel events.
            offset = egui::ScrollArea::vertical()
                .id_salt("other_editor")
                .max_height(200.0)
                .show(ui, |ui| {
                    ui.allocate_space(Vec2::new(200.0, 2000.0));
                })
                .state
                .offset
                .y;
        })
        .discard_textures();
    (offset, zoom)
}

#[test]
fn plain_panel_smoothing_matches_native_egui_for_zero_delta_starts() {
    let routed = Context::default();
    let native = Context::default();
    let target = ScrollTarget::Panel(PANEL);
    let event = |unit, delta, phase| Event::MouseWheel {
        unit,
        delta: Vec2::new(0.0, delta),
        phase,
        modifiers: Modifiers::NONE,
    };
    let mut frames = vec![
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![event(MouseWheelUnit::Line, -1.0, TouchPhase::Move)],
        Vec::new(),
        vec![event(MouseWheelUnit::Point, -2.0, TouchPhase::Move)],
        vec![event(MouseWheelUnit::Point, 0.0, TouchPhase::Start)],
        vec![event(MouseWheelUnit::Line, -1.0, TouchPhase::Move)],
        vec![event(MouseWheelUnit::Point, -9.0, TouchPhase::End)],
        Vec::new(),
        vec![event(MouseWheelUnit::Page, -1.0, TouchPhase::Move)],
    ];
    frames.extend(std::iter::repeat_n(Vec::new(), 40));
    for (step, events) in frames.into_iter().enumerate() {
        let time = f64::from(u32::try_from(step).expect("frame index")) * 0.016;
        let actual = scroll_area_frame(&routed, time, target, events.clone()).0;
        let expected = scroll_area_frame_with_routing(&native, time, target, events, None).0;
        assert!(
            (actual - expected).abs() < 0.001,
            "frame {step}: {actual} vs native {expected}"
        );
    }
}

#[test]
fn panel_start_motion_is_delivered_once_and_modified_starts_are_excluded() {
    for modifiers in [Modifiers::NONE, Modifiers::CTRL, Modifiers::COMMAND] {
        for (unit, delta) in [
            (MouseWheelUnit::Point, 0.0),
            (MouseWheelUnit::Point, -5.0),
            (MouseWheelUnit::Point, -9.0),
            (MouseWheelUnit::Line, -1.0),
        ] {
            let ctx = Context::default();
            let target = ScrollTarget::Panel(PANEL);
            for time in [0.0, 0.016, 0.032] {
                let _ = scroll_area_frame(&ctx, time, target, Vec::new());
            }
            let start = Event::MouseWheel {
                unit,
                delta: Vec2::new(0.0, delta),
                phase: TouchPhase::Start,
                modifiers,
            };
            let actual = scroll_area_frame(&ctx, 1.0, target, vec![start]).0;
            let expected = if modifiers != Modifiers::NONE {
                0.0
            } else if unit == MouseWheelUnit::Line {
                -delta * InputOptions::default().line_scroll_speed
            } else {
                -delta
            };
            assert!(
                (actual - expected).abs() < 0.001,
                "{modifiers:?}: {actual} vs {expected}"
            );
            let idle = scroll_area_frame(&ctx, 1.016, target, Vec::new()).0;
            assert!((idle - expected).abs() < 0.001, "Start must not queue duplicate motion");
            let after_move =
                scroll_area_frame(&ctx, 1.032, target, vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]).0;
            assert!((after_move - expected - 5.0).abs() < 0.001);
        }
    }
}

#[test]
fn a_zoom_modified_start_keeps_plain_panel_moves_precise() {
    let ctx = Context::default();
    let target = ScrollTarget::Panel(PANEL);
    for time in [0.0, 0.016, 0.032] {
        let _ = scroll_area_frame(&ctx, time, target, Vec::new());
    }
    let events = vec![
        Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: Vec2::ZERO,
            phase: TouchPhase::Start,
            modifiers: Modifiers::CTRL,
        },
        Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: Vec2::new(0.0, -1.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
        },
    ];
    let (offset, zoom) = scroll_area_frame(&ctx, 1.0, target, events);
    assert!((offset - InputOptions::default().line_scroll_speed).abs() < 0.001);
    assert!((zoom - 1.0).abs() < f32::EPSILON);
}

#[test]
fn clearing_panel_smoothing_preserves_outside_input_and_fresh_notches() {
    for outside in [false, true] {
        let ctx = Context::default();
        let fresh = Context::default();
        let target = ScrollTarget::Panel(PANEL);
        for time in [0.0, 0.016, 0.032] {
            let _ = scroll_area_frame(&ctx, time, target, Vec::new());
            let _ = scroll_area_frame(&fresh, time, target, Vec::new());
        }
        let notch = Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: Vec2::new(0.0, -1.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
        };
        let initial = scroll_area_frame(&ctx, 1.0, target, vec![notch.clone()]).0;
        let _ = scroll_area_frame(&fresh, 1.0, target, Vec::new());
        let before = if outside {
            let offset = scroll_area_frame_with_routing(&ctx, 1.016, target, Vec::new(), Some(false)).0;
            assert!(offset > initial, "outside widgets retain egui's own easing");
            offset
        } else {
            super::super::reset_canvas_scroll(&ctx);
            initial
        };
        let _ = scroll_area_frame(&fresh, 1.016, target, Vec::new());
        let (offset, _) = scroll_area_frame(&ctx, 1.032, target, Vec::new());
        let _ = scroll_area_frame(&fresh, 1.032, target, Vec::new());
        assert!((offset - before).abs() < 0.001, "old panel easing was cleared");
        let actual = scroll_area_frame(&ctx, 1.048, target, vec![notch.clone()]).0 - before;
        let expected = scroll_area_frame(&fresh, 1.048, target, vec![notch]).0;
        assert!((actual - expected).abs() < 0.001, "fresh notch: {actual} vs {expected}");
    }
}
