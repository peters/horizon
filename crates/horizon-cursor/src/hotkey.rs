//! Global push-to-talk listener. Press/release events are delivered even when
//! another application has focus.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

type WakeFn = Arc<dyn Fn() + Send + Sync>;

/// One binding the listener should grab, tagged with the caller's profile index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub super_: bool,
    pub key: HotkeyKey,
}

/// Keys we can grab globally. Mirrors accepted speech [`ShortcutKey`] values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HotkeyKey {
    Function(u8),
    Letter(char),
    Digit(u8),
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Enter,
    Tab,
    Comma,
    Minus,
    Plus,
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
    wake: Arc<Mutex<Option<WakeFn>>>,
    profiles: Vec<usize>,
}

impl GlobalHotkeys {
    /// Grab `bindings` (profile index, spec).
    ///
    /// # Errors
    /// Returns [`HotkeyError::Unsupported`] when `bindings` is empty, this
    /// session is Wayland (`WAYLAND_DISPLAY` is set), or there is no X11
    /// display. [`HotkeyError::Failed`] is returned when none of the bindings
    /// could be grabbed. Bindings that fail individually are omitted from
    /// [`Self::profiles`] so local handling can keep them.
    pub fn listen(bindings: &[(usize, Hotkey)]) -> Result<Self, HotkeyError> {
        if bindings.is_empty() || !session_supports_global_hotkeys(std::env::var_os("WAYLAND_DISPLAY").as_deref()) {
            return Err(HotkeyError::Unsupported);
        }
        platform::listen(bindings)
    }

    /// Ask the UI to repaint when a grabbed key is pressed or released.
    /// Subsequent calls are ignored so the first installed waker wins.
    pub fn set_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        let mut slot = self.wake.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(Arc::new(wake));
        }
    }

    #[must_use]
    pub fn has_wake(&self) -> bool {
        self.wake.lock().unwrap_or_else(PoisonError::into_inner).is_some()
    }

    #[must_use]
    pub fn try_recv(&self) -> Option<HotkeyEvent> {
        self.events.try_recv().ok()
    }

    /// Profiles whose bindings were actually grabbed. Bindings the backend
    /// could not register are omitted so local egui handling can keep them.
    #[must_use]
    pub fn profiles(&self) -> &[usize] {
        &self.profiles
    }
}

