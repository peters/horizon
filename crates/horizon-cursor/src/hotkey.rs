//! Global push-to-talk listener. Press/release events are delivered even when
//! another application has focus.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;

/// One binding the listener should grab, tagged with the caller's profile index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub super_: bool,
    pub key: HotkeyKey,
}

/// Keys we can grab globally. Speech hotkeys are function keys or modified letters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HotkeyKey {
    Function(u8),
    Letter(char),
    Digit(u8),
}

/// A grabbed binding became active or inactive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HotkeyEvent {
    Pressed(usize),
    Released(usize),
}

/// Failure to listen for global hotkeys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HotkeyError {
    Unsupported,
    Failed(&'static str),
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("global hotkeys are not available on this display"),
            Self::Failed(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for HotkeyError {}

/// Background X11 grab of speech push-to-talk bindings.
pub struct GlobalHotkeys {
    events: Receiver<HotkeyEvent>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl GlobalHotkeys {
    /// Grab `bindings` (profile index, spec). No-op empty list.
    ///
    /// # Errors
    /// Returns [`HotkeyError::Unsupported`] when this session has no X11 display,
    /// or [`HotkeyError::Failed`] when the grab could not be established.
    pub fn listen(bindings: &[(usize, Hotkey)]) -> Result<Self, HotkeyError> {
        if bindings.is_empty() {
            let (tx, events) = channel();
            drop(tx);
            return Ok(Self {
                events,
                shutdown: Arc::new(AtomicBool::new(false)),
                thread: None,
            });
        }
        platform::listen(bindings)
    }

    #[must_use]
    pub fn try_recv(&self) -> Option<HotkeyEvent> {
        self.events.try_recv().ok()
    }
}

impl Drop for GlobalHotkeys {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod platform {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::thread;
    use std::time::Duration;

    use x11rb::connection::Connection as _;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, ModMask};

    use super::{GlobalHotkeys, Hotkey, HotkeyError, HotkeyEvent, HotkeyKey};
    use crate::inject::keysym_to_keycode;

    const XK_F1: u32 = 0xffbe;
    const XK_0: u32 = 0x0030;

    pub(super) fn listen(bindings: &[(usize, Hotkey)]) -> Result<GlobalHotkeys, HotkeyError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(|_| HotkeyError::Unsupported)?;
        let root = conn
            .setup()
            .roots
            .get(screen_num)
            .map(|screen| screen.root)
            .ok_or(HotkeyError::Failed("X11 screen missing"))?;

        let mut grabbed: HashMap<u8, usize> = HashMap::new();
        for (profile, hotkey) in bindings {
            let Some(keycode) = hotkey_keycode(&conn, *hotkey) else {
                continue;
            };
            grab_all_lock_variants(&conn, root, keycode, modifiers(*hotkey))?;
            grabbed.insert(keycode, *profile);
        }
        if grabbed.is_empty() {
            return Err(HotkeyError::Failed("no speech hotkeys could be grabbed"));
        }
        conn.flush().map_err(|_| HotkeyError::Failed("X11 flush failed"))?;

        let (tx, events) = channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);
        let thread = thread::Builder::new()
            .name("horizon-speech-hotkeys".to_string())
            .spawn(move || {
                let mut held: HashMap<u8, bool> = HashMap::new();
                let mut pending: Option<Event> = None;
                while !thread_shutdown.load(Ordering::Relaxed) {
                    let event = match pending.take() {
                        Some(event) => event,
                        None => match conn.poll_for_event() {
                            Ok(Some(event)) => event,
                            Ok(None) => {
                                thread::sleep(Duration::from_millis(20));
                                continue;
                            }
                            Err(_) => break,
                        },
                    };
                    match event {
                        Event::KeyPress(press) => {
                            if let Some(&profile) = grabbed.get(&press.detail)
                                && !held.get(&press.detail).copied().unwrap_or(false)
                            {
                                held.insert(press.detail, true);
                                if tx.send(HotkeyEvent::Pressed(profile)).is_err() {
                                    break;
                                }
                            }
                        }
                        Event::KeyRelease(release) => {
                            let repeat = match conn.poll_for_event() {
                                Ok(Some(Event::KeyPress(press)))
                                    if press.detail == release.detail && press.time == release.time =>
                                {
                                    true
                                }
                                Ok(Some(other)) => {
                                    pending = Some(other);
                                    false
                                }
                                _ => false,
                            };
                            if repeat {
                                continue;
                            }
                            if let Some(&profile) = grabbed.get(&release.detail)
                                && held.insert(release.detail, false).unwrap_or(false)
                                && tx.send(HotkeyEvent::Released(profile)).is_err()
                            {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            })
            .map_err(|_| HotkeyError::Failed("failed to start global hotkey thread"))?;

        Ok(GlobalHotkeys {
            events,
            shutdown,
            thread: Some(thread),
        })
    }

    fn grab_all_lock_variants<C: x11rb::connection::Connection>(
        conn: &C,
        root: u32,
        keycode: u8,
        base: ModMask,
    ) -> Result<(), HotkeyError> {
        let extras = [
            ModMask::from(0u16),
            ModMask::LOCK,
            ModMask::M2,
            ModMask::LOCK | ModMask::M2,
        ];
        for extra in extras {
            conn.grab_key(true, root, base | extra, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                .map_err(|_| HotkeyError::Failed("XGrabKey failed"))?
                .check()
                .map_err(|_| HotkeyError::Failed("XGrabKey denied"))?;
        }
        Ok(())
    }

    fn modifiers(hotkey: Hotkey) -> ModMask {
        let mut mask = ModMask::from(0u16);
        if hotkey.ctrl {
            mask |= ModMask::CONTROL;
        }
        if hotkey.shift {
            mask |= ModMask::SHIFT;
        }
        if hotkey.alt {
            mask |= ModMask::M1;
        }
        if hotkey.super_ {
            mask |= ModMask::M4;
        }
        mask
    }

    fn hotkey_keycode<C: x11rb::connection::Connection>(conn: &C, hotkey: Hotkey) -> Option<u8> {
        keysym_to_keycode(conn, keysym(hotkey.key)?)
    }

    fn keysym(key: HotkeyKey) -> Option<u32> {
        match key {
            HotkeyKey::Function(index) => (1..=24)
                .contains(&index)
                .then_some(XK_F1 + u32::from(index.saturating_sub(1))),
            HotkeyKey::Letter(letter) => {
                let lower = letter.to_ascii_lowercase();
                lower.is_ascii_alphabetic().then_some(u32::from(lower as u8))
            }
            HotkeyKey::Digit(digit) => (digit <= 9).then_some(XK_0 + u32::from(digit)),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{HotkeyKey, keysym};

        #[test]
        fn function_and_latin_keys_map_to_x11_keysyms() {
            assert_eq!(keysym(HotkeyKey::Function(1)), Some(0xffbe));
            assert_eq!(keysym(HotkeyKey::Function(9)), Some(0xffc6));
            assert_eq!(keysym(HotkeyKey::Letter('V')), Some(u32::from(b'v')));
            assert_eq!(keysym(HotkeyKey::Digit(0)), Some(0x0030));
            assert_eq!(keysym(HotkeyKey::Function(0)), None);
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::{GlobalHotkeys, Hotkey, HotkeyEvent, HotkeyKey};

    #[test]
    #[ignore = "live X11 nested-display smoke"]
    fn live_global_hotkey_receives_press_and_release() {
        let hotkeys = GlobalHotkeys::listen(&[(
            7,
            Hotkey {
                ctrl: false,
                shift: false,
                alt: false,
                super_: false,
                key: HotkeyKey::Function(9),
            },
        )])
        .expect("grab F24");
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Some(event) = hotkeys.try_recv() {
                events.push(event);
                if events.iter().any(|event| matches!(event, HotkeyEvent::Released(7))) {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            events.contains(&HotkeyEvent::Pressed(7)) && events.contains(&HotkeyEvent::Released(7)),
            "events: {events:?}"
        );
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
mod platform {
    use super::{GlobalHotkeys, Hotkey, HotkeyError};

    pub(super) fn listen(_bindings: &[(usize, Hotkey)]) -> Result<GlobalHotkeys, HotkeyError> {
        Err(HotkeyError::Unsupported)
    }
}
