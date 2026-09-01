//! Carbon-backed macOS global hotkeys routed into Horizon's existing events.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, Once, PoisonError};

use global_hotkey::hotkey::HotKey as NativeHotkey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use super::{GlobalHotkeys, Hotkey, HotkeyError, HotkeyEvent, HotkeyKey, WakeFn};

static INSTALL_EVENT_HANDLER: Once = Once::new();
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
static EVENT_ROUTE: Mutex<Option<EventRoute>> = Mutex::new(None);

struct EventRoute {
    generation: u64,
    profiles: HashMap<u32, usize>,
    held: HashSet<u32>,
    sender: Sender<HotkeyEvent>,
    wake: Arc<Mutex<Option<WakeFn>>>,
}

pub(super) struct PlatformGuard {
    generation: u64,
    manager: GlobalHotKeyManager,
    registered: Vec<NativeHotkey>,
}

impl Drop for PlatformGuard {
    fn drop(&mut self) {
        let mut route = EVENT_ROUTE.lock().unwrap_or_else(PoisonError::into_inner);
        if route.as_ref().is_some_and(|route| route.generation == self.generation) {
            route.take();
        }
        drop(route);
        for hotkey in self.registered.iter().rev().copied() {
            let _ = self.manager.unregister(hotkey);
        }
    }
}

pub(super) fn listen(bindings: &[(usize, Hotkey)]) -> Result<GlobalHotkeys, HotkeyError> {
    let native = bindings
        .iter()
        .map(|(profile, hotkey)| native_hotkey(*hotkey).map(|hotkey| (*profile, hotkey)))
        .collect::<Option<Vec<_>>>()
        .ok_or(HotkeyError::Failed(
            "one or more speech hotkeys are unsupported on macOS",
        ))?;
    let manager =
        GlobalHotKeyManager::new().map_err(|_| HotkeyError::Failed("failed to initialize macOS global hotkeys"))?;
    let mut registered = Vec::with_capacity(native.len());
    for (_, hotkey) in &native {
        if manager.register(*hotkey).is_err() {
            for registered_hotkey in registered.iter().rev().copied() {
                let _ = manager.unregister(registered_hotkey);
            }
            return Err(HotkeyError::Failed("macOS denied one or more speech hotkeys"));
        }
        registered.push(*hotkey);
    }

    install_event_handler();
    let (sender, events) = channel();
    let wake = Arc::new(Mutex::new(None));
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    let profiles_by_id = native
        .iter()
        .map(|(profile, hotkey)| (hotkey.id(), *profile))
        .collect::<HashMap<_, _>>();
    let mut profiles = profiles_by_id.values().copied().collect::<Vec<_>>();
    profiles.sort_unstable();
    profiles.dedup();

    let mut route = EVENT_ROUTE.lock().unwrap_or_else(PoisonError::into_inner);
    if route.is_some() {
        return Err(HotkeyError::Failed("another macOS global hotkey listener is active"));
    }
    *route = Some(EventRoute {
        generation,
        profiles: profiles_by_id,
        held: HashSet::new(),
        sender,
        wake: Arc::clone(&wake),
    });
    drop(route);

    Ok(GlobalHotkeys {
        events,
        shutdown: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        thread: None,
        wake,
        profiles,
        _platform_guard: PlatformGuard {
            generation,
            manager,
            registered,
        },
    })
}

fn install_event_handler() {
    INSTALL_EVENT_HANDLER.call_once(|| GlobalHotKeyEvent::set_event_handler(Some(dispatch_event)));
}

fn dispatch_event(event: GlobalHotKeyEvent) {
    let wake = {
        let mut slot = EVENT_ROUTE.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(route) = slot.as_mut() else {
            return;
        };
        let Some(event) = route_event(route, event) else {
            return;
        };
        if route.sender.send(event).is_err() {
            return;
        }
        Arc::clone(&route.wake)
    };
    let callback = wake.lock().unwrap_or_else(PoisonError::into_inner).clone();
    if let Some(callback) = callback {
        callback();
    }
}

