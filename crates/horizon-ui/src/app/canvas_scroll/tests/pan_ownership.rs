use super::*;

#[test]
fn a_surface_contact_cannot_deliver_raw_wheels_to_a_panel() {
    let mut gesture = ScrollGesture::default();
    let options = InputOptions::default();
    let _ = gesture.route(
        &pass(1.0, vec![wheel(Vec2::ZERO, TouchPhase::Start)]),
        &options,
        ScrollTarget::Surface(901),
        true,
        &mut |_, _| false,
    );
    let routing = gesture.route(
        &pass(1.016, vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]),
        &options,
        ScrollTarget::Panel(PANEL),
        true,
        &mut |_, _| panic!("a surface never checks panel extent"),
    );
    assert_eq!(routing.claimed_wheels, vec![0]);
    assert!(!routing.pans_canvas);
    assert!(routing.owns_smooth_scroll);
}

#[test]
fn a_pan_contact_started_outside_does_not_rebind_when_it_enters() {
    for end in [TouchPhase::End, TouchPhase::Cancel] {
        let mut gesture = ScrollGesture::default();
        let options = InputOptions::default();
        let _ = gesture.route(
            &pass(1.0, vec![wheel(Vec2::ZERO, TouchPhase::Start)]),
            &options,
            ScrollTarget::Canvas,
            false,
            &mut |_, _| false,
        );
        for (time, phase) in [(1.016, TouchPhase::Move), (1.032, end)] {
            let routing = gesture.route(
                &pass(time, vec![wheel(Vec2::new(0.0, -5.0), phase)]),
                &options,
                ScrollTarget::Canvas,
                true,
                &mut |_, _| false,
            );
            assert!(!routing.pans_canvas, "rejected contact at {phase:?}");
            assert_eq!(routing.claimed_wheels, vec![0], "rejected motion cannot leak to panels");
        }
        assert!(claim(
            &mut gesture,
            1.048,
            true,
            vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Start)]
        ));
    }
}

#[test]
fn a_pan_contact_cannot_rebind_after_disallowed_input_or_focus_loss() {
    for focused in [false, true] {
        let mut gesture = ScrollGesture::default();
        let options = InputOptions::default();
        let _ = gesture.route(
            &pass(1.0, vec![wheel(Vec2::ZERO, TouchPhase::Start)]),
            &options,
            ScrollTarget::Panel(PANEL),
            true,
            &mut |_, _| false,
        );
        let mut input = pass(1.016, Vec::new());
        input.focused = focused;
        let _ = gesture.route(&input, &options, ScrollTarget::Canvas, !focused, &mut |_, _| false);
        assert!(!claim(
            &mut gesture,
            1.032,
            true,
            vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)]
        ));
        assert!(claim(
            &mut gesture,
            1.048,
            true,
            vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Start)]
        ));
    }
}

#[test]
fn suppression_preserves_rejected_pan_contacts_until_an_observed_boundary() {
    for boundary in [
        None,
        Some(TouchPhase::Start),
        Some(TouchPhase::End),
        Some(TouchPhase::Cancel),
    ] {
        let ctx = Context::default();
        let frame = |time, events, suppressed| {
            let mut result = false;
            let input = RawInput {
                time: Some(time),
                events,
                ..RawInput::default()
            };
            let _ = ctx
                .run_ui(input, |ui| {
                    if suppressed {
                        super::super::reset_canvas_scroll(ui.ctx());
                    } else {
                        result = route_canvas_scroll(ui.ctx(), ScrollTarget::Canvas, true, |_, _| false).pans_canvas;
                    }
                })
                .discard_textures();
            result
        };
        assert!(frame(1.0, vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Start)], false));
        let _ = frame(
            1.016,
            boundary.map(|phase| wheel(Vec2::ZERO, phase)).into_iter().collect(),
            true,
        );
        assert_eq!(
            frame(1.032, vec![wheel(Vec2::new(0.0, -5.0), TouchPhase::Move)], false),
            matches!(boundary, Some(TouchPhase::End | TouchPhase::Cancel))
        );
    }
}
