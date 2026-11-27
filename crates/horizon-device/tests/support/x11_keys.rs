//! The focused X11 window that the live key-chord test receives its events in.
//!
//! The window is override-redirect on the display that the target names, so a
//! chord reaches the test and not the desktop of the person.

use horizon_device::{ActRequest, Action, Device, DeviceError, Key, Modifier};
use std::time::{Duration, Instant};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xproto::{ConnectionExt, CreateWindowAux, EventMask, InputFocus, WindowClass},
    },
    rust_connection::RustConnection,
};

/// The key events of a chord arrive in the own override-redirect window of the
/// test on the display of the target, never on the desktop of the person.
pub struct KeyWindow {
    connection: RustConnection,
    root: x11rb::protocol::xproto::Window,
    window: x11rb::protocol::xproto::Window,
}

impl KeyWindow {
    /// Opens the window on `display` and gives it the input focus.
    pub fn focused(display: &str) -> Result<Self, Box<dyn std::error::Error>> {
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
                240,
                120,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
            )?
            .check()?;
        connection.map_window(window)?.check()?;
        connection
            .set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)?
            .check()?;
        Ok(Self {
            connection,
            root,
            window,
        })
    }

    /// Asserts that a key action with a stale geometry sends nothing, so the
    /// chords below measure accepted input only.
    pub fn assert_stale_geometry_sends_no_key(&self, device: &mut Device) -> Result<(), Box<dyn std::error::Error>> {
        let mut stale = device.screenshot()?.geometry;
        stale.revision.push_str("stale");
        assert!(matches!(
            device.act(&ActRequest {
                geometry: stale,
                action: Action::Key {
                    key: Key::A,
                    modifiers: vec![Modifier::Control],
                },
            }),
            Err(DeviceError::StaleGeometry)
        ));
        let quiet = Instant::now() + Duration::from_millis(200);
        while Instant::now() < quiet {
            assert!(
                !matches!(
                    self.connection.poll_for_event()?,
                    Some(Event::KeyPress(_) | Event::KeyRelease(_))
                ),
                "stale geometry sent a key event"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            u16::from(self.connection.query_pointer(self.root)?.reply()?.mask) & 0x0005,
            0,
            "stale geometry left Control or Shift held"
        );
        Ok(())
    }

    /// Sends one chord and asserts that its key reports `keysym` with Control
    /// and Shift at `state`, and that both are released afterwards.
    pub fn send_chord(
        &self,
        device: &mut Device,
        key: Key,
        modifiers: Vec<Modifier>,
        keysym: u32,
        state: u16,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let geometry = device.screenshot()?.geometry;
        device.act(&ActRequest {
            geometry,
            action: Action::Key { key, modifiers },
        })?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut pressed = false;
        while Instant::now() < deadline && !pressed {
            while let Some(event) = self.connection.poll_for_event()? {
                let Event::KeyPress(event) = event else {
                    continue;
                };
                let mapping = self.connection.get_keyboard_mapping(event.detail, 1)?.reply()?;
                let symbol = mapping.keysyms.first().copied().unwrap_or_default();
                // Each modifier press is part of the chord; only the shortcut key
                // itself must report the expected lowercase keysym.
                if matches!(symbol, 0xffe1..=0xffe4) {
                    continue;
                }
                assert_eq!(symbol, keysym, "unexpected keysym for {key:?}");
                assert_eq!(
                    u16::from(event.state) & 0x0005,
                    state,
                    "unexpected Control/Shift state for {key:?}"
                );
                pressed = true;
            }
            if !pressed {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(pressed, "no KeyPress event for {key:?} within 2 seconds");
        assert_eq!(
            u16::from(self.connection.query_pointer(self.root)?.reply()?.mask) & 0x0005,
            0,
            "Control or Shift stayed held after {key:?}"
        );
        Ok(())
    }

    /// Closes the window, which releases the focus that it took.
    pub fn destroy(self) -> Result<(), Box<dyn std::error::Error>> {
        self.connection.destroy_window(self.window)?.check()?;
        Ok(())
    }
}
