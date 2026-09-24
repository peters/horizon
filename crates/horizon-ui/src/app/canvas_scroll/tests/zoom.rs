use super::*;

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
fn a_zoom_modified_start_never_owns_or_chains_the_pan() {
    // winit's Wayland backend opens a touchpad gesture with a `Start` that
    // already carries motion. A Ctrl/Cmd one is a zoom step: it must not latch
    // the panel under the pointer, let alone ask whether that panel is
    // exhausted and chain it.
    let options = InputOptions::default();
    for zoom_modifier in [Modifiers::CTRL, Modifiers::COMMAND] {
        let zoom_wheel = |phase: TouchPhase, delta: Vec2| Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta,
            phase,
            modifiers: zoom_modifier,
        };
        let mut gesture = ScrollGesture::default();
        let mut judged = false;
        let routing = gesture.route(
            &pass(1.0, vec![zoom_wheel(TouchPhase::Start, Vec2::new(0.0, -5.0))]),
            &options,
            ScrollTarget::Panel(PANEL),
            true,
            &mut |_, _| {
                judged = true;
                true
            },
        );
        assert!(!judged, "a zoom step is never judged for chaining");
        assert!(routing.claimed_wheels.is_empty());
        assert!(!routing.pans_canvas);
        assert_eq!((gesture.canvas_owned, gesture.owner), (None, None));
        // Ctrl released mid-gesture: the first plain move latches where it is.
        let routing = gesture.route(
            &pass(1.016, vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]),
            &options,
            ScrollTarget::Canvas,
            true,
            &mut |_, _| false,
        );
        assert_eq!(routing.claimed_wheels, vec![0]);
        assert_eq!(routing.pan, Vec2::new(0.0, -5.0));
        // A modified `End` still ends the pan gesture.
        let _ = gesture.route(
            &pass(1.032, vec![zoom_wheel(TouchPhase::End, Vec2::ZERO)]),
            &options,
            ScrollTarget::Canvas,
            true,
            &mut |_, _| false,
        );
        assert_eq!(gesture.canvas_owned, None);
    }
}

#[test]
fn zoom_modified_gesture_boundaries_end_the_pan_but_stay_zoom_steps() {
    // A Ctrl/Cmd `End`, `Cancel` or `Start` can carry a delta. It still ends
    // the pan gesture and lands that gesture's eased motion in full, but its
    // own delta is a zoom step: never judged, claimed or panned, whoever owns
    // the gesture. A plain `Start` lands the old motion the same way.
    let options = InputOptions::default();
    let notch = Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: Vec2::new(0.0, -1.0),
        phase: TouchPhase::Move,
        modifiers: Modifiers::NONE,
    };
    let boundary = |phase, modifiers, delta| Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta,
        phase,
        modifiers,
    };
    let mut cases = vec![boundary(TouchPhase::Start, Modifiers::NONE, Vec2::ZERO)];
    for phase in [TouchPhase::End, TouchPhase::Cancel, TouchPhase::Start] {
        for modifiers in [Modifiers::CTRL, Modifiers::COMMAND] {
            cases.push(boundary(phase, modifiers, Vec2::new(0.0, -5.0)));
        }
    }
    for event in cases {
        let Event::MouseWheel { modifiers, .. } = event else {
            unreachable!()
        };
        // A canvas gesture with a mouse-wheel notch still easing in.
        let mut gesture = ScrollGesture::default();
        let first = gesture.route(
            &pass(1.0, vec![notch.clone()]),
            &options,
            ScrollTarget::Canvas,
            true,
            &mut |_, _| false,
        );
        assert!(first.pan.y > -options.line_scroll_speed, "the notch is still easing");
        let last = gesture.route(
            &pass(1.016, vec![event.clone()]),
            &options,
            ScrollTarget::Canvas,
            true,
            &mut |_, _| false,
        );
        assert!(
            ((first.pan + last.pan).y + options.line_scroll_speed).abs() < 0.001,
            "{event:?}: exactly one line lands"
        );
        if modifiers.is_none() {
            continue;
        }
        assert!(last.claimed_wheels.is_empty(), "{event:?}");
        assert_eq!(gesture.canvas_owned, None);
        // A panel-owned gesture is never judged for chaining by it.
        let mut gesture = ScrollGesture::default();
        let _ = gesture.route(
            &pass(2.0, vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]),
            &options,
            ScrollTarget::Panel(PANEL),
            true,
            &mut |_, _| false,
        );
        let mut judged = false;
        let routing = gesture.route(
            &pass(2.016, vec![event.clone()]),
            &options,
            ScrollTarget::Panel(PANEL),
            true,
            &mut |_, _| {
                judged = true;
                true
            },
        );
        assert!(!judged, "{event:?}");
        assert!(!routing.pans_canvas);
        assert_eq!(gesture.canvas_owned, None);
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

