use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent as NativeEvent, GlobalHotKeyManager, HotKeyState};
use macos_accessibility_client::accessibility::{application_is_trusted, application_is_trusted_with_prompt};

use super::{
    ActiveBinding, GlobalHotkeyEvent, GlobalHotkeyStatus, NativeHotkey, NativeKey, NativeModifiers,
    RegistrationBackend, RegistrationFailure, RegistrationPreflight, accept_event, quarantine_binding_ids,
    register_transaction, registration_preflight, remember_sticky_unregister_failure, unregister_with_sticky_failure,
};
use horizon_core::SpeechConfig;

#[derive(Clone, Copy)]
struct QueuedEvent {
    generation: u64,
    id: u32,
    pressed: bool,
}

struct Destination {
    sender: Sender<QueuedEvent>,
    context: egui::Context,
}

struct EventBridge {
    accepting: AtomicBool,
    generation: AtomicU64,
    destination: Mutex<Option<Destination>>,
}

impl EventBridge {
    fn attach(&self, sender: Sender<QueuedEvent>, context: egui::Context) {
        *self
            .destination
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Destination { sender, context });
    }

    fn pause_and_advance(&self) -> u64 {
        self.accepting.store(false, Ordering::Release);
        let _destination = self
            .destination
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn resume(&self) {
        self.accepting.store(true, Ordering::Release);
    }

    fn queue(&self, event: NativeEvent) {
        if !self.accepting.load(Ordering::Acquire) {
            return;
        }
        let destination = self
            .destination
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.accepting.load(Ordering::Acquire) {
            return;
        }
        let queued = QueuedEvent {
            generation: self.generation.load(Ordering::Acquire),
            id: event.id,
            pressed: event.state == HotKeyState::Pressed,
        };
        if let Some(destination) = destination.as_ref() {
            let _ = destination.sender.send(queued);
            destination.context.request_repaint();
        }
    }
}

fn event_bridge() -> Arc<EventBridge> {
    static BRIDGE: OnceLock<Arc<EventBridge>> = OnceLock::new();
    Arc::clone(BRIDGE.get_or_init(|| {
        let bridge = Arc::new(EventBridge {
            accepting: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            destination: Mutex::new(None),
        });
        let handler_bridge = Arc::clone(&bridge);
        NativeEvent::set_event_handler(Some(move |event| handler_bridge.queue(event)));
        bridge
    }))
}

struct MacRegistrar(GlobalHotKeyManager);

impl RegistrationBackend for MacRegistrar {
    type Error = global_hotkey::Error;

    fn register(&mut self, hotkey: NativeHotkey) -> Result<u32, Self::Error> {
        let hotkey = to_native(hotkey);
        self.0.register(hotkey)?;
        Ok(hotkey.id())
    }

    fn unregister(&mut self, hotkey: NativeHotkey) -> Result<(), Self::Error> {
        self.0.unregister(to_native(hotkey))
    }
}

fn to_native(hotkey: NativeHotkey) -> HotKey {
    let mut modifiers = Modifiers::empty();
    if hotkey.modifiers.0 & NativeModifiers::ALT != 0 {
        modifiers |= Modifiers::ALT;
    }
    if hotkey.modifiers.0 & NativeModifiers::CONTROL != 0 {
        modifiers |= Modifiers::CONTROL;
    }
    if hotkey.modifiers.0 & NativeModifiers::SHIFT != 0 {
        modifiers |= Modifiers::SHIFT;
    }
    if hotkey.modifiers.0 & NativeModifiers::SUPER != 0 {
        modifiers |= Modifiers::SUPER;
    }
    HotKey::new(Some(modifiers), code(hotkey.key))
}

