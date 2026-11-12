//! Interact sends a person's keys and text to a frame source program, and only
//! while the picture has the keyboard.
use super::*;
use horizon_frame_source::{InputEvent, InputHello};

const SCREEN: egui::Vec2 = egui::vec2(1200.0, 800.0);

fn frame_device() -> DevicePanelState {
    DevicePanelState {
        target: horizon_core::DeviceViewTarget::frame_source(horizon_core::FrameSourceRef {
            name: "standin".into(),
            target: None,
        }),
        input: horizon_core::DeviceInput::Accepted,
        identity: None,
        connect_on_start: false,
        ssh_tunnel: None,
    }
}

fn frame_viewer() -> (
    egui::Context,
    DevicePanelState,
    DeviceUiState,
    horizon_frame_source::SentFrameInput,
) {
    let ctx = egui::Context::default();
    ctx.all_styles_mut(|style| style.animation_time = 0.0);
    let (session, sent) = Session::frame_fixture();
    let mut state = DeviceUiState {
        initialized: true,
        status: Status::Connected,
        session: Some(session),
        frame_input: Some(InputHello {
            allowed: true,
            reason: None,
        }),
        ..DeviceUiState::default()
    };
    // One picture of 4:3, so a panel of another shape leaves a letterbox beside it.
    state.set_source(&ctx, ColorImage::filled([160, 120], egui::Color32::GREEN));
    (ctx, frame_device(), state, sent)
}

fn frame(
    ctx: &egui::Context,
    state: &mut DeviceUiState,
    device: &DevicePanelState,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    state.begin_frame();
    let output = ctx
        .run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
                ..Default::default()
            },
            |ui| {
                if state.source.is_none() {
                    state.update_texture(ui.ctx(), ColorImage::filled([160, 120], egui::Color32::GREEN));
                }
                state.show(ui, device, true);
            },
        )
        .discard_textures();
    state.finish_frame(ctx);
    output
}

fn key_event(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers,
    }
}

fn named(events: &[InputEvent]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            InputEvent::Key { key, down } => Some((key.name(), *down)),
            _ => None,
        })
        .collect()
}

/// One sent button: its name, whether it went down, and the point it was sent at.
type SentButton = (String, bool, Option<(u32, u32)>);

fn buttons(events: &[InputEvent]) -> Vec<SentButton> {
    events
        .iter()
        .filter_map(|event| match event {
            InputEvent::Button { button, down, at } => {
                Some((button.name().to_owned(), *down, at.map(|at| (at.x, at.y))))
            }
            _ => None,
        })
        .collect()
}

fn pointers(events: &[InputEvent]) -> Vec<(u32, u32)> {
    events
        .iter()
        .filter_map(|event| match event {
            InputEvent::Pointer { at } => Some((at.x, at.y)),
            _ => None,
        })
        .collect()
}