fn zoom_frame(ctx: &Context, time: f64, events: Vec<Event>, over_canvas: bool) -> f32 {
    let mut zoom = 1.0;
    let input = RawInput {
        time: Some(time),
        events,
        ..RawInput::default()
    };
    let _ = ctx
        .run_ui(input, |ui| zoom = canvas_zoom_delta(ui.ctx(), over_canvas))
        .discard_textures();
    zoom
}

fn zoom_notch() -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: Vec2::new(0.0, -1.0),
        phase: TouchPhase::Move,
        modifiers: Modifiers::COMMAND,
    }
}

#[test]
fn a_zoom_wheel_notch_eases_in_to_exactly_one_line_of_egui_zoom() {
    let options = InputOptions::default();
    let ctx = Context::default();
    let zoom_at = |time: f64, events: Vec<Event>| zoom_frame(&ctx, time, events, true);
    let first = zoom_at(1.0, vec![zoom_notch()]);
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
    // A Wayland touchpad gesture opens with a `Start` that already moves;
    // egui drops that delta, the canvas zooms by it.
    let start = Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -5.0),
        phase: TouchPhase::Start,
        modifiers: Modifiers::COMMAND,
    };
    assert!((zoom_at(5.0, vec![start]) - expected).abs() < 1e-6);
}

#[test]
fn a_zoom_notch_off_the_canvas_never_zooms_it_later() {
    // The canvas applies zoom only while the pointer is over it. A notch
    // turned over the sidebar, or one still easing in when the pointer
    // leaves, must not zoom the canvas once the pointer comes back.
    let ctx = Context::default();
    let mut total = zoom_frame(&ctx, 1.0, vec![zoom_notch()], false);
    for frame in 1..60 {
        total *= zoom_frame(&ctx, 1.0 + f64::from(frame) / 60.0, Vec::new(), true);
    }
    assert!((total - 1.0).abs() < f32::EPSILON, "{total}");
    let started = zoom_frame(&ctx, 3.0, vec![zoom_notch()], true);
    assert!(started < 1.0, "a notch over the canvas starts easing in");
    let _ = zoom_frame(&ctx, 3.016, Vec::new(), false);
    for frame in 2..60 {
        let zoom = zoom_frame(&ctx, 3.0 + f64::from(frame) / 60.0, Vec::new(), true);
        assert!((zoom - 1.0).abs() < f32::EPSILON, "frame {frame}: {zoom}");
    }
}

#[test]
fn a_new_zoom_contact_lands_the_previous_notch_in_full() {
    for modifiers in [Modifiers::NONE, Modifiers::CTRL, Modifiers::COMMAND] {
        let ctx = Context::default();
        let first = zoom_frame(&ctx, 1.0, vec![zoom_notch()], true);
        let boundary = Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: Vec2::ZERO,
            phase: TouchPhase::Start,
            modifiers,
        };
        let last = zoom_frame(&ctx, 1.016, vec![boundary], true);
        let options = InputOptions::default();
        let expected = (-options.line_scroll_speed * options.scroll_zoom_speed).exp();
        assert!(
            (first * last - expected).abs() < 1e-5,
            "{modifiers:?}: the old notch lands at Start"
        );
        assert!((zoom_frame(&ctx, 1.032, Vec::new(), true) - 1.0).abs() < f32::EPSILON);
    }
}

fn zoom_contact(phase: TouchPhase, modifiers: Modifiers) -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -5.0),
        phase,
        modifiers,
    }
}