fn session_supports_global_hotkeys(wayland_display: Option<&std::ffi::OsStr>) -> bool {
    wayland_display.is_none()
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread;
    use std::time::Duration;

    use x11rb::connection::Connection as _;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, ModMask};

    use super::{GlobalHotkeys, Hotkey, HotkeyError, HotkeyEvent, HotkeyKey};
    use crate::inject::keysym_to_keycode_with_shift;

    const XK_F1: u32 = 0xffbe;
    const XK_0: u32 = 0x0030;
    const XK_TAB: u32 = 0xff09;
    const XK_RETURN: u32 = 0xff0d;
    const XK_LEFT: u32 = 0xff51;
    const XK_UP: u32 = 0xff52;
    const XK_RIGHT: u32 = 0xff53;
    const XK_DOWN: u32 = 0xff54;
    const XK_COMMA: u32 = 0x002c;
    const XK_MINUS: u32 = 0x002d;
    const XK_PLUS: u32 = 0x002b;

    pub(super) fn listen(bindings: &[(usize, Hotkey)]) -> Result<GlobalHotkeys, HotkeyError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(|_| HotkeyError::Unsupported)?;
        let root = conn
            .setup()
            .roots
            .get(screen_num)
            .map(|screen| screen.root)
            .ok_or(HotkeyError::Failed("X11 screen missing"))?;

        let mut grabbed: HashMap<(u8, u16), usize> = HashMap::new();
        for (profile, hotkey) in bindings {
            let Some((keycode, mods)) = grab_spec(&conn, *hotkey) else {
                continue;
            };
            if grab_all_lock_variants(&conn, root, keycode, mods).is_err() {
                continue;
            }
            grabbed.insert(grab_key(keycode, mods), *profile);
        }
        if grabbed.is_empty() {
            return Err(HotkeyError::Failed("no speech hotkeys could be grabbed"));
        }
        let mut profiles: Vec<usize> = grabbed.values().copied().collect();
        profiles.sort_unstable();
        profiles.dedup();
        conn.flush().map_err(|_| HotkeyError::Failed("X11 flush failed"))?;

        let (tx, events) = channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);
        let wake = Arc::new(Mutex::new(None));
        let thread_wake = Arc::clone(&wake);
        let thread = thread::Builder::new()
            .name("horizon-speech-hotkeys".to_string())
            .spawn(move || {
                let mut held: HashMap<u8, usize> = HashMap::new();
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
                            if let Some(profile) = activate_press(&grabbed, &mut held, press.detail, press.state) {
                                if tx.send(HotkeyEvent::Pressed(profile)).is_err() {
                                    break;
                                }
                                invoke_wake(&thread_wake);
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
                            if let Some(profile) = activate_release(&mut held, release.detail) {
                                if tx.send(HotkeyEvent::Released(profile)).is_err() {
                                    break;
                                }
                                invoke_wake(&thread_wake);
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
            wake,
            profiles,
        })
    }

    fn activate_press(
        grabbed: &HashMap<(u8, u16), usize>,
        held: &mut HashMap<u8, usize>,
        keycode: u8,
        state: x11rb::protocol::xproto::KeyButMask,
    ) -> Option<usize> {
        let profile = *grabbed.get(&event_key(keycode, state))?;
        if held.contains_key(&keycode) {
            return None;
        }
        held.insert(keycode, profile);
        Some(profile)
    }

    fn activate_release(held: &mut HashMap<u8, usize>, keycode: u8) -> Option<usize> {
        held.remove(&keycode)
    }

    fn invoke_wake(wake: &Mutex<Option<super::WakeFn>>) {
        let callback = wake.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(callback) = callback {
            callback();
        }
    }

    fn lock_modifier_mask() -> u16 {
        u16::from(ModMask::LOCK) | u16::from(ModMask::M2)
    }

    fn grab_modifier_mask() -> u16 {
        u16::from(ModMask::SHIFT)
            | u16::from(ModMask::CONTROL)
            | u16::from(ModMask::M1)
            | u16::from(ModMask::M4)
            | lock_modifier_mask()
    }

    fn normalized_mods(state: u16) -> u16 {
        state & !lock_modifier_mask()
    }

    fn grab_key(keycode: u8, mods: ModMask) -> (u8, u16) {
        (keycode, normalized_mods(u16::from(mods)))
    }

    fn event_key(keycode: u8, state: x11rb::protocol::xproto::KeyButMask) -> (u8, u16) {
        (keycode, normalized_mods(u16::from(state) & grab_modifier_mask()))
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
        for (index, extra) in extras.iter().copied().enumerate() {
            let grabbed = conn
                .grab_key(true, root, base | extra, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                .map_err(|_| HotkeyError::Failed("XGrabKey failed"))
                .and_then(|cookie| cookie.check().map_err(|_| HotkeyError::Failed("XGrabKey denied")));
            if grabbed.is_err() {
                for extra in extras.iter().copied().take(index) {
                    let _ = conn.ungrab_key(keycode, root, base | extra);
                }
                return grabbed;
            }
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

    fn grab_spec<C: x11rb::connection::Connection>(conn: &C, hotkey: Hotkey) -> Option<(u8, ModMask)> {
        let (keycode, shift_level) = keysym_to_keycode_with_shift(conn, keysym(hotkey.key)?)?;
        let mut mods = modifiers(hotkey);
        if shift_level {
            mods |= ModMask::SHIFT;
        }
        Some((keycode, mods))
    }

    fn keysym(key: HotkeyKey) -> Option<u32> {
        match key {
            HotkeyKey::Function(index) => (1..=35)
                .contains(&index)
                .then_some(XK_F1 + u32::from(index.saturating_sub(1))),
            HotkeyKey::Letter(letter) => {
                let lower = letter.to_ascii_lowercase();
                lower.is_ascii_alphabetic().then_some(u32::from(lower as u8))
            }
            HotkeyKey::Digit(digit) => (digit <= 9).then_some(XK_0 + u32::from(digit)),
            HotkeyKey::ArrowLeft => Some(XK_LEFT),
            HotkeyKey::ArrowUp => Some(XK_UP),
            HotkeyKey::ArrowRight => Some(XK_RIGHT),
            HotkeyKey::ArrowDown => Some(XK_DOWN),
            HotkeyKey::Enter => Some(XK_RETURN),
            HotkeyKey::Tab => Some(XK_TAB),
            HotkeyKey::Comma => Some(XK_COMMA),
            HotkeyKey::Minus => Some(XK_MINUS),
            HotkeyKey::Plus => Some(XK_PLUS),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{
            Hotkey, HotkeyKey, activate_press, activate_release, grab_key, keysym, modifiers, normalized_mods,
        };
        use std::collections::HashMap;
        use x11rb::protocol::xproto::{KeyButMask, ModMask};

        #[test]
        fn function_and_latin_keys_map_to_x11_keysyms() {
            assert_eq!(keysym(HotkeyKey::Function(1)), Some(0xffbe));
            assert_eq!(keysym(HotkeyKey::Function(9)), Some(0xffc6));
            assert_eq!(keysym(HotkeyKey::Function(25)), Some(0xffd6));
            assert_eq!(keysym(HotkeyKey::Function(35)), Some(0xffe0));
            assert_eq!(keysym(HotkeyKey::Letter('V')), Some(u32::from(b'v')));
            assert_eq!(keysym(HotkeyKey::Digit(0)), Some(0x0030));
            assert_eq!(keysym(HotkeyKey::Function(0)), None);
            assert_eq!(keysym(HotkeyKey::Function(36)), None);
            assert_eq!(keysym(HotkeyKey::ArrowUp), Some(0xff52));
            assert_eq!(keysym(HotkeyKey::Enter), Some(0xff0d));
            assert_eq!(keysym(HotkeyKey::Plus), Some(0x002b));
        }

        #[test]
        fn release_uses_the_press_keycode_even_when_modifiers_have_changed() {
            let mut grabbed = HashMap::new();
            grabbed.insert(
                grab_key(
                    45,
                    modifiers(Hotkey {
                        ctrl: true,
                        shift: false,
                        alt: false,
                        super_: false,
                        key: HotkeyKey::Letter('k'),
                    }),
                ),
                3,
            );
            let mut held = HashMap::new();
            let control = u16::from(ModMask::CONTROL);
            assert_eq!(
                activate_press(
                    &grabbed,
                    &mut held,
                    45,
                    x11rb::protocol::xproto::KeyButMask::from(control)
                ),
                Some(3)
            );
            assert_eq!(activate_release(&mut held, 45), Some(3));
            assert!(held.is_empty());
        }

        #[test]
        fn pointer_button_bits_do_not_block_a_ctrl_binding() {
            let mut grabbed = HashMap::new();
            grabbed.insert(
                grab_key(
                    45,
                    modifiers(Hotkey {
                        ctrl: true,
                        shift: false,
                        alt: false,
                        super_: false,
                        key: HotkeyKey::Letter('k'),
                    }),
                ),
                3,
            );
            let mut held = HashMap::new();
            let state = u16::from(ModMask::CONTROL) | u16::from(KeyButMask::BUTTON1);
            assert_eq!(
                activate_press(&grabbed, &mut held, 45, KeyButMask::from(state)),
                Some(3)
            );
        }

        #[test]
        fn modifier_masks_keep_ctrl_and_alt_bindings_distinct() {
            let letter_k = Hotkey {
                ctrl: false,
                shift: false,
                alt: false,
                super_: false,
                key: HotkeyKey::Letter('k'),
            };
            let ctrl_k = Hotkey { ctrl: true, ..letter_k };
            let alt_k = Hotkey { alt: true, ..letter_k };
            assert_ne!(grab_key(45, modifiers(ctrl_k)), grab_key(45, modifiers(alt_k)));
            let caps = u16::from(ModMask::CONTROL) | u16::from(ModMask::LOCK);
            assert_eq!(normalized_mods(caps), u16::from(ModMask::CONTROL));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GlobalHotkeys, HotkeyError};

    #[test]
    fn empty_binding_list_does_not_claim_a_listener() {
        assert!(matches!(GlobalHotkeys::listen(&[]), Err(HotkeyError::Unsupported)));
    }

    #[test]
    fn wayland_display_disables_global_hotkeys() {
        assert!(!super::session_supports_global_hotkeys(Some(std::ffi::OsStr::new(
            "wayland-0"
        ))));
        assert!(super::session_supports_global_hotkeys(None));
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
        .expect("grab F9");
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
