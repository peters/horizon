//! The icon of the title bar of a panel: its words, what it can press, and the
//! keys that press it.

use std::time::{Duration, Instant};

use egui::{Context, Event, Id, Key, Modifiers, Pos2, RawInput, Rect, Vec2};
use horizon_core::{PanelId, ShortcutBinding, ShortcutKey, ShortcutModifiers};

use crate::test_egui::DiscardTextures;

use super::{mic_accessibility_label, mic_control_response, mic_rect, mic_widget_info, take_clock};
use crate::app::panels::PanelFrame;
use crate::app::speech::{MicState, PanelMic, panel_mic_enabled};

fn key_press(key: Key) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

fn idle_mic() -> PanelMic {
    PanelMic {
        state: MicState::Idle,
        enabled: true,
        active: false,
        started: None,
        hotkey: None,
    }
}

/// One frame of one icon, and whether a press of the keys of this frame reached it.
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
                        Id::unique("mic_keyboard_test"),
                        enabled,
                        MicState::Idle,
                        None,
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

#[test]
fn mic_widget_info_reports_button_state_and_label() {
    for (state, label) in [
        (MicState::Idle, "Start dictation"),
        (MicState::Recording, "Stop dictation; recording"),
        (MicState::Busy, "Dictation transcription in progress"),
    ] {
        let info = mic_widget_info(state, true, None);
        assert_eq!(info.role, egui::Role::Button);
        assert!(info.enabled);
        assert_eq!(info.label.as_deref(), Some(label));
        assert_eq!(info.selected, None);
        assert_eq!(info.current_text_value, None, "an icon without a take has no value");
        assert_eq!(mic_accessibility_label(state), label);
    }

    assert!(!mic_widget_info(MicState::Idle, false, None).enabled);
}

#[test]
fn the_clock_of_a_take_reaches_the_value_of_the_control() {
    // The words of a recording stay the words of a recording, and the clock of the
    // take is the value of the control, so a person who cannot see the icon reads
    // how long the take runs.
    let info = mic_widget_info(MicState::Recording, true, Some("0:03"));
    assert_eq!(info.label.as_deref(), Some("Stop dictation; recording"));
    assert_eq!(info.current_text_value.as_deref(), Some("0:03"));

    let started = Instant::now()
        .checked_sub(Duration::from_secs(63))
        .expect("a clock of the test");
    let mut mic = idle_mic();
    mic.state = MicState::Recording;
    mic.started = Some(started);
    assert_eq!(take_clock(&mic).as_deref(), Some("1:03"));

    // An icon that is not the recording of a take shows no clock.
    mic.state = MicState::Busy;
    assert_eq!(take_clock(&mic), None);
    mic.state = MicState::Idle;
    assert_eq!(take_clock(&mic), None);
}

#[test]
fn mic_control_availability_matches_engine_and_viewport_state() {
    assert!(panel_mic_enabled(true, false, MicState::Idle));
    assert!(panel_mic_enabled(true, true, MicState::Recording));
    assert!(!panel_mic_enabled(true, true, MicState::Idle));
    assert!(!panel_mic_enabled(true, true, MicState::Busy));

    for state in [MicState::Idle, MicState::Recording, MicState::Busy] {
        assert!(!panel_mic_enabled(false, true, state));
    }
}

