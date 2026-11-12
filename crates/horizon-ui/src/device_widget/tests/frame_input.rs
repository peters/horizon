//! Interact sends a person's keys and text to a frame source program, and only
//! while the picture has the keyboard.
use super::*;
use horizon_frame_source::{InputEvent, InputHello};

const SCREEN: egui::Vec2 = egui::vec2(1200.0, 800.0);

pub(super) fn frame_device() -> DevicePanelState {
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
    frame_viewer_of([160, 120])
}

/// A frame source viewer that holds one picture of `picture` pixels.
fn frame_viewer_of(
    picture: [usize; 2],
) -> (
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
    // A picture of 4:3 leaves a letterbox beside it in a panel of another shape.
    state.set_source(&ctx, ColorImage::filled(picture, egui::Color32::GREEN));
    (ctx, frame_device(), state, sent)
}

fn frame(
    ctx: &egui::Context,
    state: &mut DeviceUiState,
    device: &DevicePanelState,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    frame_in(ctx, state, device, SCREEN, events)
}

/// One frame in a body of `screen` points.
fn frame_in(
    ctx: &egui::Context,
    state: &mut DeviceUiState,
    device: &DevicePanelState,
    screen: egui::Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    state.begin_frame();
    let output = ctx
        .run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
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
fn one_notch_stays_one_step_over_the_frames_of_the_smoothing() {
    // The context of a frame source viewer of the window: the options that a start of
    // Horizon sets, so a test reads the same smoothing as a person.
    let (ctx, device, mut state, mut sent) = viewer_with_pointer_in_the_picture();
    crate::app::bootstrap::pin_chrome_to_native_display_scale(&ctx);
    frame(
        &ctx,
        &mut state,
        &device,
        vec![notch(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0))],
    );
    // egui hands the travel of a notch out over a few frames of the smoothing, and
    // none of those frames may send the step again.
    for _ in 0..6 {
        frame(&ctx, &mut state, &device, Vec::new());
    }
    assert_eq!(
        wheel_events(&sent.drain()),
        vec![(0, 1, Some((80, 60)))],
        "one notch is one step in all of the frames that the smoothing takes"
    );
}

#[test]
fn the_press_and_the_release_of_one_wheel_button_are_one_step() {
    // winit's X11 backend sends one wheel event for the press of a wheel button and
    // one for its release, with the same delta, so one notch of a wheel without
    // scroll valuators - and of the XTEST click of the fixture - arrives twice.
    let (ctx, device, mut state, mut sent) = viewer_with_pointer_in_the_picture();
    let one = notch(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0));
    frame(&ctx, &mut state, &device, vec![one.clone(), one]);
    assert_eq!(
        wheel_events(&sent.drain()),
        vec![(0, 1, Some((80, 60)))],
        "the press and the release of one notch are one step"
    );
    frame(&ctx, &mut state, &device, Vec::new());
    assert!(
        wheel_events(&sent.drain()).is_empty(),
        "and no frame after it sends it again"
    );
}

