#![cfg(target_os = "linux")]
use horizon_device::{ActRequest, Action, Device, DeviceError, Key, Modifier, Point, Target};
use x11rb::{
    connection::Connection,
    protocol::xproto::{ConnectionExt, KEY_PRESS_EVENT, KEY_RELEASE_EVENT},
};

#[path = "support/x11_text.rs"]
mod x11_text;

const CAPS_LOCK: u32 = 0xffe5;
const SHIFT_L: u32 = 0xffe1;

/// Both tests type into their own focused window on the same display.
static DISPLAY: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn exclusive_display() -> std::sync::MutexGuard<'static, ()> {
    DISPLAY.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Never defaults to DISPLAY. Run explicitly against a task-owned virtual desktop.
#[test]
#[ignore = "requires HORIZON_DEVICE_TEST_TARGET pointing to an owned virtual desktop"]
fn input_is_bounded_and_releases_buttons_and_modifiers() -> Result<(), Box<dyn std::error::Error>> {
    let _serialized = exclusive_display();
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
    capture_options_keep_original_geometry(&target)?;
    Ok(())
}

/// Text longer than one action's mapping capacity is sent as several `type`
/// actions back to back. The client translates every key 600 ms late with
/// the keymap current at that time, so a keycode that changes before a late
/// client reads its key is detected as a lost or wrong character.
#[test]
#[ignore = "requires HORIZON_DEVICE_TEST_TARGET pointing to an owned virtual desktop"]
fn multi_chunk_type_actions_deliver_every_character_once() -> x11_text::TestResult<()> {
    use std::time::Duration;
    let _serialized = exclusive_display();
    let config = std::env::var("HORIZON_DEVICE_TEST_TARGET")?;
    let target: Target = serde_json::from_slice(&std::fs::read(config)?)?;
    let horizon_device::Endpoint::LocalX11 { display } = &target.endpoint else {
        return Err("requires local X11".into());
    };
    let iterations: u64 = std::env::var("HORIZON_DEVICE_TYPE_ITERATIONS")
        .ok()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(3);
    let lag = Duration::from_millis(600);
    let mut client = x11_text::Receiver::open(display)?;
    for iteration in 0..iterations {
        let text = synthetic_key(iteration, 106);
        let chunks: Vec<String> = text
            .chars()
            .collect::<Vec<_>>()
            .chunks(40)
            .map(|chunk| chunk.iter().collect())
            .collect();
        let received = type_chunks(&target, &mut client, chunks, lag)?;
        assert_eq!(received, text, "ASCII iteration {iteration}");
    }

    // Two disjoint sets that each fill the mapping capacity force every
    // temporary keycode to be reassigned between actions.
    let capacity = u32::try_from(x11_text::mapping_capacity(&client.connection)?)?;
    assert!(capacity >= 2, "the display needs unused keycodes");
    let set = |base: u32| -> String { (base..base + capacity).filter_map(char::from_u32).collect() };
    let (first, second) = (set(0xe000), set(0xe400));
    let chunks = vec![
        format!("{first}Aa"),
        format!("{second}_-"),
        format!("Zz{second}"),
        format!("{first}09"),
    ];
    let expected: String = chunks.concat();
    let received = type_chunks(&target, &mut client, chunks, lag)?;
    assert_eq!(received, expected, "reassigned temporary mappings");

    // Caps Lock inverts Shift for letter keys only. A letter that needs a
    // temporary mapping is refused, because a client can change its case.
    let both = [KEY_PRESS_EVENT, KEY_RELEASE_EVENT];
    fake_keysym(&client.connection, CAPS_LOCK, &both)?;
    let caps = vec!["AbC_9-xYz".to_owned(), "\u{e000}Qq".to_owned()];
    let received = type_chunks(&target, &mut client, caps.clone(), lag);
    let letter = Device::connect(&target).and_then(|mut device| {
        device.act(&ActRequest {
            geometry: device.screenshot()?.geometry,
            action: Action::Type { text: "æ".into() },
        })
    });
    fake_keysym(&client.connection, CAPS_LOCK, &both)?;
    assert_eq!(received?, caps.concat(), "Caps Lock");
    assert!(matches!(letter, Err(DeviceError::Unsupported(_))), "{letter:?}");
    assert_eq!(client.collect_while(lag, || true)?, "", "no key for a refused letter");

    // A held modifier would change each key, so text input refuses it before input.
    fake_keysym(&client.connection, SHIFT_L, &[KEY_PRESS_EVENT])?;
    let held = Device::connect(&target).and_then(|mut device| {
        device.act(&ActRequest {
            geometry: device.screenshot()?.geometry,
            action: Action::Type { text: "a".into() },
        })
    });
    fake_keysym(&client.connection, SHIFT_L, &[KEY_RELEASE_EVENT])?;
    assert!(matches!(held, Err(DeviceError::Unsupported(_))), "{held:?}");
    assert_eq!(client.collect_while(lag, || true)?, "", "no key while Shift is held");

    // Pointer and key actions need one unused keycode after text fills the rest,
    // also when another client takes the keycode that text input keeps.
    let mut device = Device::connect(&target)?;
    let meta = |device: &mut Device| -> horizon_device::Result<()> {
        device.act(&ActRequest {
            geometry: device.screenshot()?.geometry,
            action: Action::Key {
                key: Key::Escape,
                modifiers: vec![Modifier::Meta],
            },
        })?;
        Ok(())
    };
    device.doctor()?;
    meta(&mut device)?;
    let kept = unused_keycodes(&client.connection)?;
    assert_eq!(kept.len(), 1, "text input keeps exactly one unused keycode");
    client
        .connection
        .change_keyboard_mapping(1, kept[0], 2, &[0x0100_f8ff, 0x0100_f8ff])?
        .check()?;
    let recovered = device.doctor().map(drop).and_then(|()| meta(&mut device));
    client
        .connection
        .change_keyboard_mapping(1, kept[0], 2, &[0, 0])?
        .check()?;
    recovered?;
    Ok(())
}