fn route_event(route: &mut EventRoute, event: GlobalHotKeyEvent) -> Option<HotkeyEvent> {
    let profile = *route.profiles.get(&event.id)?;
    match event.state {
        HotKeyState::Pressed if route.held.insert(event.id) => Some(HotkeyEvent::Pressed(profile)),
        HotKeyState::Released if route.held.remove(&event.id) => Some(HotkeyEvent::Released(profile)),
        HotKeyState::Pressed | HotKeyState::Released => None,
    }
}

fn native_hotkey(hotkey: Hotkey) -> Option<NativeHotkey> {
    let key = native_key_name(hotkey.key)?;
    let mut parts = Vec::with_capacity(5);
    if hotkey.ctrl {
        parts.push("control".to_owned());
    }
    if hotkey.alt {
        parts.push("alt".to_owned());
    }
    if hotkey.shift || hotkey.key == HotkeyKey::Plus {
        parts.push("shift".to_owned());
    }
    if hotkey.super_ {
        parts.push("super".to_owned());
    }
    parts.push(key);
    parts.join("+").parse().ok()
}

fn native_key_name(key: HotkeyKey) -> Option<String> {
    match key {
        HotkeyKey::Function(index) if (1..=20).contains(&index) => Some(format!("F{index}")),
        HotkeyKey::Letter(letter) if letter.is_ascii_alphabetic() => {
            Some(format!("Key{}", letter.to_ascii_uppercase()))
        }
        HotkeyKey::Digit(digit) if digit <= 9 => Some(format!("Digit{digit}")),
        HotkeyKey::ArrowDown => Some("ArrowDown".to_owned()),
        HotkeyKey::ArrowLeft => Some("ArrowLeft".to_owned()),
        HotkeyKey::ArrowRight => Some("ArrowRight".to_owned()),
        HotkeyKey::ArrowUp => Some("ArrowUp".to_owned()),
        HotkeyKey::Enter => Some("Enter".to_owned()),
        HotkeyKey::Tab => Some("Tab".to_owned()),
        HotkeyKey::Comma => Some("Comma".to_owned()),
        HotkeyKey::Minus => Some("Minus".to_owned()),
        HotkeyKey::Plus => Some("Equal".to_owned()),
        HotkeyKey::Function(_) | HotkeyKey::Letter(_) | HotkeyKey::Digit(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};

    use global_hotkey::hotkey::{Code, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, HotKeyState};

    use super::{EventRoute, Hotkey, HotkeyEvent, HotkeyKey, native_hotkey, route_event};

    fn plain(key: HotkeyKey) -> Hotkey {
        Hotkey {
            ctrl: false,
            shift: false,
            alt: false,
            super_: false,
            key,
        }
    }

    #[test]
    fn supported_keys_and_plus_map_to_carbon_compatible_codes() {
        let f1 = native_hotkey(plain(HotkeyKey::Function(1))).expect("F1");
        assert_eq!(f1.key, Code::F1);
        assert!(native_hotkey(plain(HotkeyKey::Function(20))).is_some());
        assert!(native_hotkey(plain(HotkeyKey::Function(21))).is_none());

        let plus = native_hotkey(plain(HotkeyKey::Plus)).expect("plus");
        assert_eq!(plus.key, Code::Equal);
        assert!(plus.mods.contains(Modifiers::SHIFT));
    }

    #[test]
    fn native_repeats_are_deduplicated_until_release() {
        let (sender, _events) = channel();
        let mut route = EventRoute {
            generation: 1,
            profiles: HashMap::from([(42, 7)]),
            held: HashSet::new(),
            sender,
            wake: Arc::new(Mutex::new(None)),
        };
        let event = |state| GlobalHotKeyEvent { id: 42, state };
        assert_eq!(
            route_event(&mut route, event(HotKeyState::Pressed)),
            Some(HotkeyEvent::Pressed(7))
        );
        assert_eq!(route_event(&mut route, event(HotKeyState::Pressed)), None);
        assert_eq!(
            route_event(&mut route, event(HotKeyState::Released)),
            Some(HotkeyEvent::Released(7))
        );
        assert_eq!(route_event(&mut route, event(HotKeyState::Released)), None);
    }
}
