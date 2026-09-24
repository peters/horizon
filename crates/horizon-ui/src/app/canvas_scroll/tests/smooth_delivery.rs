use super::*;

#[test]
fn displaced_panel_wheels_do_not_reach_another_panels_scroll_area() {
    assert_displaced_wheels_stay_with_owner(ScrollTarget::Panel(PANEL));
}

#[test]
fn displaced_surface_wheels_do_not_reach_a_panels_scroll_area() {
    assert_displaced_wheels_stay_with_owner(ScrollTarget::Surface);
}

fn assert_displaced_wheels_stay_with_owner(owner: ScrollTarget) {
    let ctx = Context::default();
    let frame = |time, target, events| {
        let mut input = raw_input([400.0, 300.0], None);
        input.time = Some(time);
        input.events = std::iter::once(Event::PointerMoved(Pos2::new(100.0, 100.0)))
            .chain(events)
            .collect();
        let mut offset = 0.0;
        let _ = ctx
            .run_ui(input, |ui| {
                let mut routing = route_canvas_scroll(ui.ctx(), target, true, |_, _| false);
                routing.discard_displaced_wheels(target);
                routing.defer_for_panel_delivery(ui.ctx());
                routing.consume(ui.ctx(), &mut Vec::new());
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
        offset
    };
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