#[test]
fn focused_mic_activates_once_from_enter_or_space() {
    for key in [Key::Enter, Key::Space] {
        let ctx = Context::default();
        assert!(!mic_frame(&ctx, Vec::new(), true, true).0);
        assert!(ctx.memory(|memory| memory.has_focus(Id::unique("mic_keyboard_test"))));
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
    assert_eq!(description, "", "a disabled icon says nothing");
}

#[test]
fn the_icon_sits_44_points_from_the_right_edge_on_the_title_bar_centre() {
    let panel = Rect::from_min_size(Pos2::new(40.0, 60.0), Vec2::new(520.0, 400.0));
    let frame = PanelFrame::new(panel);

    assert_eq!(frame.mic.size(), Vec2::splat(16.0));
    assert!((panel.max.x - frame.mic.center().x - 44.0).abs() <= f32::EPSILON);
    assert!((frame.mic.center().y - frame.titlebar.center().y).abs() <= f32::EPSILON);
    assert!(
        frame.titlebar.contains_rect(frame.mic),
        "the icon stays in the title bar"
    );
    assert_eq!(frame.mic, mic_rect(panel, frame.titlebar));
}

#[test]
fn the_icon_keeps_its_own_hit_target_beside_the_close_control() {
    // The hit targets of the icon and of the close control do not meet, so a press
    // on one of them never reaches the other.
    let panel = Rect::from_min_size(Pos2::new(40.0, 60.0), Vec2::new(520.0, 400.0));
    let frame = PanelFrame::new(panel);
    let mic_hit = frame.mic.expand2(Vec2::splat(4.0));
    let close_hit = frame.close.expand2(Vec2::splat(4.0));
    assert!(!mic_hit.intersects(close_hit), "{mic_hit:?} meets {close_hit:?}");

    // A press on the icon is the press of the icon: the drag area of the title bar
    // and the close control of the same rectangle stay quiet.
    let ctx = Context::default();
    let target = mic_hit.center();
    let mut clicked = (false, false, false);
    // The rects of a pass are what the events of the next pass are hit-tested
    // against, so the pointer comes over the icon one frame before the press.
    for events in [
        vec![Event::PointerMoved(target)],
        vec![Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
        vec![Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    ] {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(720.0, 480.0))),
            events,
            ..RawInput::default()
        };
        let _ = ctx
            .run_ui(input, |ui| {
                let _ = ui.allocate_exact_size(Vec2::new(720.0, 480.0), egui::Sense::hover());
                let drag = ui.interact(
                    Rect::from_min_size(Pos2::new(40.0, 60.0), Vec2::new(520.0, 34.0)),
                    Id::unique("mic_drag_test"),
                    egui::Sense::click_and_drag(),
                );
                let close = ui.interact(close_hit, Id::unique("mic_close_test"), egui::Sense::click());
                let mic = mic_control_response(ui, mic_hit, Id::unique("mic_target_test"), true, MicState::Idle, None);
                clicked = (mic.clicked(), drag.clicked(), close.clicked());
            })
            .discard_textures();
    }
    assert_eq!(clicked, (true, false, false), "only the icon takes the press");
}

#[test]
fn the_hover_sentence_names_the_state_and_the_hotkey() {
    let mic = PanelMic {
        hotkey: Some(ShortcutBinding::new(ShortcutModifiers::NONE, ShortcutKey::Function(9))),
        ..idle_mic()
    };
    assert_eq!(
        super::hover_text(&mic),
        format!(
            "Dictate into this panel (click, or hold: {})",
            ShortcutBinding::new(ShortcutModifiers::NONE, ShortcutKey::Function(9))
                .display_label(crate::app::speech::primary_modifier_label())
        )
    );

    // A setting with no hotkey, and a window where another panel records.
    assert_eq!(
        super::hover_text(&idle_mic()),
        "Dictate into this panel (click to start/stop)"
    );
    assert_eq!(
        super::hover_text(&PanelMic {
            active: true,
            ..idle_mic()
        }),
        "Dictation is active in another panel"
    );

    let mut recording = idle_mic();
    recording.state = MicState::Recording;
    recording.started = Instant::now().checked_sub(Duration::from_secs(3));
    assert_eq!(
        super::hover_text(&recording),
        "Recording into this panel (click to stop): 0:03"
    );
    assert_eq!(
        super::hover_text(&PanelMic {
            state: MicState::Busy,
            ..idle_mic()
        }),
        "Transcribing dictation for this panel"
    );
}

#[test]
fn the_icon_of_a_panel_is_the_square_left_of_the_close_control() {
    // The place of the icon does not depend on the state of speech input: the
    // title bar keeps its room and only the icon is drawn or not.
    let panel = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(320.0, 240.0));
    let frame = PanelFrame::new(panel);
    assert!(
        frame.mic.max.x < frame.close.min.x,
        "the icon stays left of the close control"
    );
    assert!(frame.mic.min.x > panel.min.x, "the icon stays inside the panel");
    assert_eq!(frame.mic.center().y, frame.close.center().y);
}

#[test]
fn the_icon_of_a_panel_keeps_its_widget_identity_between_frames() {
    // The keyboard focus of the icon belongs to the panel, so a frame that draws the
    // same panel again finds the same widget and another panel finds another one.
    let ctx = Context::default();
    let mic = idle_mic();
    let mut ids = Vec::new();
    for panel_id in [PanelId(4), PanelId(4), PanelId(5)] {
        let _ = ctx
            .run_ui(RawInput::default(), |ui| {
                let widget = super::show(ui, panel_id, Rect::from_min_size(Pos2::ZERO, Vec2::splat(16.0)), mic);
                ids.push(widget.response.id);
            })
            .discard_textures();
    }
    assert_eq!(ids[0], ids[1], "one panel holds one icon");
    assert_ne!(ids[0], ids[2], "two panels hold two icons");
}
