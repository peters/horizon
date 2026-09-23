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