fn code(key: NativeKey) -> Code {
    match key {
        NativeKey::ArrowDown => Code::ArrowDown,
        NativeKey::ArrowLeft => Code::ArrowLeft,
        NativeKey::ArrowRight => Code::ArrowRight,
        NativeKey::ArrowUp => Code::ArrowUp,
        NativeKey::Escape => Code::Escape,
        NativeKey::Enter => Code::Enter,
        NativeKey::Tab => Code::Tab,
        NativeKey::Comma => Code::Comma,
        NativeKey::Minus => Code::Minus,
        NativeKey::Digit(0) => Code::Digit0,
        NativeKey::Digit(1) => Code::Digit1,
        NativeKey::Digit(2) => Code::Digit2,
        NativeKey::Digit(3) => Code::Digit3,
        NativeKey::Digit(4) => Code::Digit4,
        NativeKey::Digit(5) => Code::Digit5,
        NativeKey::Digit(6) => Code::Digit6,
        NativeKey::Digit(7) => Code::Digit7,
        NativeKey::Digit(8) => Code::Digit8,
        NativeKey::Digit(9) => Code::Digit9,
        NativeKey::Letter('A') => Code::KeyA,
        NativeKey::Letter('B') => Code::KeyB,
        NativeKey::Letter('C') => Code::KeyC,
        NativeKey::Letter('D') => Code::KeyD,
        NativeKey::Letter('E') => Code::KeyE,
        NativeKey::Letter('F') => Code::KeyF,
        NativeKey::Letter('G') => Code::KeyG,
        NativeKey::Letter('H') => Code::KeyH,
        NativeKey::Letter('I') => Code::KeyI,
        NativeKey::Letter('J') => Code::KeyJ,
        NativeKey::Letter('K') => Code::KeyK,
        NativeKey::Letter('L') => Code::KeyL,
        NativeKey::Letter('M') => Code::KeyM,
        NativeKey::Letter('N') => Code::KeyN,
        NativeKey::Letter('O') => Code::KeyO,
        NativeKey::Letter('P') => Code::KeyP,
        NativeKey::Letter('Q') => Code::KeyQ,
        NativeKey::Letter('R') => Code::KeyR,
        NativeKey::Letter('S') => Code::KeyS,
        NativeKey::Letter('T') => Code::KeyT,
        NativeKey::Letter('U') => Code::KeyU,
        NativeKey::Letter('V') => Code::KeyV,
        NativeKey::Letter('W') => Code::KeyW,
        NativeKey::Letter('X') => Code::KeyX,
        NativeKey::Letter('Y') => Code::KeyY,
        NativeKey::Letter('Z') => Code::KeyZ,
        NativeKey::Function(1) => Code::F1,
        NativeKey::Function(2) => Code::F2,
        NativeKey::Function(3) => Code::F3,
        NativeKey::Function(4) => Code::F4,
        NativeKey::Function(5) => Code::F5,
        NativeKey::Function(6) => Code::F6,
        NativeKey::Function(7) => Code::F7,
        NativeKey::Function(8) => Code::F8,
        NativeKey::Function(9) => Code::F9,
        NativeKey::Function(10) => Code::F10,
        NativeKey::Function(11) => Code::F11,
        NativeKey::Function(12) => Code::F12,
        NativeKey::Function(13) => Code::F13,
        NativeKey::Function(14) => Code::F14,
        NativeKey::Function(15) => Code::F15,
        NativeKey::Function(16) => Code::F16,
        NativeKey::Function(17) => Code::F17,
        NativeKey::Function(18) => Code::F18,
        NativeKey::Function(19) => Code::F19,
        NativeKey::Function(20) => Code::F20,
        NativeKey::Digit(_) | NativeKey::Letter(_) | NativeKey::Function(_) => unreachable!(),
    }
}

pub(super) struct PlatformGlobalHotkeys {
    context: egui::Context,
    committed: SpeechConfig,
    status: GlobalHotkeyStatus,
    paused: bool,
    registrar: Option<MacRegistrar>,
    active: Vec<ActiveBinding>,
    receiver: Receiver<QueuedEvent>,
    sender: Sender<QueuedEvent>,
    generation: u64,
    pressed: HashSet<u32>,
    suppressed_until_release: HashSet<u32>,
    quarantined_keys: HashSet<NativeKey>,
    unregister_failure: Option<(String, String)>,
}