#[test]
fn a_zoom_contact_started_outside_stays_rejected_until_its_boundary() {
    for modifiers in [Modifiers::NONE, Modifiers::CTRL, Modifiers::COMMAND] {
        for end in [TouchPhase::End, TouchPhase::Cancel] {
            let ctx = Context::default();
            let _ = zoom_frame(&ctx, 1.0, vec![zoom_contact(TouchPhase::Start, modifiers)], false);
            for (time, phase) in [(1.016, TouchPhase::Move), (1.032, end)] {
                let zoom = zoom_frame(&ctx, time, vec![zoom_contact(phase, Modifiers::CTRL)], true);
                assert!((zoom - 1.0).abs() < f32::EPSILON, "rejected contact at {phase:?}");
            }
            assert!(
                zoom_frame(
                    &ctx,
                    1.048,
                    vec![zoom_contact(TouchPhase::Start, Modifiers::CTRL)],
                    true
                ) < 1.0
            );
        }
    }
}

#[test]
fn a_zoom_contact_cannot_resume_after_leaving_the_canvas() {
    let ctx = Context::default();
    assert!(zoom_frame(&ctx, 1.0, vec![zoom_contact(TouchPhase::Start, Modifiers::CTRL)], true) < 1.0);
    let _ = zoom_frame(&ctx, 1.016, Vec::new(), false);
    let zoom = zoom_frame(&ctx, 1.032, vec![zoom_contact(TouchPhase::Move, Modifiers::CTRL)], true);
    assert!((zoom - 1.0).abs() < f32::EPSILON);
    assert!(
        zoom_frame(
            &ctx,
            1.048,
            vec![zoom_contact(TouchPhase::Start, Modifiers::CTRL)],
            true
        ) < 1.0
    );

    let unphased = Context::default();
    let _ = zoom_frame(&unphased, 1.0, vec![zoom_notch()], false);
    assert!(zoom_frame(&unphased, 1.016, vec![zoom_notch()], true) < 1.0);
}

#[test]
fn a_suppressed_zoom_contact_observes_boundaries_without_reaccepting_moves() {
    for boundary in [
        None,
        Some(TouchPhase::Start),
        Some(TouchPhase::End),
        Some(TouchPhase::Cancel),
    ] {
        let ctx = Context::default();
        let _ = zoom_frame(&ctx, 1.0, vec![zoom_contact(TouchPhase::Start, Modifiers::CTRL)], true);
        let input = RawInput {
            time: Some(1.016),
            events: boundary
                .map(|phase| zoom_contact(phase, Modifiers::CTRL))
                .into_iter()
                .collect(),
            ..RawInput::default()
        };
        let _ = ctx
            .run_ui(input, |ui| super::super::reset_canvas_scroll(ui.ctx()))
            .discard_textures();
        let zoom = zoom_frame(&ctx, 1.032, vec![zoom_contact(TouchPhase::Move, Modifiers::CTRL)], true);
        if matches!(boundary, Some(TouchPhase::End | TouchPhase::Cancel)) {
            assert!(zoom < 1.0, "a closed contact permits later unphased wheels");
        } else {
            assert!((zoom - 1.0).abs() < f32::EPSILON, "suppressed {boundary:?} contact");
        }
    }
}

#[test]
fn losing_focus_rejects_a_zoom_contact_and_discards_unphased_easing() {
    for phased in [false, true] {
        let ctx = Context::default();
        let initial = if phased {
            zoom_contact(TouchPhase::Start, Modifiers::CTRL)
        } else {
            zoom_notch()
        };
        assert!(zoom_frame(&ctx, 1.0, vec![initial], true) < 1.0);
        let mut zoom = 0.0;
        let _ = ctx
            .run_ui(
                RawInput {
                    time: Some(1.016),
                    focused: false,
                    ..RawInput::default()
                },
                |ui| {
                    zoom = canvas_zoom_delta(ui.ctx(), true);
                },
            )
            .discard_textures();
        assert!((zoom - 1.0).abs() < f32::EPSILON);
        assert!((zoom_frame(&ctx, 1.032, Vec::new(), true) - 1.0).abs() < f32::EPSILON);
        let zoom = zoom_frame(&ctx, 1.048, vec![zoom_contact(TouchPhase::Move, Modifiers::CTRL)], true);
        if phased {
            assert!((zoom - 1.0).abs() < f32::EPSILON);
        } else {
            assert!(zoom < 1.0);
        }
    }
}