#[test]
fn a_notch_of_each_frame_is_its_own_step() {
    let (ctx, device, mut state, mut sent) = viewer_with_pointer_in_the_picture();
    for _ in 0..2 {
        frame(
            &ctx,
            &mut state,
            &device,
            vec![notch(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0))],
        );
    }
    assert_eq!(
        wheel_events(&sent.drain()),
        vec![(0, 1, Some((80, 60))), (0, 1, Some((80, 60)))],
        "one notch a frame is one step a frame"
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

#[test]
fn the_picture_of_a_new_viewer_is_drawn_as_wide_as_its_panel() {
    let (ctx, device, mut state, _sent) = frame_viewer_of([640, 360]);
    // The style of the window, which sets the heights of the rows of the chrome.
    crate::theme::apply(&ctx, horizon_core::AppearanceTheme::Dark);
    // The default size of a frame source panel less its titlebar and its padding:
    // the body that this widget draws in.
    let body = egui::vec2(
        horizon_core::FRAME_SOURCE_PANEL_SIZE[0] - 2.0 * crate::app::PANEL_PADDING,
        horizon_core::FRAME_SOURCE_PANEL_SIZE[1] - crate::app::PANEL_TITLEBAR_HEIGHT - 2.0 * crate::app::PANEL_PADDING,
    );
    let output = frame_in(&ctx, &mut state, &device, body, Vec::new());
    let image = image_rect(&output);
    assert!(
        image.width() >= body.x - 0.5,
        "the first picture of the program gets the full width of the body of its \
         panel, and the chrome above it ({:?} of {:?}) takes no more than \
         FRAME_SOURCE_VIEWER_CHROME_HEIGHT allows: {image:?}",
        body.y - image.height(),
        body
    );
    let shape = image.width() / image.height();
    assert!(
        (shape - 640.0 / 360.0).abs() < 0.02,
        "and it keeps the shape of its own stream message: {shape}"
    );
}

#[test]
fn the_mode_line_names_the_program_of_a_frame_source_and_the_desktop_of_a_vnc_viewer() {
    let painted = |output: &egui::FullOutput| -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    };

    let (ctx, device, mut state, _sent) = frame_viewer();
    click_label(&ctx, &mut state, &device, "Interact");
    assert!(state.interact);
    let texts = painted(&frame(&ctx, &mut state, &device, Vec::new()));
    assert!(
        texts
            .iter()
            .any(|text| text == "Interactive: your mouse and keyboard go to the program"),
        "the input of a frame source viewer goes to its program: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text.contains("go to the desktop")),
        "a frame source viewer shows no desktop: {texts:?}"
    );

    let desktop = ColorImage::filled([160, 80], egui::Color32::GREEN);
    let (session, _input) = Session::pending_frame_with_input(desktop.clone(), desktop, ViewOptions::default());
    let mut vnc = DeviceUiState {
        initialized: true,
        status: Status::Connected,
        session: Some(session),
        ..DeviceUiState::default()
    };
    let vnc_device = fixture_device();
    let vnc_ctx = egui::Context::default();
    vnc_ctx.all_styles_mut(|style| style.animation_time = 0.0);
    click_label(&vnc_ctx, &mut vnc, &vnc_device, "Interact");
    assert!(vnc.interact);
    let texts = painted(&frame(&vnc_ctx, &mut vnc, &vnc_device, Vec::new()));
    assert!(
        texts
            .iter()
            .any(|text| text == "Interactive: your mouse and keyboard go to the desktop"),
        "the sentence of a VNC viewer stays: {texts:?}"
    );
}

#[test]
fn the_chrome_of_a_frame_source_viewer_names_the_program_and_its_source() {
    let (ctx, device, mut state, _sent) = frame_viewer();
    // What the program said of itself in its `hello`.
    state.server.name = Some("frame_source_standin".to_owned());
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let texts: Vec<String> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect();
    assert!(
        texts.iter().any(|text| text == "Frame source program"),
        "the viewer says what kind of source it shows: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text == "frame source standin"),
        "and the name of the program of the settings: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text == "frame_source_standin"),
        "the text that the program gives itself is no name of the viewer: {texts:?}"
    );
}

#[test]
fn the_counters_of_a_hidden_program_reach_an_inspection() {
    let (ctx, device, mut state, _sent) = frame_viewer();
    // What the last `stats` message of the program said: the age of the picture
    // that it sent last, and the pictures that it dropped.
    state.stats = Some(horizon_frame_source::Stats {
        frames_sent: 5,
        frames_dropped: 2,
        bytes: 900,
        width: 160,
        height: 120,
        age_ms: 1234,
    });
    // A frame that does not draw the viewer keeps the counters of the program.
    state.begin_frame();
    state.finish_frame(&ctx);
    let observation = state.observation("panel".into(), &device, false, "agent");
    assert_eq!(observation.age_ms, Some(1234), "the age of the program is its own");
    assert_eq!(observation.frames_dropped, Some(2));
    assert_eq!(
        observation.diagnostics.map(|diagnostics| diagnostics.presentation),
        Some(horizon_core::browser::manifest::device::Presentation::Hidden)
    );
}

#[test]
fn a_new_viewer_opens_with_its_details_and_its_controls_closed() {
    let (ctx, device, mut state, _sent) = frame_viewer();
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let texts: Vec<String> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect();
    for header in ["Connection details", "View controls"] {
        assert!(texts.iter().any(|text| text == header), "the viewer draws {header}");
    }
    for inside in ["Dropped pictures", "Maximum fps"] {
        assert!(
            !texts.iter().any(|text| text == inside),
            "the picture keeps its room: {inside} is inside a closed section: {texts:?}"
        );
    }
}