impl PlatformGlobalHotkeys {
    pub(super) fn new(context: &egui::Context, config: &SpeechConfig) -> Self {
        let (sender, receiver) = mpsc::channel();
        let mut globals = Self {
            context: context.clone(),
            committed: config.clone(),
            status: GlobalHotkeyStatus::Off,
            paused: false,
            registrar: None,
            active: Vec::new(),
            receiver,
            sender,
            generation: 0,
            pressed: HashSet::new(),
            suppressed_until_release: HashSet::new(),
            quarantined_keys: HashSet::new(),
            unregister_failure: None,
        };
        globals.apply(false);
        globals
    }

    pub(super) const fn status(&self) -> &GlobalHotkeyStatus {
        &self.status
    }

    pub(super) fn is_registered(&self) -> bool {
        !self.active.is_empty() && matches!(self.status, GlobalHotkeyStatus::Registered { .. })
    }

    pub(super) fn drain_events(&mut self) -> Vec<GlobalHotkeyEvent> {
        let Ok(first) = self.receiver.try_recv() else {
            return Vec::new();
        };
        let profiles: HashMap<u32, usize> = self
            .active
            .iter()
            .map(|binding| (binding.id, binding.desired.profile))
            .collect();
        let mut events = Vec::new();
        if let Some(event) = accept_event(
            self.generation,
            first.generation,
            first.id,
            first.pressed,
            &profiles,
            &mut self.pressed,
            &mut self.suppressed_until_release,
        ) {
            events.push(event);
        }
        while let Ok(event) = self.receiver.try_recv() {
            if let Some(event) = accept_event(
                self.generation,
                event.generation,
                event.id,
                event.pressed,
                &profiles,
                &mut self.pressed,
                &mut self.suppressed_until_release,
            ) {
                events.push(event);
            }
        }
        events
    }

    pub(super) fn clear_event_ownership(&mut self) {
        let registered = self.is_registered();
        if self.registrar.is_some() {
            let bridge = event_bridge();
            self.generation = bridge.pause_and_advance();
            self.quarantine_inflight_presses();
            if registered {
                bridge.resume();
            }
        } else {
            self.quarantine_inflight_presses();
        }
    }

    pub(super) fn reconfigure_committed(&mut self, config: &SpeechConfig) {
        self.committed = config.clone();
        if !self.paused {
            self.apply(false);
        }
    }

