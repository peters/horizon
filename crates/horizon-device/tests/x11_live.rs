#![cfg(target_os = "linux")]
use horizon_device::{ActRequest, Action, Device, DeviceError, Key, Modifier, Point, Target};
use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};

// Never defaults to DISPLAY. Run explicitly against a task-owned virtual desktop.
#[test]
#[ignore = "requires HORIZON_DEVICE_TEST_TARGET pointing to an owned virtual desktop"]
fn input_is_bounded_and_releases_buttons_and_modifiers() -> Result<(), Box<dyn std::error::Error>> {
    let config = std::env::var("HORIZON_DEVICE_TEST_TARGET")?;
    let target: Target = serde_json::from_slice(&std::fs::read(config)?)?;
    let horizon_device::Endpoint::LocalX11 { display } = &target.endpoint else {
        return Err("requires local X11".into());
    };
    let (connection, screen) = x11rb::connect(Some(display))?;
    let root = connection.setup().roots[screen].root;
    let mut device = Device::connect(&target)?;
    let geometry = device.screenshot()?.geometry;
    let before = connection.query_pointer(root)?.reply()?;
    let mut request = ActRequest {
        geometry,
        action: Action::Drag {
            from: Point { x: 1100, y: 180 },
            to: Point { x: 1150, y: 220 },
            duration_ms: 200,
        },
    };
    request.geometry.revision.push_str("stale");
    assert!(matches!(device.act(&request), Err(DeviceError::StaleGeometry)));
    let after = connection.query_pointer(root)?.reply()?;
    assert_eq!(
        (before.root_x, before.root_y, before.mask),
        (after.root_x, after.root_y, after.mask)
    );
    request.geometry = device.screenshot()?.geometry;
    device.act(&request)?;
    let after = connection.query_pointer(root)?.reply()?;
    assert_eq!((after.root_x, after.root_y), (1150, 220));
    assert_eq!(u16::from(after.mask) & 0x1f00, 0, "mouse buttons released");
    request.action = Action::Key {
        key: Key::Escape,
        modifiers: vec![Modifier::Control, Modifier::Shift],
    };
    device.act(&request)?;
    assert_eq!(u16::from(connection.query_pointer(root)?.reply()?.mask) & 5, 0);
    request.action = Action::Scroll {
        at: Point { x: 1130, y: 200 },
        vertical_notches: 1,
        horizontal_notches: -1,
    };
    device.act(&request)?;
    assert_eq!(u16::from(connection.query_pointer(root)?.reply()?.mask) & 0x1f00, 0);
    delayed_text_receiver_preserves_unicode(&target)?;
    Ok(())
}

fn delayed_text_receiver_preserves_unicode(target: &Target) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};
    use x11rb::protocol::{
        Event,
        xproto::{CreateWindowAux, EventMask, InputFocus, WindowClass},
    };

    let horizon_device::Endpoint::LocalX11 { display } = &target.endpoint else {
        return Err("requires local X11".into());
    };
    let (connection, screen) = x11rb::connect(Some(display))?;
    let root = connection.setup().roots[screen].root;
    let window = connection.generate_id()?;
    connection
        .create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            100,
            100,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .override_redirect(1)
                .event_mask(EventMask::KEY_PRESS),
        )?
        .check()?;
    connection.map_window(window)?.check()?;
    connection
        .set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)?
        .check()?;
    let setup = connection.setup();
    let mapping = connection
        .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)?
        .reply()?;
    let capacity = (setup.min_keycode..=setup.max_keycode)
        .zip(mapping.keysyms.chunks(usize::from(mapping.keysyms_per_keycode)))
        .filter(|(keycode, symbols)| *keycode != 8 && symbols.iter().all(|symbol| *symbol == 0))
        .count();
    assert!(capacity > 0 && capacity < 256);
    let expected: String = (0..u32::try_from(capacity)?)
        .filter_map(|index| char::from_u32(0xe000 + index))
        .collect();
    let mut device = Device::connect(target)?;
    let geometry = device.screenshot()?.geometry;
    assert!(matches!(
        device.act(&ActRequest {
            geometry,
            action: Action::Type {
                text: format!("{expected}\u{f000}")
            },
        }),
        Err(DeviceError::Invalid(_))
    ));
    while let Some(event) = connection.poll_for_event()? {
        assert!(!matches!(event, Event::KeyPress(_)), "rejected request sent input");
    }
    let sent = expected.clone();
    let target = target.clone();
    let sender = std::thread::spawn(move || -> horizon_device::Result<()> {
        let mut device = Device::connect(&target)?;
        device.act(&ActRequest {
            geometry: device.screenshot()?.geometry,
            action: Action::Type { text: sent },
        })?;
        Ok(())
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut received = String::new();
    while Instant::now() < deadline {
        // Model a GUI consuming key events on a later frame. Looking up the
        // current mapping exposes premature cleanup after the last character.
        std::thread::sleep(Duration::from_millis(30));
        while let Some(event) = connection.poll_for_event()? {
            if let Event::KeyPress(event) = event {
                let mapping = connection.get_keyboard_mapping(event.detail, 1)?.reply()?;
                let symbol = mapping.keysyms.first().copied().unwrap_or_default();
                let codepoint = if symbol & 0xff00_0000 == 0x0100_0000 {
                    symbol & 0x00ff_ffff
                } else {
                    symbol
                };
                received.push(char::from_u32(codepoint).unwrap_or('\u{fffd}'));
            }
        }
        if sender.is_finished() {
            break;
        }
    }
    sender.join().map_err(|_| "text sender panicked")??;
    assert_eq!(
        received, expected,
        "temporary key mappings survived delayed consumption"
    );
    Ok(())
}