fn texts(events: &[InputEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            InputEvent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A viewer with Interact on and the keyboard in the picture, as a person leaves it
/// after a click on the image.
fn captured_viewer() -> (
    egui::Context,
    DevicePanelState,
    DeviceUiState,
    horizon_frame_source::SentFrameInput,
) {
    let (ctx, device, mut state, mut sent) = frame_viewer();
    click_label(&ctx, &mut state, &device, "Interact");
    assert!(state.interact);
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let middle = image_rect(&output).center();
    frame(&ctx, &mut state, &device, click_events(middle, true));
    frame(&ctx, &mut state, &device, click_events(middle, false));
    assert!(state.captured, "the click on the picture took the keyboard");
    sent.drain();
    (ctx, device, state, sent)
}

#[test]
fn the_picture_takes_the_keyboard_and_the_text_of_a_frame_is_one_message() {
    let (ctx, device, mut state, mut sent) = frame_viewer();
    click_label(&ctx, &mut state, &device, "Interact");
    // Interact is on, but no press has reached the picture yet: no key leaves.
    let output = frame(&ctx, &mut state, &device, Vec::new());
    frame(&ctx, &mut state, &device, vec![egui::Event::Text("no".into())]);
    assert!(
        texts(&sent.drain()).is_empty(),
        "a picture without the keyboard sends no text"
    );

    let middle = image_rect(&output).center();
    frame(&ctx, &mut state, &device, click_events(middle, true));
    frame(&ctx, &mut state, &device, click_events(middle, false));
    assert!(state.captured, "the click on the picture took the keyboard");
    sent.drain();

    frame(
        &ctx,
        &mut state,
        &device,
        vec![egui::Event::Text("h".into()), egui::Event::Text("i".into())],
    );
    assert_eq!(
        texts(&sent.drain()),
        vec!["hi".to_owned()],
        "the characters of one frame are one text message"
    );

    frame(
        &ctx,
        &mut state,
        &device,
        vec![egui::Event::Ime(egui::ImeEvent::Commit("ø".into()))],
    );
    assert_eq!(texts(&sent.drain()), vec!["ø".to_owned()]);
}

#[test]
fn every_named_key_reaches_the_program_as_its_own_name() {
    let (ctx, device, mut state, mut sent) = captured_viewer();
    for (key, name) in [
        (egui::Key::ArrowUp, "up"),
        (egui::Key::ArrowDown, "down"),
        (egui::Key::Enter, "enter"),
        (egui::Key::Escape, "escape"),
        (egui::Key::Tab, "tab"),
        (egui::Key::Backspace, "backspace"),
        (egui::Key::Delete, "delete"),
        (egui::Key::F1, "f1"),
        (egui::Key::F12, "f12"),
    ] {
        frame(
            &ctx,
            &mut state,
            &device,
            vec![
                key_event(key, true, egui::Modifiers::NONE),
                key_event(key, false, egui::Modifiers::NONE),
            ],
        );
        assert_eq!(
            named(&sent.drain()),
            vec![(name.to_owned(), true), (name.to_owned(), false)],
            "{name} goes down and up"
        );
        assert!(state.captured, "{name} stays in the picture");
    }
}

#[test]
fn a_chord_reaches_the_program_as_keys_and_no_text() {
    let (ctx, device, mut state, mut sent) = captured_viewer();
    frame(
        &ctx,
        &mut state,
        &device,
        vec![
            egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
            key_event(egui::Key::C, true, egui::Modifiers::CTRL),
            key_event(egui::Key::C, false, egui::Modifiers::CTRL),
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
        ],
    );
    let sent_events = sent.drain();
    assert_eq!(
        named(&sent_events),
        vec![
            ("ctrl".to_owned(), true),
            ("c".to_owned(), true),
            ("c".to_owned(), false),
            ("ctrl".to_owned(), false)
        ],
        "the chord holds Ctrl around c"
    );
    assert!(texts(&sent_events).is_empty(), "a chord is no text");

    // What egui-winit reports for a quick command+C: the chord as Copy, then the
    // release of c, and the command key up by the end of the frame.
    let (command_modifier, command) = if cfg!(target_os = "macos") {
        (egui::Modifiers::MAC_CMD, "super")
    } else {
        (egui::Modifiers::CTRL, "ctrl")
    };
    frame(
        &ctx,
        &mut state,
        &device,
        vec![
            egui::Event::ModifiersChanged(command_modifier),
            egui::Event::Copy,
            key_event(egui::Key::C, false, command_modifier),
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
        ],
    );
    let copied = sent.drain();
    assert_eq!(
        named(&copied),
        vec![
            (command.to_owned(), true),
            ("c".to_owned(), true),
            ("c".to_owned(), false),
            (command.to_owned(), false)
        ],
        "one clipboard chord, and no second release of c"
    );
    assert!(texts(&copied).is_empty());
}

#[test]
fn a_key_that_horizon_handled_as_its_shortcut_is_not_sent() {
    let (ctx, device, mut state, mut sent) = captured_viewer();
    let binding = horizon_core::ShortcutBinding::parse("Ctrl+K").expect("a shortcut of the settings");
    state.begin_frame();
    let _ = ctx
        .run_ui(
            egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                    key_event(egui::Key::K, true, egui::Modifiers::CTRL),
                    key_event(egui::Key::K, false, egui::Modifiers::CTRL),
                    egui::Event::ModifiersChanged(egui::Modifiers::NONE),
                ],
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
                ..Default::default()
            },
            |ui| {
                // The shortcut of the app is read before the panels are drawn.
                crate::app::shortcuts::consume_shortcut(ui.ctx(), binding);
                state.show(ui, &device, true);
            },
        )
        .discard_textures();
    state.finish_frame(&ctx);
    let sent_events = sent.drain();
    assert!(
        !named(&sent_events).iter().any(|(name, _)| name == "k"),
        "a chord of Horizon never reaches the program: {sent_events:?}"
    );
    assert!(texts(&sent_events).is_empty());
}

#[test]
fn a_key_that_was_never_sent_down_is_not_released() {
    let (ctx, device, mut state, mut sent) = captured_viewer();
    // Only the release arrives, as it does for a press that Horizon consumed.
    frame(
        &ctx,
        &mut state,
        &device,
        vec![key_event(egui::Key::Enter, false, egui::Modifiers::NONE)],
    );
    assert!(
        named(&sent.drain()).is_empty(),
        "a key that never went down is not released"
    );
}