    pub(super) fn set_rebinding_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        if paused {
            self.status = match self.pause_and_unregister() {
                Ok(()) => GlobalHotkeyStatus::PausedForRebinding,
                Err((binding, reason)) => GlobalHotkeyStatus::RegistrationConflict {
                    binding,
                    reason,
                    retryable: false,
                },
            };
        } else {
            self.apply(false);
        }
    }

    pub(super) fn quarantine_captured_key(&mut self, key: egui::Key, physical_key: Option<egui::Key>) {
        if let Some(key) = native_key_from_egui(key) {
            self.quarantined_keys.insert(key);
        }
        if let Some(key) = physical_key.and_then(native_key_from_egui) {
            self.quarantined_keys.insert(key);
        }
    }

    pub(super) fn grant_accessibility_and_retry(&mut self, config: &SpeechConfig) {
        self.committed = config.clone();
        self.apply(true);
    }

    pub(super) fn retry(&mut self, config: &SpeechConfig) {
        self.committed = config.clone();
        self.apply(false);
    }

    pub(super) fn permission_is_current(&self) -> bool {
        application_is_trusted()
    }

    pub(super) fn unregister_all(&mut self) {
        if let Err((binding, reason)) = self.pause_and_unregister() {
            self.status = GlobalHotkeyStatus::RegistrationConflict {
                binding,
                reason,
                retryable: false,
            };
            return;
        }
        self.status = if self.wants_globals() && !application_is_trusted() {
            GlobalHotkeyStatus::AccessibilityRequired
        } else {
            GlobalHotkeyStatus::Off
        };
    }

    fn wants_globals(&self) -> bool {
        self.committed.enabled && self.committed.dictate_outside_horizon
    }

    fn pause_and_unregister(&mut self) -> Result<(), (String, String)> {
        if let Some(bridge) = self.registrar.as_ref().map(|_| event_bridge()) {
            self.generation = bridge.pause_and_advance();
        }
        let result = if let Some(registrar) = self.registrar.as_mut() {
            unregister_with_sticky_failure(registrar, &mut self.active, &mut self.unregister_failure)
        } else {
            self.unregister_failure.clone().map_or(Ok(()), Err)
        };
        self.quarantine_inflight_presses();
        result
    }

    fn quarantine_inflight_presses(&mut self) {
        self.suppressed_until_release.extend(self.pressed.drain());
        while let Ok(event) = self.receiver.try_recv() {
            if event.pressed {
                self.suppressed_until_release.insert(event.id);
            } else {
                self.suppressed_until_release.remove(&event.id);
            }
        }
    }

    fn apply(&mut self, prompt: bool) {
        if let Err((binding, reason)) = self.pause_and_unregister() {
            self.status = GlobalHotkeyStatus::RegistrationConflict {
                binding,
                reason,
                retryable: false,
            };
            return;
        }
        let preflight = match registration_preflight(&self.committed, self.paused, || {
            if prompt {
                application_is_trusted_with_prompt()
            } else {
                application_is_trusted()
            }
        }) {
            Ok(preflight) => preflight,
            Err(status) => {
                self.status = status;
                return;
            }
        };
        let desired = match preflight {
            RegistrationPreflight::Off => {
                self.status = GlobalHotkeyStatus::Off;
                return;
            }
            RegistrationPreflight::Paused => {
                self.status = GlobalHotkeyStatus::PausedForRebinding;
                return;
            }
            RegistrationPreflight::Register(desired) => desired,
        };
        if self.registrar.is_none() {
            match GlobalHotKeyManager::new() {
                Ok(manager) => self.registrar = Some(MacRegistrar(manager)),
                Err(error) => {
                    self.status = GlobalHotkeyStatus::RegistrationConflict {
                        binding: desired
                            .first()
                            .map_or_else(|| "speech hotkeys".to_string(), |item| item.label.clone()),
                        reason: error.to_string(),
                        retryable: true,
                    };
                    return;
                }
            }
        }
        let bridge = event_bridge();
        bridge.attach(self.sender.clone(), self.context.clone());
        self.generation = bridge.pause_and_advance();
        let Some(registrar) = self.registrar.as_mut() else {
            self.status = GlobalHotkeyStatus::RegistrationConflict {
                binding: desired[0].label.clone(),
                reason: "macOS global-hotkey manager was unavailable".to_string(),
                retryable: true,
            };
            return;
        };
        match register_transaction(registrar, &desired) {
            Ok(active) => {
                let bindings = desired.iter().map(|binding| binding.label.clone()).collect();
                self.active = active;
                let active_ids: HashSet<u32> = self.active.iter().map(|binding| binding.id).collect();
                self.suppressed_until_release.retain(|id| active_ids.contains(id));
                quarantine_binding_ids(&self.active, &self.quarantined_keys, &mut self.suppressed_until_release);
                self.quarantined_keys.clear();
                self.status = GlobalHotkeyStatus::Registered { bindings };
                bridge.resume();
            }
            Err(RegistrationFailure {
                binding,
                reason,
                remaining,
            }) => {
                self.active = remaining;
                let (binding, reason) = if self.active.is_empty() {
                    (binding, reason)
                } else {
                    remember_sticky_unregister_failure(&mut self.unregister_failure, binding, &reason)
                };
                self.status = GlobalHotkeyStatus::RegistrationConflict {
                    binding,
                    reason,
                    retryable: self.active.is_empty(),
                };
            }
        }
    }
}

