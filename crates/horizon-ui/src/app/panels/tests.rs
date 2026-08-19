use crate::test_egui::DiscardTextures;
use egui::{Context, Event, Id, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use horizon_core::{AgentSessionBinding, PanelKind};

use super::{
    MicState, clip_screen_rect_to_canvas, mic_accessibility_label, mic_control_enabled, mic_control_response,
    mic_widget_info, render_session_rebind_options,
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
    let mut clicked = false;
    let output = ctx
        .run_ui(input, |ui| {
            clicked = egui::CentralPanel::default()
                .show(ui, |ui| {
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
        })
        .discard_textures();
    (clicked, output.platform_output.events_description())
}

fn rebind_options_frame(
    ctx: &Context,
    events: Vec<Event>,
    options: &[(String, AgentSessionBinding)],
) -> super::SessionRebindRenderOutcome {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 320.0))),
        events,
        ..RawInput::default()
    };
    let mut outcome = None;
    let _ = ctx
        .run_ui(input, |ui| {
            outcome = Some(
                egui::CentralPanel::default()
                    .show(ui, |ui| render_session_rebind_options(ui, options))
                    .inner,
            );
        })
        .discard_textures();
    outcome.expect("rebind options frame ran")
}

fn session_binding(session_id: &str) -> AgentSessionBinding {
    AgentSessionBinding::new(
        PanelKind::Codex,
        session_id.to_string(),
        Some("/repo".to_string()),
        None,
        None,
    )
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

#[test]
fn clicking_a_rebind_and_restart_row_returns_the_exact_session() {
    let ctx = Context::default();
    let options = vec![
        ("First session · 11111111".to_string(), session_binding("session-1")),
        ("Second session · 22222222".to_string(), session_binding("session-2")),
    ];
    let initial = rebind_options_frame(&ctx, Vec::new(), &options);
    let second_row = initial.option_rects.get(1).expect("second rebind row").center();

    let pressed = rebind_options_frame(
        &ctx,
        vec![
            Event::PointerMoved(second_row),
            Event::PointerButton {
                pos: second_row,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
        &options,
    );
    assert!(pressed.binding.is_none());

    let released = rebind_options_frame(
        &ctx,
        vec![Event::PointerButton {
            pos: second_row,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
        &options,
    );

    assert_eq!(released.binding, Some(options[1].1.clone()));
}
