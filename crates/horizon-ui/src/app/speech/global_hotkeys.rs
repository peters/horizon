//! Transactional system-wide speech-hotkey registration.
//!
//! The platform event handler is process-global and deliberately tiny: it
//! tags native press/release events with the current registration generation,
//! queues them, and asks egui for a repaint. Routing remains on the UI thread.

#![cfg_attr(not(all(feature = "speech", target_os = "macos")), allow(dead_code))]

use horizon_core::SpeechConfig;
#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
use horizon_core::{ShortcutBinding, ShortcutKey};

/// Cached settings state. Reading this enum never performs an AX call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GlobalHotkeyStatus {
    Off,
    AccessibilityRequired,
    Registered {
        bindings: Vec<String>,
    },
    PausedForRebinding,
    UnsupportedBinding {
        binding: String,
        reason: String,
    },
    RegistrationConflict {
        binding: String,
        reason: String,
        retryable: bool,
    },
    #[cfg(not(all(feature = "speech", target_os = "macos")))]
    UnsupportedPlatform,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlobalHotkeyAction {
    GrantAccessibility,
    RetryRegistration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GlobalHotkeyEvent {
    pub generation: u64,
    pub profile: usize,
    pub pressed: bool,
}

/// Cross-platform shell; the non-macOS implementation is inert.
pub struct GlobalHotkeys {
    inner: PlatformGlobalHotkeys,
    #[cfg(test)]
    injected_events: Vec<GlobalHotkeyEvent>,
}

impl GlobalHotkeys {
    #[must_use]
    pub fn new(ctx: &egui::Context, config: &SpeechConfig) -> Self {
        Self {
            inner: PlatformGlobalHotkeys::new(ctx, config),
            #[cfg(test)]
            injected_events: Vec::new(),
        }
    }

    #[must_use]
    pub fn status(&self) -> &GlobalHotkeyStatus {
        self.inner.status()
    }

    #[must_use]
    pub fn is_registered(&self) -> bool {
        self.inner.is_registered()
    }

    pub fn drain_events(&mut self) -> Vec<GlobalHotkeyEvent> {
        #[cfg(test)]
        {
            let mut events = self.inner.drain_events();
            events.splice(0..0, std::mem::take(&mut self.injected_events));
            events
        }
        #[cfg(not(test))]
        {
            self.inner.drain_events()
        }
    }

    #[cfg(test)]
    pub(in crate::app) fn inject_event(&mut self, event: GlobalHotkeyEvent) {
        self.injected_events.push(event);
    }

    /// Forget any key presses owned by the previous speech/session state.
    /// Registered bindings stay installed, but events already in flight are
    /// invalidated before acceptance resumes.
    pub fn clear_event_ownership(&mut self) {
        self.inner.clear_event_ownership();
    }

    /// Apply a saved/file-reloaded configuration. Settings live preview must
    /// not call this; the previous committed set stays active until Save.
    pub fn reconfigure_committed(&mut self, config: &SpeechConfig) {
        self.inner.reconfigure_committed(config);
    }

    pub fn set_rebinding_paused(&mut self, paused: bool) {
        self.inner.set_rebinding_paused(paused);
    }

    /// A rebind release was lost while globals were unregistered. If the
    /// captured key belongs to the committed set, suppress native repeats and
    /// the next press until Carbon observes a release.
    pub fn quarantine_captured_key(&mut self, key: egui::Key, physical_key: Option<egui::Key>) {
        self.inner.quarantine_captured_key(key, physical_key);
    }

    /// The only API that may display Apple's Accessibility prompt.
    pub fn grant_accessibility_and_retry(&mut self, config: &SpeechConfig) {
        self.inner.grant_accessibility_and_retry(config);
    }

    /// Recheck permission and registration without displaying a prompt.
    pub fn retry(&mut self, config: &SpeechConfig) {
        self.inner.retry(config);
    }

    #[must_use]
    pub fn permission_is_current(&self) -> bool {
        self.inner.permission_is_current()
    }

    pub fn unregister_all(&mut self) {
        self.inner.unregister_all();
    }
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct NativeModifiers(u8);

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
impl NativeModifiers {
    const ALT: u8 = 1 << 0;
    const CONTROL: u8 = 1 << 1;
    const SHIFT: u8 = 1 << 2;
    const SUPER: u8 = 1 << 3;
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum NativeKey {
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Escape,
    Enter,
    Tab,
    Comma,
    Minus,
    Digit(u8),
    Letter(char),
    Function(u8),
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct NativeHotkey {
    modifiers: NativeModifiers,
    key: NativeKey,
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct DesiredBinding {
    profile: usize,
    label: String,
    hotkey: NativeHotkey,
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
#[derive(Clone, Debug, Eq, PartialEq)]
enum RegistrationPreflight {
    Off,
    Paused,
    Register(Vec<DesiredBinding>),
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
fn native_hotkey(binding: ShortcutBinding) -> Result<NativeHotkey, String> {
    let modifiers = binding.modifiers;
    let mut bits = 0;
    if modifiers.alt() {
        bits |= NativeModifiers::ALT;
    }
    if modifiers.ctrl() {
        bits |= NativeModifiers::CONTROL;
    }
    if modifiers.shift() {
        bits |= NativeModifiers::SHIFT;
    }
    if modifiers.command() || modifiers.mac_cmd() {
        bits |= NativeModifiers::SUPER;
    }
    let key = match binding.key {
        ShortcutKey::ArrowDown => NativeKey::ArrowDown,
        ShortcutKey::ArrowLeft => NativeKey::ArrowLeft,
        ShortcutKey::ArrowRight => NativeKey::ArrowRight,
        ShortcutKey::ArrowUp => NativeKey::ArrowUp,
        ShortcutKey::Escape => NativeKey::Escape,
        ShortcutKey::Enter => NativeKey::Enter,
        ShortcutKey::Tab => NativeKey::Tab,
        ShortcutKey::Comma => NativeKey::Comma,
        ShortcutKey::Minus => NativeKey::Minus,
        ShortcutKey::Plus => {
            return Err(
                "Plus is not supported by macOS global hotkeys; use a physical key with explicit modifiers".to_string(),
            );
        }
        ShortcutKey::Digit(digit @ 0..=9) => NativeKey::Digit(digit),
        ShortcutKey::Letter(letter @ 'A'..='Z') => NativeKey::Letter(letter),
        // global-hotkey 0.8.0's macOS Carbon table has F1 through F20.
        ShortcutKey::Function(function @ 1..=20) => NativeKey::Function(function),
        unsupported => {
            return Err(format!("{unsupported:?} is not supported by macOS global hotkeys"));
        }
    };
    Ok(NativeHotkey {
        modifiers: NativeModifiers(bits),
        key,
    })
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
fn desired_bindings(config: &SpeechConfig) -> Result<Vec<DesiredBinding>, GlobalHotkeyStatus> {
    let mut desired = Vec::new();
    for (profile, configured) in config.resolved_profiles().iter().enumerate() {
        let label = configured.hotkey.trim();
        if label.is_empty() {
            continue;
        }
        let binding = ShortcutBinding::parse(label).map_err(|error| GlobalHotkeyStatus::UnsupportedBinding {
            binding: label.to_string(),
            reason: error.to_string(),
        })?;
        let hotkey = native_hotkey(binding).map_err(|reason| GlobalHotkeyStatus::UnsupportedBinding {
            binding: label.to_string(),
            reason,
        })?;
        if desired
            .iter()
            .any(|existing: &DesiredBinding| existing.hotkey == hotkey)
        {
            return Err(GlobalHotkeyStatus::UnsupportedBinding {
                binding: label.to_string(),
                reason: "binding resolves to the same macOS key as another speech profile".to_string(),
            });
        }
        desired.push(DesiredBinding {
            profile,
            label: label.to_string(),
            hotkey,
        });
    }
    Ok(desired)
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
fn registration_preflight(
    config: &SpeechConfig,
    paused: bool,
    trust_check: impl FnOnce() -> bool,
) -> Result<RegistrationPreflight, GlobalHotkeyStatus> {
    if paused {
        return Ok(RegistrationPreflight::Paused);
    }
    if !config.enabled || !config.dictate_outside_horizon {
        return Ok(RegistrationPreflight::Off);
    }
    let desired = desired_bindings(config)?;
    if desired.is_empty() {
        return Ok(RegistrationPreflight::Off);
    }
    if !trust_check() {
        return Err(GlobalHotkeyStatus::AccessibilityRequired);
    }
    Ok(RegistrationPreflight::Register(desired))
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
trait RegistrationBackend {
    type Error: std::fmt::Display;

    fn register(&mut self, hotkey: NativeHotkey) -> Result<u32, Self::Error>;
    fn unregister(&mut self, hotkey: NativeHotkey) -> Result<(), Self::Error>;
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
#[derive(Clone, Debug)]
struct ActiveBinding {
    desired: DesiredBinding,
    id: u32,
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
#[derive(Debug)]
struct RegistrationFailure {
    binding: String,
    reason: String,
    remaining: Vec<ActiveBinding>,
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
fn register_transaction<B: RegistrationBackend>(
    backend: &mut B,
    desired: &[DesiredBinding],
) -> Result<Vec<ActiveBinding>, RegistrationFailure> {
    let mut active: Vec<ActiveBinding> = Vec::new();
    for binding in desired {
        match backend.register(binding.hotkey) {
            Ok(id) => active.push(ActiveBinding {
                desired: binding.clone(),
                id,
            }),
            Err(error) => {
                let failed_binding = binding.label.clone();
                let failed_reason = error.to_string();
                if let Err((rollback_binding, rollback_reason)) = unregister_transaction(backend, &mut active) {
                    return Err(RegistrationFailure {
                        binding: rollback_binding,
                        reason: format!(
                            "registering `{failed_binding}` failed: {failed_reason}; rollback also failed: {rollback_reason}"
                        ),
                        remaining: active,
                    });
                }
                return Err(RegistrationFailure {
                    binding: failed_binding,
                    reason: failed_reason,
                    remaining: active,
                });
            }
        }
    }
    Ok(active)
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
fn unregister_transaction<B: RegistrationBackend>(
    backend: &mut B,
    active: &mut Vec<ActiveBinding>,
) -> Result<(), (String, String)> {
    let mut first_failure = None;
    let mut remaining = Vec::new();
    for registered in std::mem::take(active).into_iter().rev() {
        if let Err(error) = backend.unregister(registered.desired.hotkey) {
            if first_failure.is_none() {
                first_failure = Some((registered.desired.label.clone(), error.to_string()));
            }
            remaining.push(registered);
        }
    }
    remaining.reverse();
    *active = remaining;
    first_failure.map_or(Ok(()), Err)
}

/// Carbon removes its internal handle before reporting an unregister error,
/// so retrying cannot prove that the system key was released. Taint the
/// manager after the first failure and keep subsequent cleanup attempts from
/// touching the backend until the process restarts.
#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
fn unregister_with_sticky_failure<B: RegistrationBackend>(
    backend: &mut B,
    active: &mut Vec<ActiveBinding>,
    sticky_failure: &mut Option<(String, String)>,
) -> Result<(), (String, String)> {
    if let Some(failure) = sticky_failure {
        return Err(failure.clone());
    }
    unregister_transaction(backend, active)
        .map_err(|(binding, reason)| remember_sticky_unregister_failure(sticky_failure, binding, &reason))
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
fn remember_sticky_unregister_failure(
    sticky_failure: &mut Option<(String, String)>,
    binding: String,
    reason: &str,
) -> (String, String) {
    let failure = (
        binding,
        format!("{reason}; restart Horizon to release the affected system key"),
    );
    *sticky_failure = Some(failure.clone());
    failure
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
fn accept_event(
    current_generation: u64,
    event_generation: u64,
    id: u32,
    pressed: bool,
    profiles: &std::collections::HashMap<u32, usize>,
    held: &mut std::collections::HashSet<u32>,
    suppressed_until_release: &mut std::collections::HashSet<u32>,
) -> Option<GlobalHotkeyEvent> {
    if event_generation != current_generation {
        return None;
    }
    if suppressed_until_release.contains(&id) {
        if !pressed {
            suppressed_until_release.remove(&id);
        }
        return None;
    }
    let profile = profiles.get(&id).copied()?;
    let accepted = if pressed { held.insert(id) } else { held.remove(&id) };
    accepted.then_some(GlobalHotkeyEvent {
        generation: event_generation,
        profile,
        pressed,
    })
}

#[cfg(any(all(feature = "speech", target_os = "macos"), test))]
fn quarantine_binding_ids(
    active: &[ActiveBinding],
    keys: &std::collections::HashSet<NativeKey>,
    suppressed_until_release: &mut std::collections::HashSet<u32>,
) {
    suppressed_until_release.extend(
        active
            .iter()
            .filter(|binding| keys.contains(&binding.desired.hotkey.key))
            .map(|binding| binding.id),
    );
}

#[cfg(all(feature = "speech", target_os = "macos"))]
#[path = "global_hotkeys/macos.rs"]
mod macos;
#[cfg(all(feature = "speech", target_os = "macos"))]
use macos::PlatformGlobalHotkeys;

#[cfg(not(all(feature = "speech", target_os = "macos")))]
struct PlatformGlobalHotkeys {
    committed: SpeechConfig,
    status: GlobalHotkeyStatus,
    paused: bool,
}

#[cfg(not(all(feature = "speech", target_os = "macos")))]
impl PlatformGlobalHotkeys {
    fn new(_ctx: &egui::Context, config: &SpeechConfig) -> Self {
        let mut globals = Self {
            committed: config.clone(),
            status: GlobalHotkeyStatus::Off,
            paused: false,
        };
        globals.refresh();
        globals
    }

    const fn status(&self) -> &GlobalHotkeyStatus {
        &self.status
    }

    const fn is_registered(&self) -> bool {
        let _ = self;
        false
    }

    fn drain_events(&mut self) -> Vec<GlobalHotkeyEvent> {
        let _ = self;
        Vec::new()
    }

    fn clear_event_ownership(&mut self) {
        let _ = self;
    }

    fn reconfigure_committed(&mut self, config: &SpeechConfig) {
        self.committed = config.clone();
        self.refresh();
    }

    fn set_rebinding_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.refresh();
    }

    fn quarantine_captured_key(&mut self, key: egui::Key, physical_key: Option<egui::Key>) {
        let _ = (self, key, physical_key);
    }

    fn grant_accessibility_and_retry(&mut self, config: &SpeechConfig) {
        self.reconfigure_committed(config);
    }

    fn retry(&mut self, config: &SpeechConfig) {
        self.reconfigure_committed(config);
    }

    const fn permission_is_current(&self) -> bool {
        let _ = self;
        false
    }

    fn unregister_all(&mut self) {
        self.status = GlobalHotkeyStatus::Off;
    }

    fn refresh(&mut self) {
        self.status = if !self.committed.enabled || !self.committed.dictate_outside_horizon {
            GlobalHotkeyStatus::Off
        } else if self.paused {
            GlobalHotkeyStatus::PausedForRebinding
        } else {
            GlobalHotkeyStatus::UnsupportedPlatform
        };
    }
}

#[cfg(test)]
#[path = "global_hotkeys/tests.rs"]
mod tests;
