//! Interact sends the viewer's pointer and keys to the desktop, and only then.
use super::*;
use vnc::X11Event;

const SCREEN: egui::Vec2 = egui::vec2(1200.0, 800.0);

fn viewer_with_input() -> (
    egui::Context,
    DevicePanelState,
    DeviceUiState,
    tokio::sync::mpsc::UnboundedReceiver<X11Event>,
) {
    let ctx = egui::Context::default();
    ctx.all_styles_mut(|style| style.animation_time = 0.0);
    let desktop = ColorImage::filled([160, 80], egui::Color32::GREEN);
    let (session, receiver) = Session::pending_frame_with_input(desktop.clone(), desktop, DeviceViewOptions::default());
    let state = DeviceUiState {
        initialized: true,
        status: Status::Connected,
        session: Some(session),
        ..Default::default()
    };
    (ctx, fixture_device(), state, receiver)
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
            |ui| state.show(ui, device, true),
        )
        .discard_textures();
    state.finish_frame();
    output
}

/// The rendered desktop: the largest texture-filled rectangle.
fn image_rect(output: &egui::FullOutput) -> egui::Rect {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect)
                if rect
                    .brush
                    .as_ref()
                    .is_some_and(|brush| brush.fill_texture_id != egui::TextureId::default()) =>
            {
                Some(rect.rect)
            }
            egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                Some(shape.shape.visual_bounding_rect())
            }
            _ => None,
        })
        .max_by(|a, b| a.area().total_cmp(&b.area()))
        .expect("desktop image")
}

fn drain(receiver: &mut tokio::sync::mpsc::UnboundedReceiver<X11Event>) -> Vec<X11Event> {
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        events.push(event);
    }
    events
}

fn keys(events: &[X11Event]) -> Vec<(u32, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            X11Event::KeyEvent(key) => Some((key.keycode, key.down)),
            _ => None,
        })
        .collect()
}