fn native_key_from_egui(key: egui::Key) -> Option<NativeKey> {
    Some(match key {
        egui::Key::ArrowDown => NativeKey::ArrowDown,
        egui::Key::ArrowLeft => NativeKey::ArrowLeft,
        egui::Key::ArrowRight => NativeKey::ArrowRight,
        egui::Key::ArrowUp => NativeKey::ArrowUp,
        egui::Key::Escape => NativeKey::Escape,
        egui::Key::Enter => NativeKey::Enter,
        egui::Key::Tab => NativeKey::Tab,
        egui::Key::Comma => NativeKey::Comma,
        egui::Key::Minus => NativeKey::Minus,
        egui::Key::Num0 => NativeKey::Digit(0),
        egui::Key::Num1 => NativeKey::Digit(1),
        egui::Key::Num2 => NativeKey::Digit(2),
        egui::Key::Num3 => NativeKey::Digit(3),
        egui::Key::Num4 => NativeKey::Digit(4),
        egui::Key::Num5 => NativeKey::Digit(5),
        egui::Key::Num6 => NativeKey::Digit(6),
        egui::Key::Num7 => NativeKey::Digit(7),
        egui::Key::Num8 => NativeKey::Digit(8),
        egui::Key::Num9 => NativeKey::Digit(9),
        egui::Key::A => NativeKey::Letter('A'),
        egui::Key::B => NativeKey::Letter('B'),
        egui::Key::C => NativeKey::Letter('C'),
        egui::Key::D => NativeKey::Letter('D'),
        egui::Key::E => NativeKey::Letter('E'),
        egui::Key::F => NativeKey::Letter('F'),
        egui::Key::G => NativeKey::Letter('G'),
        egui::Key::H => NativeKey::Letter('H'),
        egui::Key::I => NativeKey::Letter('I'),
        egui::Key::J => NativeKey::Letter('J'),
        egui::Key::K => NativeKey::Letter('K'),
        egui::Key::L => NativeKey::Letter('L'),
        egui::Key::M => NativeKey::Letter('M'),
        egui::Key::N => NativeKey::Letter('N'),
        egui::Key::O => NativeKey::Letter('O'),
        egui::Key::P => NativeKey::Letter('P'),
        egui::Key::Q => NativeKey::Letter('Q'),
        egui::Key::R => NativeKey::Letter('R'),
        egui::Key::S => NativeKey::Letter('S'),
        egui::Key::T => NativeKey::Letter('T'),
        egui::Key::U => NativeKey::Letter('U'),
        egui::Key::V => NativeKey::Letter('V'),
        egui::Key::W => NativeKey::Letter('W'),
        egui::Key::X => NativeKey::Letter('X'),
        egui::Key::Y => NativeKey::Letter('Y'),
        egui::Key::Z => NativeKey::Letter('Z'),
        egui::Key::F1 => NativeKey::Function(1),
        egui::Key::F2 => NativeKey::Function(2),
        egui::Key::F3 => NativeKey::Function(3),
        egui::Key::F4 => NativeKey::Function(4),
        egui::Key::F5 => NativeKey::Function(5),
        egui::Key::F6 => NativeKey::Function(6),
        egui::Key::F7 => NativeKey::Function(7),
        egui::Key::F8 => NativeKey::Function(8),
        egui::Key::F9 => NativeKey::Function(9),
        egui::Key::F10 => NativeKey::Function(10),
        egui::Key::F11 => NativeKey::Function(11),
        egui::Key::F12 => NativeKey::Function(12),
        egui::Key::F13 => NativeKey::Function(13),
        egui::Key::F14 => NativeKey::Function(14),
        egui::Key::F15 => NativeKey::Function(15),
        egui::Key::F16 => NativeKey::Function(16),
        egui::Key::F17 => NativeKey::Function(17),
        egui::Key::F18 => NativeKey::Function(18),
        egui::Key::F19 => NativeKey::Function(19),
        egui::Key::F20 => NativeKey::Function(20),
        _ => return None,
    })
}

impl Drop for PlatformGlobalHotkeys {
    fn drop(&mut self) {
        if let Err((binding, reason)) = self.pause_and_unregister() {
            tracing::warn!(%binding, %reason, "failed to unregister a macOS speech hotkey during shutdown");
        }
    }
}