#[test]
fn a_press_outside_the_picture_gives_the_keyboard_back_and_releases_held_keys() {
    let (ctx, device, mut state, mut sent) = captured_viewer();
    frame(
        &ctx,
        &mut state,
        &device,
        vec![key_event(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    assert_eq!(named(&sent.drain()), vec![("enter".to_owned(), true)]);

    let outside = egui::Pos2::new(SCREEN.x - 4.0, SCREEN.y - 4.0);
    frame(&ctx, &mut state, &device, click_events(outside, true));
    frame(&ctx, &mut state, &device, click_events(outside, false));
    assert!(!state.captured);
    assert_eq!(
        named(&sent.drain()),
        vec![("enter".to_owned(), false)],
        "the held key is released when the focus leaves"
    );
    frame(&ctx, &mut state, &device, vec![egui::Event::Text("x".into())]);
    assert!(texts(&sent.drain()).is_empty(), "no keyboard, no text");
}

#[test]
fn turning_interact_off_releases_a_held_key() {
    let (ctx, device, mut state, mut sent) = captured_viewer();
    frame(
        &ctx,
        &mut state,
        &device,
        vec![key_event(egui::Key::Tab, true, egui::Modifiers::NONE)],
    );
    assert_eq!(named(&sent.drain()), vec![("tab".to_owned(), true)]);
    click_label(&ctx, &mut state, &device, "Interact");
    assert!(!state.interact);
    assert_eq!(named(&sent.drain()), vec![("tab".to_owned(), false)]);
}

#[test]
fn a_viewer_that_is_not_drawn_releases_a_held_key() {
    let (ctx, device, mut state, mut sent) = captured_viewer();
    frame(
        &ctx,
        &mut state,
        &device,
        vec![key_event(egui::Key::F5, true, egui::Modifiers::NONE)],
    );
    assert_eq!(named(&sent.drain()), vec![("f5".to_owned(), true)]);
    state.begin_frame();
    state.finish_frame(&ctx);
    assert_eq!(named(&sent.drain()), vec![("f5".to_owned(), false)]);
}

/// A viewer with Interact on and nothing clicked in the picture yet.
fn interacting_viewer() -> (
    egui::Context,
    DevicePanelState,
    DeviceUiState,
    horizon_frame_source::SentFrameInput,
) {
    let (ctx, device, mut state, sent) = frame_viewer();
    click_label(&ctx, &mut state, &device, "Interact");
    assert!(state.interact);
    (ctx, device, state, sent)
}

#[test]
fn the_click_that_turns_interact_on_is_not_sent() {
    let (ctx, device, mut state, mut sent) = interacting_viewer();
    frame(&ctx, &mut state, &device, Vec::new());
    assert!(
        sent.drain().is_empty(),
        "the press on the Interact control belongs to Horizon, not to the program"
    );
}

#[test]
fn a_click_beside_the_picture_is_not_sent() {
    let (ctx, device, mut state, mut sent) = interacting_viewer();
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let image = image_rect(&output);
    // The chrome of the panel just above the picture, and the letterbox of its
    // own row to the right of a picture of another shape than the panel.
    let chrome = egui::Pos2::new(image.center().x, image.min.y - 6.0);
    let letterbox = egui::Pos2::new(image.max.x + 30.0, image.center().y);
    assert!(image.max.x + 30.0 < SCREEN.x, "the letterbox is on the screen");
    for point in [chrome, letterbox] {
        frame(&ctx, &mut state, &device, click_events(point, true));
        frame(&ctx, &mut state, &device, click_events(point, false));
        let sent_events = sent.drain();
        assert!(
            buttons(&sent_events).is_empty(),
            "a click at {point:?} beside the picture reaches no program: {sent_events:?}"
        );
        assert!(
            pointers(&sent_events).is_empty(),
            "and it moves no pointer of the picture: {sent_events:?}"
        );
    }
}

#[test]
fn a_release_is_sent_only_for_a_press_that_was_sent() {
    let (ctx, device, mut state, mut sent) = interacting_viewer();
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let image = image_rect(&output);
    let letterbox = egui::Pos2::new(image.max.x + 30.0, image.center().y);
    frame(&ctx, &mut state, &device, click_events(letterbox, true));
    frame(
        &ctx,
        &mut state,
        &device,
        vec![egui::Event::PointerMoved(image.center())],
    );
    frame(&ctx, &mut state, &device, click_events(image.center(), false));
    let sent_events = sent.drain();
    assert!(
        buttons(&sent_events).is_empty(),
        "a release without a press of the picture is no event: {sent_events:?}"
    );
    assert_eq!(
        pointers(&sent_events),
        vec![(80, 60)],
        "the pointer that entered the picture is the only event of its own"
    );
}

#[test]
fn a_release_beside_the_picture_keeps_the_last_point_the_program_saw() {
    let (ctx, device, mut state, mut sent) = interacting_viewer();
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let image = image_rect(&output);
    frame(&ctx, &mut state, &device, click_events(image.center(), true));
    assert_eq!(
        buttons(&sent.drain()),
        vec![("left".to_owned(), true, Some((80, 60)))],
        "the press is at the middle of the picture"
    );
    let letterbox = egui::Pos2::new(image.max.x + 30.0, image.center().y);
    frame(&ctx, &mut state, &device, click_events(letterbox, false));
    assert_eq!(
        buttons(&sent.drain()),
        vec![("left".to_owned(), false, Some((80, 60)))],
        "the release is at the last point of the picture, never at an edge"
    );
}

/// One sent wheel event: its steps on both axes and the point it was sent at.
type SentWheel = (i32, i32, Option<(u32, u32)>);

fn wheel_events(events: &[InputEvent]) -> Vec<SentWheel> {
    events
        .iter()
        .filter_map(|event| match event {
            InputEvent::Wheel { dx, dy, at } => Some((*dx, *dy, at.map(|at| (at.x, at.y)))),
            _ => None,
        })
        .collect()
}

fn notch(unit: egui::MouseWheelUnit, delta: egui::Vec2) -> egui::Event {
    egui::Event::MouseWheel {
        source: egui::MouseWheelSource::Unknown,
        unit,
        delta,
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    }
}

/// A viewer with Interact on and the pointer in the middle of the picture.
fn viewer_with_pointer_in_the_picture() -> (
    egui::Context,
    DevicePanelState,
    DeviceUiState,
    horizon_frame_source::SentFrameInput,
) {
    let (ctx, device, mut state, mut sent) = interacting_viewer();
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let middle = image_rect(&output).center();
    frame(&ctx, &mut state, &device, vec![egui::Event::PointerMoved(middle)]);
    sent.drain();
    (ctx, device, state, sent)
}

#[test]
fn one_notch_of_the_wheel_is_one_step_of_the_contract() {
    let (ctx, device, mut state, mut sent) = viewer_with_pointer_in_the_picture();
    frame(
        &ctx,
        &mut state,
        &device,
        vec![notch(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0))],
    );
    assert_eq!(
        wheel_events(&sent.drain()),
        vec![(0, 1, Some((80, 60)))],
        "one notch down is one step at the pointer, and one event"
    );
}

#[test]
fn a_smooth_scroll_of_two_half_notches_is_one_step() {
    let (ctx, device, mut state, mut sent) = viewer_with_pointer_in_the_picture();
    let half = egui::vec2(0.0, -horizon_vnc::SCROLL_NOTCH_POINTS / 2.0);
    frame(
        &ctx,
        &mut state,
        &device,
        vec![notch(egui::MouseWheelUnit::Point, half)],
    );
    assert!(wheel_events(&sent.drain()).is_empty(), "half a notch is no step yet");
    frame(
        &ctx,
        &mut state,
        &device,
        vec![notch(egui::MouseWheelUnit::Point, half)],
    );
    assert_eq!(wheel_events(&sent.drain()), vec![(0, 1, Some((80, 60)))]);

    frame(
        &ctx,
        &mut state,
        &device,
        vec![notch(egui::MouseWheelUnit::Line, egui::vec2(0.0, 1.0))],
    );
    assert_eq!(
        wheel_events(&sent.drain()),
        vec![(0, -1, Some((80, 60)))],
        "up is negative"
    );
}

#[test]
fn a_scroll_sideways_is_its_own_step_and_a_wheel_beside_the_picture_is_ignored() {
    let (ctx, device, mut state, mut sent) = viewer_with_pointer_in_the_picture();
    frame(
        &ctx,
        &mut state,
        &device,
        vec![notch(egui::MouseWheelUnit::Line, egui::vec2(-1.0, 0.0))],
    );
    assert_eq!(
        wheel_events(&sent.drain()),
        vec![(1, 0, Some((80, 60)))],
        "a wheel to the left is a positive dx"
    );

    let output = frame(&ctx, &mut state, &device, Vec::new());
    let image = image_rect(&output);
    let letterbox = egui::Pos2::new(image.max.x + 30.0, image.center().y);
    frame(&ctx, &mut state, &device, vec![egui::Event::PointerMoved(letterbox)]);
    sent.drain();
    frame(
        &ctx,
        &mut state,
        &device,
        vec![notch(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0))],
    );
    assert!(
        wheel_events(&sent.drain()).is_empty(),
        "a wheel beside the picture belongs to the canvas"
    );
}