fn unused_keycodes(connection: &x11rb::rust_connection::RustConnection) -> x11_text::TestResult<Vec<u8>> {
    let setup = connection.setup();
    let mapping = connection
        .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)?
        .reply()?;
    Ok((setup.min_keycode..=setup.max_keycode)
        .zip(mapping.keysyms.chunks(usize::from(mapping.keysyms_per_keycode)))
        .filter(|(keycode, symbols)| *keycode != 8 && symbols.iter().all(|symbol| *symbol == 0))
        .map(|(keycode, _)| keycode)
        .collect())
}

/// Sends XTEST events of `kinds` for the first key whose first level is `keysym`.
fn fake_keysym(
    connection: &x11rb::rust_connection::RustConnection,
    keysym: u32,
    kinds: &[u8],
) -> x11_text::TestResult<()> {
    use x11rb::protocol::xtest::ConnectionExt as _;
    let setup = connection.setup();
    let root = setup.roots[0].root;
    let mapping = connection
        .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)?
        .reply()?;
    let keycode = (setup.min_keycode..=setup.max_keycode)
        .zip(mapping.keysyms.chunks(usize::from(mapping.keysyms_per_keycode)))
        .find(|(_, symbols)| symbols.first() == Some(&keysym))
        .map(|(keycode, _)| keycode)
        .ok_or("no key for the keysym")?;
    for &kind in kinds {
        connection
            .xtest_fake_input(kind, keycode, x11rb::CURRENT_TIME, root, 0, 0, 0)?
            .check()?;
    }
    Ok(())
}

/// A deterministic synthetic string over `[A-Za-z0-9_-]`; never a real key.
fn synthetic_key(seed: u64, length: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            char::from(ALPHABET[usize::try_from(state % 64).unwrap_or_default()])
        })
        .collect()
}

fn type_chunks(
    target: &Target,
    client: &mut x11_text::Receiver,
    chunks: Vec<String>,
    lag: std::time::Duration,
) -> x11_text::TestResult<String> {
    let target = target.clone();
    let sender = std::thread::spawn(move || -> horizon_device::Result<()> {
        let mut device = Device::connect(&target)?;
        for text in chunks {
            let geometry = device.screenshot()?.geometry;
            device.act(&ActRequest {
                geometry,
                action: Action::Type { text },
            })?;
        }
        Ok(())
    });
    let received = client.collect_while(lag, || sender.is_finished())?;
    sender.join().map_err(|_| "text sender panicked")??;
    Ok(received)
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
    let capacity = x11_text::mapping_capacity(&connection).map_err(|e| e.to_string())?;
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

fn capture_options_keep_original_geometry(target: &Target) -> Result<(), Box<dyn std::error::Error>> {
    use horizon_device::{CaptureOptions, ImageDimensions, ImageFormat, Region};
    let mut device = Device::connect(target)?;
    let baseline = device.screenshot()?;
    let region = Region {
        x: baseline.geometry.width - 2,
        y: baseline.geometry.height - 2,
        width: 2,
        height: 2,
    };
    for format in [ImageFormat::Png, ImageFormat::Jpeg] {
        let observation = device.screenshot_with(&CaptureOptions {
            region: Some(region),
            output: Some(ImageDimensions { width: 4, height: 4 }),
            format,
            quality: None,
        })?;
        assert_eq!(observation.geometry, baseline.geometry);
        assert_eq!(observation.source_region, region);
        assert_eq!(observation.image_dimensions, ImageDimensions { width: 4, height: 4 });
        let point = observation.surface_point(&Point { x: 3, y: 3 })?;
        assert_eq!(
            (point.x, point.y),
            (
                i32::try_from(baseline.geometry.width - 1)?,
                i32::try_from(baseline.geometry.height - 1)?
            )
        );
        let mut stale = observation.geometry;
        stale.revision.push_str("stale");
        assert!(matches!(
            device.act(&ActRequest {
                geometry: stale,
                action: Action::Click {
                    at: point,
                    button: horizon_device::Button::Left
                }
            }),
            Err(DeviceError::StaleGeometry)
        ));
    }
    assert!(
        device
            .screenshot_with(&CaptureOptions {
                region: Some(Region { width: 3, ..region }),
                ..Default::default()
            })
            .is_err()
    );
    Ok(())
}
