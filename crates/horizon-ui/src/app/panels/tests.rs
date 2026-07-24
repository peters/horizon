use egui::{Context, Event, Id, Key, Modifiers, Pos2, RawInput, Rect, Vec2};

use super::{
    MicState, clip_screen_rect_to_canvas, mic_accessibility_label, mic_control_enabled, mic_control_response,
    mic_widget_info,
};

fn key_press(key: Key) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

fn mic_frame(ctx: &Context, events: Vec<Event>, enabled: bool, request_focus: bool) -> (bool, String) {
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 200.0))),
        events,
        ..RawInput::default()
    };
    input.viewport_id = egui::ViewportId::ROOT;
    ctx.begin_pass(input);
    let clicked = egui::CentralPanel::default()
        .show(ctx, |ui| {
            let response = mic_control_response(
                ui,
                Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::splat(24.0)),
                Id::new("mic_keyboard_test"),
                enabled,
                MicState::Idle,
            );
            if request_focus {
                response.request_focus();
            }
            response.clicked()
        })
        .inner;
    let output = ctx.end_pass();
    (clicked, output.platform_output.events_description())
}

#[test]
fn clip_screen_rect_to_canvas_intersects_with_canvas_bounds() {
    let canvas_rect = Rect::from_min_max(Pos2::new(100.0, 80.0), Pos2::new(420.0, 320.0));
    let raw_rect = Rect::from_min_max(Pos2::new(60.0, 40.0), Pos2::new(180.0, 180.0));

    assert_eq!(
        clip_screen_rect_to_canvas(raw_rect, canvas_rect),
        Some(Rect::from_min_max(Pos2::new(100.0, 80.0), Pos2::new(180.0, 180.0)))
    );
}

#[test]
fn clip_screen_rect_to_canvas_rejects_non_positive_intersections() {
    let canvas_rect = Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::new(320.0, 240.0));
    let raw_rect = Rect::from_min_size(Pos2::new(430.0, 90.0), Vec2::new(80.0, 80.0));

    assert_eq!(clip_screen_rect_to_canvas(raw_rect, canvas_rect), None);
}

#[test]
fn mic_widget_info_reports_button_state_and_label() {
    for (state, label) in [
        (MicState::Idle, "Start dictation"),
        (MicState::Recording, "Stop dictation; recording"),
        (MicState::Busy, "Dictation transcription in progress"),
    ] {
        let info = mic_widget_info(state, true);
        assert_eq!(info.typ, egui::WidgetType::Button);
        assert!(info.enabled);
        assert_eq!(info.label.as_deref(), Some(label));
        assert_eq!(info.selected, None);
        assert_eq!(mic_accessibility_label(state), label);
    }

    assert!(!mic_widget_info(MicState::Idle, false).enabled);
}

#[test]
fn mic_control_availability_matches_engine_and_viewport_state() {
    assert!(mic_control_enabled(true, false, MicState::Idle));
    assert!(mic_control_enabled(true, true, MicState::Recording));
    assert!(!mic_control_enabled(true, true, MicState::Idle));
    assert!(!mic_control_enabled(true, true, MicState::Busy));

    for state in [MicState::Idle, MicState::Recording, MicState::Busy] {
        assert!(!mic_control_enabled(false, true, state));
    }
}

#[test]
fn focused_mic_activates_once_from_enter_or_space() {
    for key in [Key::Enter, Key::Space] {
        let ctx = Context::default();
        assert!(!mic_frame(&ctx, Vec::new(), true, true).0);
        assert!(ctx.memory(|memory| memory.has_focus(Id::new("mic_keyboard_test"))));
        let (clicked, description) = mic_frame(&ctx, vec![key_press(key)], true, false);
        assert!(clicked);
        assert_eq!(description, "Start dictation: button");
    }
}

#[test]
fn disabled_mic_ignores_focused_keyboard_activation() {
    let ctx = Context::default();
    assert!(!mic_frame(&ctx, Vec::new(), true, true).0);
    let (clicked, description) = mic_frame(&ctx, vec![key_press(Key::Enter)], false, false);
    assert!(!clicked);
    assert!(description.is_empty());
}