fn pointers(events: &[X11Event]) -> Vec<(u16, u16, u8)> {
    events
        .iter()
        .filter_map(|event| match event {
            X11Event::PointerEvent(mouse) => Some((mouse.position_x, mouse.position_y, mouse.bottons)),
            _ => None,
        })
        .collect()
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

#[test]
fn read_only_by_default_sends_nothing_and_interact_forwards_pointer_and_keys() {
    let (ctx, device, mut state, mut receiver) = viewer_with_input();
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let image = image_rect(&output);
    let middle = image.center();
    frame(&ctx, &mut state, &device, click_events(middle, true));
    frame(&ctx, &mut state, &device, click_events(middle, false));
    frame(&ctx, &mut state, &device, vec![egui::Event::Text("a".into())]);
    assert!(drain(&mut receiver).is_empty(), "read-only viewers never send input");
    assert!(!state.captured);

    click_label(&ctx, &mut state, &device, "Interact");
    assert!(state.interact);
    let output = frame(&ctx, &mut state, &device, vec![egui::Event::PointerMoved(middle)]);
    let image = image_rect(&output);
    let sent = drain(&mut receiver);
    assert_eq!(
        pointers(&sent),
        vec![(80, 40, 0)],
        "the image centre is the desktop centre"
    );
    assert!(keys(&sent).is_empty());

    let corner = image.min + egui::vec2(1.0, 1.0);
    frame(&ctx, &mut state, &device, click_events(corner, true));
    let pressed = drain(&mut receiver);
    assert_eq!(
        pointers(&pressed).last(),
        Some(&(0, 0, 1)),
        "press at the top-left desktop pixel"
    );
    frame(&ctx, &mut state, &device, click_events(corner, false));
    assert_eq!(pointers(&drain(&mut receiver)).last(), Some(&(0, 0, 0)));
    assert!(state.captured, "the click focused the image");

    frame(
        &ctx,
        &mut state,
        &device,
        vec![
            key_event(egui::Key::A, true, egui::Modifiers::NONE),
            egui::Event::Text("a".into()),
            key_event(egui::Key::A, false, egui::Modifiers::NONE),
        ],
    );
    assert_eq!(
        keys(&drain(&mut receiver)),
        vec![(u32::from('a'), true), (u32::from('a'), false)],
        "a printable key is sent once, from its text"
    );
    frame(
        &ctx,
        &mut state,
        &device,
        vec![key_event(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    assert_eq!(keys(&drain(&mut receiver)), vec![(0xff0d, true)]);

    // Turning Interact off releases what the desktop still thinks is held.
    click_label(&ctx, &mut state, &device, "Interact");
    assert!(!state.interact);
    let released = keys(&drain(&mut receiver));
    assert!(released.contains(&(0xff0d, false)), "{released:?}");
    assert!(!state.captured);
    frame(&ctx, &mut state, &device, vec![egui::Event::Text("b".into())]);
    assert!(drain(&mut receiver).is_empty());
}

#[test]
fn modifiers_and_wheel_reach_the_desktop_and_escape_drops_capture() {
    let (ctx, device, mut state, mut receiver) = viewer_with_input();
    click_label(&ctx, &mut state, &device, "Interact");
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let middle = image_rect(&output).center();
    frame(&ctx, &mut state, &device, click_events(middle, true));
    frame(&ctx, &mut state, &device, click_events(middle, false));
    drain(&mut receiver);

    frame(
        &ctx,
        &mut state,
        &device,
        vec![
            egui::Event::PointerMoved(middle),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -50.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ],
    );
    let wheel = pointers(&drain(&mut receiver));
    assert!(wheel.contains(&(80, 40, 16)), "scroll down is button 5: {wheel:?}");
    assert_eq!(wheel.last(), Some(&(80, 40, 0)));

    frame(
        &ctx,
        &mut state,
        &device,
        vec![key_event(egui::Key::C, true, egui::Modifiers::CTRL)],
    );
    let chord = keys(&drain(&mut receiver));
    assert_eq!(
        &chord[..2],
        [(0xffe3, true), (u32::from('c'), true)],
        "Control_L goes down before c: {chord:?}"
    );

    frame(
        &ctx,
        &mut state,
        &device,
        vec![
            key_event(egui::Key::C, false, egui::Modifiers::NONE),
            key_event(egui::Key::Escape, true, egui::Modifiers::NONE),
        ],
    );
    let after = keys(&drain(&mut receiver));
    // The test frames carry no modifier state, so Control_L goes up at the end
    // of the chord's own frame; either way it must be released before `c` is.
    let control_up = chord
        .iter()
        .chain(&after)
        .position(|event| *event == (0xffe3, false))
        .expect("Control_L released");
    let c_up = chord
        .iter()
        .chain(&after)
        .position(|event| *event == (u32::from('c'), false))
        .expect("c released");
    assert!(control_up < c_up, "{chord:?} then {after:?}");
    assert!(
        after.contains(&(u32::from('c'), false)),
        "c released after Ctrl: {after:?}"
    );
    assert!(after.contains(&(0xff1b, true)), "Escape goes to the desktop: {after:?}");
    assert!(state.captured, "Escape does not give the keyboard back");

    // Clicking outside the image does, and releases whatever is still down.
    frame(
        &ctx,
        &mut state,
        &device,
        vec![key_event(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    drain(&mut receiver);
    let outside = egui::Pos2::new(SCREEN.x - 4.0, SCREEN.y - 4.0);
    frame(&ctx, &mut state, &device, click_events(outside, true));
    frame(&ctx, &mut state, &device, click_events(outside, false));
    let released = keys(&drain(&mut receiver));
    assert!(
        released.contains(&(0xff0d, false)),
        "Enter released on focus loss: {released:?}"
    );
    assert!(!state.captured);
    frame(&ctx, &mut state, &device, vec![egui::Event::Text("z".into())]);
    assert!(drain(&mut receiver).is_empty(), "no capture, no keys");
}

#[test]
fn pointer_positions_are_mapped_through_the_panel_layer_transform() {
    let (ctx, device, mut state, mut receiver) = viewer_with_input();
    click_label(&ctx, &mut state, &device, "Interact");
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let image = image_rect(&output);
    // The canvas draws this layer scaled by half and shifted, as a zoomed-out
    // workspace does; global pointer positions must be mapped back.
    let transform = egui::emath::TSTransform::new(egui::vec2(300.0, 200.0), 0.5);
    let global_center = transform * image.center();
    let layer = egui::LayerId::background();
    ctx.set_transform_layer(layer, transform);
    frame(
        &ctx,
        &mut state,
        &device,
        vec![egui::Event::PointerMoved(global_center)],
    );
    ctx.set_transform_layer(layer, transform);
    frame(&ctx, &mut state, &device, click_events(global_center, true));
    ctx.set_transform_layer(layer, transform);
    frame(&ctx, &mut state, &device, click_events(global_center, false));
    let sent = pointers(&drain(&mut receiver));
    assert!(
        sent.contains(&(80, 40, 1)) && sent.last() == Some(&(80, 40, 0)),
        "the desktop centre is pressed and released through the transform: {sent:?}"
    );
}

#[test]
fn a_drag_that_leaves_the_window_or_a_hidden_viewer_releases_what_is_held() {
    let (ctx, device, mut state, mut receiver) = viewer_with_input();
    click_label(&ctx, &mut state, &device, "Interact");
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let middle = image_rect(&output).center();
    frame(&ctx, &mut state, &device, click_events(middle, true));
    drain(&mut receiver);
    frame(&ctx, &mut state, &device, vec![egui::Event::PointerGone]);
    assert_eq!(
        pointers(&drain(&mut receiver)),
        vec![(80, 40, 0)],
        "the button goes up where the desktop last saw the pointer"
    );

    frame(&ctx, &mut state, &device, click_events(middle, true));
    frame(&ctx, &mut state, &device, click_events(middle, false));
    frame(
        &ctx,
        &mut state,
        &device,
        vec![key_event(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    drain(&mut receiver);
    // A frame in which the panel is not drawn at all.
    state.begin_frame();
    state.finish_frame();
    assert!(keys(&drain(&mut receiver)).contains(&(0xff0d, false)));
    assert!(!state.captured);
}

#[test]
fn a_click_on_ui_covering_the_image_does_not_reach_the_desktop() {
    let (ctx, device, mut state, mut receiver) = viewer_with_input();
    click_label(&ctx, &mut state, &device, "Interact");
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let middle = image_rect(&output).center();
    // A foreground area (a popup, modal or another panel) over the image.
    let covered = |state: &mut DeviceUiState, events: Vec<egui::Event>| {
        state.begin_frame();
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
                    ..Default::default()
                },
                |ui| {
                    state.show(ui, &device, true);
                    egui::Area::new(egui::Id::new("cover"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(middle - egui::vec2(40.0, 40.0))
                        .show(ui.ctx(), |ui| {
                            ui.allocate_response(egui::vec2(80.0, 80.0), egui::Sense::click());
                        });
                },
            )
            .discard_textures();
        state.finish_frame();
    };
    // egui lays a new area out invisibly first; hit testing sees it after.
    for events in [
        Vec::new(),
        Vec::new(),
        vec![egui::Event::PointerMoved(middle)],
        click_events(middle, true),
        click_events(middle, false),
    ] {
        covered(&mut state, events);
    }
    let sent = pointers(&drain(&mut receiver));
    assert!(
        sent.iter().all(|(_, _, buttons)| *buttons == 0),
        "no press reached the desktop through the covering UI: {sent:?}"
    );
    assert!(!state.captured, "the covered click did not capture the keyboard");
}

#[test]
fn a_discarded_pass_does_not_send_its_input_twice() {
    let (ctx, device, mut state, mut receiver) = viewer_with_input();
    click_label(&ctx, &mut state, &device, "Interact");
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let middle = image_rect(&output).center();
    frame(&ctx, &mut state, &device, click_events(middle, true));
    frame(&ctx, &mut state, &device, click_events(middle, false));
    drain(&mut receiver);
    // egui re-runs a discarded pass with the same input events.
    let mut passes = 0;
    state.begin_frame();
    let _ = ctx
        .run_ui(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(middle),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::vec2(0.0, -1.0),
                        modifiers: egui::Modifiers::NONE,
                        phase: egui::TouchPhase::Move,
                    },
                    egui::Event::Text("a".into()),
                ],
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
                ..Default::default()
            },
            |ui| {
                passes += 1;
                state.show(ui, &device, true);
                if ui.ctx().current_pass_index() == 0 {
                    ui.ctx().request_discard("test");
                }
            },
        )
        .discard_textures();
    state.finish_frame();
    assert_eq!(passes, 2, "the frame ran a second pass");
    let sent = drain(&mut receiver);
    assert_eq!(
        pointers(&sent).iter().filter(|(_, _, buttons)| *buttons == 16).count(),
        1,
        "one wheel line is one notch: {sent:?}"
    );
    assert_eq!(keys(&sent), vec![(u32::from('a'), true), (u32::from('a'), false)]);
}

#[test]
fn a_fast_drag_that_ends_outside_the_image_keeps_its_press_and_release() {
    let (ctx, device, mut state, mut receiver) = viewer_with_input();
    click_label(&ctx, &mut state, &device, "Interact");
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let start = image_rect(&output).center();
    let outside = egui::Pos2::new(SCREEN.x - 4.0, SCREEN.y - 4.0);
    frame(&ctx, &mut state, &device, vec![egui::Event::PointerMoved(start)]);
    drain(&mut receiver);
    // Press on the image, drag off it and release, all before one repaint.
    frame(
        &ctx,
        &mut state,
        &device,
        vec![
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerMoved(outside),
            egui::Event::PointerButton {
                pos: outside,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    let sent = pointers(&drain(&mut receiver));
    assert!(sent.contains(&(80, 40, 1)), "the press reached the desktop: {sent:?}");
    assert_eq!(
        sent.last().map(|(_, _, buttons)| *buttons),
        Some(0),
        "and so did its release: {sent:?}"
    );
}

fn frame_with_modifiers(
    ctx: &egui::Context,
    state: &mut DeviceUiState,
    device: &DevicePanelState,
    mut events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
) -> egui::FullOutput {
    events.insert(0, egui::Event::ModifiersChanged(modifiers));
    state.begin_frame();
    let output = ctx
        .run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
                ..Default::default()
            },
            |ui| state.show(ui, device, true),
        )
        .discard_textures();
    state.finish_frame();
    output
}

#[test]
fn clipboard_chords_and_ime_commits_reach_the_desktop_without_the_local_clipboard() {
    // The platform command modifier: Ctrl (Control_L) except on macOS, where
    // egui-winit reports Command and the viewer sends Super_L.
    let (command, command_key, command_press) = if cfg!(target_os = "macos") {
        (
            egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND,
            0xffeb,
            egui::Key::SuperLeft,
        )
    } else {
        (egui::Modifiers::CTRL, 0xffe3, egui::Key::ControlLeft)
    };
    let (ctx, device, mut state, mut receiver) = viewer_with_input();
    click_label(&ctx, &mut state, &device, "Interact");
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let middle = image_rect(&output).center();
    frame(&ctx, &mut state, &device, click_events(middle, true));
    let output = frame(&ctx, &mut state, &device, click_events(middle, false));
    assert!(
        output.platform_output.ime.is_some(),
        "the input method is on while captured"
    );
    drain(&mut receiver);

    // What egui-winit emits for a quick Ctrl+C inside one repaint: Copy, the
    // key release, and Ctrl already up by the end of the frame.
    frame_with_modifiers(
        &ctx,
        &mut state,
        &device,
        vec![egui::Event::Copy, key_event(egui::Key::C, false, command)],
        egui::Modifiers::NONE,
    );
    let c = u32::from('c');
    assert_eq!(
        keys(&drain(&mut receiver)),
        vec![(command_key, true), (c, true), (c, false), (command_key, false)],
        "the command key is held around c"
    );
    // Ctrl+V with an empty local clipboard, as winit reports it on X11: Ctrl
    // goes down and up, then only the V release arrives.
    frame_with_modifiers(
        &ctx,
        &mut state,
        &device,
        vec![
            key_event(command_press, true, egui::Modifiers::NONE),
            key_event(command_press, false, command),
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            key_event(egui::Key::V, false, egui::Modifiers::NONE),
        ],
        egui::Modifiers::NONE,
    );
    let v = u32::from('v');
    assert_eq!(
        keys(&drain(&mut receiver)),
        vec![
            (command_key, true),
            (command_key, false),
            (command_key, true),
            (v, true),
            (v, false),
            (command_key, false)
        ],
        "the Ctrl press as it happened, then the paste chord with Ctrl held around v"
    );

    frame_with_modifiers(
        &ctx,
        &mut state,
        &device,
        vec![egui::Event::Paste("local secret".into()), egui::Event::Cut],
        command,
    );
    frame_with_modifiers(&ctx, &mut state, &device, Vec::new(), egui::Modifiers::NONE);
    let sent = keys(&drain(&mut receiver));
    let letters: Vec<u32> = sent
        .iter()
        .filter(|(_, down)| *down)
        .map(|(keysym, _)| *keysym)
        .collect();
    assert_eq!(letters, vec![command_key, u32::from('v'), u32::from('x')], "{sent:?}");
    assert!(
        !sent.iter().any(|(keysym, _)| *keysym == u32::from('l')),
        "the local clipboard is not typed"
    );

    frame(
        &ctx,
        &mut state,
        &device,
        vec![egui::Event::Ime(egui::ImeEvent::Commit("日本".into()))],
    );
    assert_eq!(
        keys(&drain(&mut receiver)),
        vec![
            (0x0100_65e5, true),
            (0x0100_65e5, false),
            (0x0100_672c, true),
            (0x0100_672c, false)
        ],
        "a composed commit is sent as Unicode keysyms"
    );
}

#[test]
fn wheel_events_go_to_the_image_under_the_pointer_when_they_happened() {
    let (ctx, device, mut state, mut receiver) = viewer_with_input();
    click_label(&ctx, &mut state, &device, "Interact");
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let middle = image_rect(&output).center();
    let outside = egui::Pos2::new(SCREEN.x - 4.0, SCREEN.y - 4.0);
    let wheel = egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, -1.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    };
    frame(&ctx, &mut state, &device, vec![egui::Event::PointerMoved(middle)]);
    drain(&mut receiver);
    let notches = |events: Vec<X11Event>| {
        pointers(&events)
            .iter()
            .filter(|(_, _, buttons)| *buttons == 16)
            .count()
    };

    // Scrolled over the image, then left it before the repaint: still ours.
    frame(
        &ctx,
        &mut state,
        &device,
        vec![wheel.clone(), egui::Event::PointerMoved(outside)],
    );
    assert_eq!(notches(drain(&mut receiver)), 1);
    // Scrolled elsewhere, then entered the image before the repaint: not ours.
    frame(
        &ctx,
        &mut state,
        &device,
        vec![wheel, egui::Event::PointerMoved(middle)],
    );
    assert_eq!(notches(drain(&mut receiver)), 0);
}

#[test]
fn window_focus_loss_releases_and_the_first_wheel_uses_the_current_pointer() {
    let (ctx, device, mut state, mut receiver) = viewer_with_input();
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let middle = image_rect(&output).center();
    // The pointer is already over the image when Interact is turned on.
    frame(&ctx, &mut state, &device, vec![egui::Event::PointerMoved(middle)]);
    click_label(&ctx, &mut state, &device, "Interact");
    frame(&ctx, &mut state, &device, click_events(middle, true));
    frame(&ctx, &mut state, &device, click_events(middle, false));
    drain(&mut receiver);
    frame(&ctx, &mut state, &device, vec![egui::Event::PointerMoved(middle)]);
    drain(&mut receiver);
    frame(
        &ctx,
        &mut state,
        &device,
        vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, -1.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        }],
    );
    assert!(
        pointers(&drain(&mut receiver))
            .iter()
            .any(|(_, _, buttons)| *buttons == 16),
        "a wheel with no move in its frame still scrolls"
    );

    frame(
        &ctx,
        &mut state,
        &device,
        vec![key_event(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    drain(&mut receiver);
    // Alt-Tab away: the window loses OS focus while Enter is held.
    state.begin_frame();
    let _ = ctx
        .run_ui(
            egui::RawInput {
                events: vec![egui::Event::WindowFocused(false)],
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
                viewports: std::iter::once((
                    egui::ViewportId::ROOT,
                    egui::ViewportInfo {
                        focused: Some(false),
                        ..Default::default()
                    },
                ))
                .collect(),
                ..Default::default()
            },
            |ui| state.show(ui, &device, true),
        )
        .discard_textures();
    state.finish_frame();
    assert!(keys(&drain(&mut receiver)).contains(&(0xff0d, false)));
    assert!(!state.captured);
}

#[test]
fn a_press_on_covering_ui_stays_there_even_if_the_pointer_then_reaches_the_image() {
    let (ctx, device, mut state, mut receiver) = viewer_with_input();
    click_label(&ctx, &mut state, &device, "Interact");
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let image = image_rect(&output);
    let covered_spot = image.center();
    let open_spot = image.min + egui::vec2(10.0, 10.0);
    let covered = |state: &mut DeviceUiState, events: Vec<egui::Event>| {
        state.begin_frame();
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
                    ..Default::default()
                },
                |ui| {
                    state.show(ui, &device, true);
                    egui::Area::new(egui::Id::new("popup"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(covered_spot - egui::vec2(20.0, 20.0))
                        .show(ui.ctx(), |ui| {
                            ui.allocate_response(egui::vec2(40.0, 40.0), egui::Sense::click());
                        });
                },
            )
            .discard_textures();
        state.finish_frame();
    };
    covered(&mut state, Vec::new());
    covered(&mut state, Vec::new());
    drain(&mut receiver);
    // Press on the popup, then move onto the open image before the repaint.
    covered(
        &mut state,
        vec![
            egui::Event::PointerMoved(covered_spot),
            egui::Event::PointerButton {
                pos: covered_spot,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerMoved(open_spot),
        ],
    );
    let sent = pointers(&drain(&mut receiver));
    assert!(
        sent.iter().all(|(_, _, buttons)| *buttons == 0),
        "the popup's press is not replayed to the desktop: {sent:?}"
    );
}

#[test]
fn a_wheel_without_a_move_is_sent_at_the_current_pointer() {
    let (ctx, device, mut state, mut receiver) = viewer_with_input();
    click_label(&ctx, &mut state, &device, "Interact");
    let output = frame(&ctx, &mut state, &device, Vec::new());
    let image = image_rect(&output);
    frame(
        &ctx,
        &mut state,
        &device,
        vec![egui::Event::PointerMoved(image.center())],
    );
    drain(&mut receiver);
    // The desktop last saw the centre; the pointer is now near the top-left.
    frame(
        &ctx,
        &mut state,
        &device,
        vec![egui::Event::PointerMoved(image.min + egui::vec2(1.0, 1.0))],
    );
    state.input = InputState::default();
    drain(&mut receiver);
    frame(
        &ctx,
        &mut state,
        &device,
        vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        }],
    );
    let sent = pointers(&drain(&mut receiver));
    assert!(
        sent.contains(&(0, 0, 8)),
        "the notch is at the pointer, not dropped: {sent:?}"
    );
}
