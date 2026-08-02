use std::collections::{HashMap, HashSet};

use horizon_core::{ShortcutModifiers, SpeechProfile};

use super::*;

#[test]
fn maps_function_keys_and_macos_modifiers() {
    let binding = ShortcutBinding::new(
        ShortcutModifiers::PRIMARY
            .plus(ShortcutModifiers::CTRL)
            .plus(ShortcutModifiers::ALT)
            .plus(ShortcutModifiers::SHIFT),
        ShortcutKey::Function(3),
    );
    assert_eq!(
        native_hotkey(binding),
        Ok(NativeHotkey {
            modifiers: NativeModifiers(
                NativeModifiers::SUPER | NativeModifiers::CONTROL | NativeModifiers::ALT | NativeModifiers::SHIFT,
            ),
            key: NativeKey::Function(3),
        })
    );
    let unsupported = ShortcutBinding::new(ShortcutModifiers::NONE, ShortcutKey::Function(21));
    assert!(native_hotkey(unsupported).is_err());
    let shifted_symbol = ShortcutBinding::new(ShortcutModifiers::NONE, ShortcutKey::Plus);
    assert!(native_hotkey(shifted_symbol).is_err());
}

#[test]
fn permission_preflight_is_explicit_fail_closed_and_skipped_when_inactive() {
    use std::cell::Cell;

    let config = SpeechConfig {
        enabled: true,
        dictate_outside_horizon: true,
        profiles: vec![SpeechProfile {
            name: "one".to_string(),
            hotkey: "F1".to_string(),
            ..SpeechProfile::default()
        }],
        ..SpeechConfig::default()
    };
    let checks = Cell::new(0);
    let denied = registration_preflight(&config, false, || {
        checks.set(checks.get() + 1);
        false
    });
    assert_eq!(denied, Err(GlobalHotkeyStatus::AccessibilityRequired));
    assert_eq!(checks.get(), 1);

    let registered = registration_preflight(&config, false, || true).expect("trusted preflight");
    assert!(matches!(registered, RegistrationPreflight::Register(bindings) if bindings.len() == 1));
    assert_eq!(
        registration_preflight(&config, false, || false),
        Err(GlobalHotkeyStatus::AccessibilityRequired),
        "revocation must fail closed on the next active preflight"
    );

    let mut off = config.clone();
    off.dictate_outside_horizon = false;
    assert_eq!(
        registration_preflight(&off, false, || panic!("off must not check permission")),
        Ok(RegistrationPreflight::Off)
    );
    assert_eq!(
        registration_preflight(&config, true, || panic!("paused must not check permission")),
        Ok(RegistrationPreflight::Paused)
    );
}

#[test]
fn filters_stale_repeated_and_mismatched_events() {
    let profiles = HashMap::from([(11, 0), (22, 1)]);
    let mut held = HashSet::new();
    let mut suppressed = HashSet::new();

    assert_eq!(
        accept_event(7, 6, 11, true, &profiles, &mut held, &mut suppressed),
        None
    );
    assert_eq!(
        accept_event(7, 7, 11, true, &profiles, &mut held, &mut suppressed),
        Some(GlobalHotkeyEvent {
            generation: 7,
            profile: 0,
            pressed: true,
        })
    );
    assert_eq!(
        accept_event(7, 7, 11, true, &profiles, &mut held, &mut suppressed),
        None
    );
    assert_eq!(
        accept_event(7, 7, 22, true, &profiles, &mut held, &mut suppressed),
        Some(GlobalHotkeyEvent {
            generation: 7,
            profile: 1,
            pressed: true,
        })
    );
    assert_eq!(
        accept_event(7, 7, 33, false, &profiles, &mut held, &mut suppressed),
        None
    );
    assert_eq!(
        accept_event(7, 7, 11, false, &profiles, &mut held, &mut suppressed),
        Some(GlobalHotkeyEvent {
            generation: 7,
            profile: 0,
            pressed: false,
        })
    );
    assert_eq!(
        accept_event(7, 7, 11, false, &profiles, &mut held, &mut suppressed),
        None
    );
    assert!(held.contains(&22));
}

#[test]
fn generation_change_clears_press_ownership() {
    let profiles = HashMap::from([(11, 0)]);
    let mut held = HashSet::new();
    let mut suppressed = HashSet::new();
    assert!(accept_event(3, 3, 11, true, &profiles, &mut held, &mut suppressed).is_some());

    suppressed.extend(held.drain());
    assert_eq!(
        accept_event(4, 3, 11, false, &profiles, &mut held, &mut suppressed),
        None
    );
    assert_eq!(
        accept_event(4, 4, 11, true, &profiles, &mut held, &mut suppressed),
        None
    );
    assert_eq!(
        accept_event(4, 4, 11, false, &profiles, &mut held, &mut suppressed),
        None
    );
    assert!(suppressed.is_empty());
    assert!(accept_event(4, 4, 11, true, &profiles, &mut held, &mut suppressed).is_some());
}

#[test]
fn timed_out_capture_quarantines_matching_native_ids_until_release() {
    let config = SpeechConfig {
        enabled: true,
        dictate_outside_horizon: true,
        profiles: vec![
            SpeechProfile {
                name: "one".to_string(),
                hotkey: "F1".to_string(),
                ..SpeechProfile::default()
            },
            SpeechProfile {
                name: "two".to_string(),
                hotkey: "F2".to_string(),
                ..SpeechProfile::default()
            },
        ],
        ..SpeechConfig::default()
    };
    let mut registrar = FakeRegistrar::default();
    let active =
        register_transaction(&mut registrar, &desired_bindings(&config).expect("bindings")).expect("registration");
    let profiles = active
        .iter()
        .map(|binding| (binding.id, binding.desired.profile))
        .collect::<HashMap<_, _>>();
    let f1_id = active[0].id;
    let f2_id = active[1].id;
    let mut held = HashSet::new();
    let mut suppressed = HashSet::new();

    quarantine_binding_ids(&active, &HashSet::from([NativeKey::Function(1)]), &mut suppressed);
    assert_eq!(suppressed, HashSet::from([f1_id]));
    assert!(accept_event(3, 3, f1_id, true, &profiles, &mut held, &mut suppressed).is_none());
    assert!(accept_event(3, 3, f1_id, false, &profiles, &mut held, &mut suppressed).is_none());
    assert!(accept_event(3, 3, f1_id, true, &profiles, &mut held, &mut suppressed).is_some());
    assert!(accept_event(3, 3, f2_id, true, &profiles, &mut held, &mut suppressed).is_some());
}

#[derive(Default)]
struct FakeRegistrar {
    registered: HashSet<NativeHotkey>,
    fail_on: Option<NativeHotkey>,
    fail_unregister_on: Option<NativeHotkey>,
    unregister_attempts: usize,
}

impl RegistrationBackend for FakeRegistrar {
    type Error = &'static str;

    fn register(&mut self, hotkey: NativeHotkey) -> Result<u32, Self::Error> {
        if self.fail_on == Some(hotkey) {
            return Err("occupied");
        }
        self.registered.insert(hotkey);
        u32::try_from(self.registered.len()).map_err(|_| "too many registered bindings")
    }

    fn unregister(&mut self, hotkey: NativeHotkey) -> Result<(), Self::Error> {
        self.unregister_attempts += 1;
        if self.fail_unregister_on == Some(hotkey) {
            return Err("unregister failed");
        }
        self.registered.remove(&hotkey);
        Ok(())
    }
}

#[test]
fn one_conflict_rolls_back_the_whole_set() {
    let mut config = SpeechConfig {
        enabled: true,
        dictate_outside_horizon: true,
        profiles: vec![
            SpeechProfile {
                name: "one".to_string(),
                hotkey: "F1".to_string(),
                ..SpeechProfile::default()
            },
            SpeechProfile {
                name: "two".to_string(),
                hotkey: "F2".to_string(),
                ..SpeechProfile::default()
            },
        ],
        ..SpeechConfig::default()
    };
    let desired = desired_bindings(&config).expect("bindings");
    let mut registrar = FakeRegistrar {
        fail_on: Some(desired[1].hotkey),
        ..FakeRegistrar::default()
    };
    let failure = register_transaction(&mut registrar, &desired).expect_err("second binding conflicts");
    assert_eq!(failure.binding, "F2");
    assert!(failure.remaining.is_empty());
    assert!(registrar.registered.is_empty());

    config.profiles[1].hotkey = "F21".to_string();
    assert!(matches!(
        desired_bindings(&config),
        Err(GlobalHotkeyStatus::UnsupportedBinding { binding, .. }) if binding == "F21"
    ));
}

#[test]
fn unregister_and_rollback_failures_name_the_binding() {
    let config = SpeechConfig {
        enabled: true,
        dictate_outside_horizon: true,
        profiles: vec![
            SpeechProfile {
                name: "one".to_string(),
                hotkey: "F1".to_string(),
                ..SpeechProfile::default()
            },
            SpeechProfile {
                name: "two".to_string(),
                hotkey: "F2".to_string(),
                ..SpeechProfile::default()
            },
        ],
        ..SpeechConfig::default()
    };
    let desired = desired_bindings(&config).expect("bindings");
    let mut registrar = FakeRegistrar {
        fail_on: Some(desired[1].hotkey),
        fail_unregister_on: Some(desired[0].hotkey),
        ..FakeRegistrar::default()
    };

    let failure = register_transaction(&mut registrar, &desired).expect_err("rollback must surface its failure");
    assert_eq!(failure.binding, "F1");
    assert!(failure.reason.contains("registering `F2` failed"));
    assert!(failure.reason.contains("rollback also failed"));
    assert_eq!(failure.remaining.len(), 1);
    assert!(registrar.registered.contains(&desired[0].hotkey));
}

#[test]
fn unregister_failure_retains_the_binding_for_honest_cleanup_state() {
    let config = SpeechConfig {
        enabled: true,
        dictate_outside_horizon: true,
        profiles: vec![SpeechProfile {
            name: "one".to_string(),
            hotkey: "F1".to_string(),
            ..SpeechProfile::default()
        }],
        ..SpeechConfig::default()
    };
    let desired = desired_bindings(&config).expect("bindings");
    let mut registrar = FakeRegistrar::default();
    let mut active = register_transaction(&mut registrar, &desired).expect("registration");
    registrar.fail_unregister_on = Some(desired[0].hotkey);

    let failure = unregister_transaction(&mut registrar, &mut active).expect_err("unregister failure");
    assert_eq!(failure.0, "F1");
    assert_eq!(active.len(), 1);
    assert!(registrar.registered.contains(&desired[0].hotkey));

    registrar.fail_unregister_on = None;
    unregister_transaction(&mut registrar, &mut active).expect("retry cleanup");
    assert!(active.is_empty());
    assert!(registrar.registered.is_empty());
}

#[test]
fn unregister_failure_taints_manager_and_blocks_unsafe_retries() {
    let config = SpeechConfig {
        enabled: true,
        dictate_outside_horizon: true,
        profiles: vec![SpeechProfile {
            name: "one".to_string(),
            hotkey: "F1".to_string(),
            ..SpeechProfile::default()
        }],
        ..SpeechConfig::default()
    };
    let desired = desired_bindings(&config).expect("bindings");
    let mut registrar = FakeRegistrar::default();
    let mut active = register_transaction(&mut registrar, &desired).expect("registration");
    let mut sticky_failure = None;
    registrar.fail_unregister_on = Some(desired[0].hotkey);

    let first = unregister_with_sticky_failure(&mut registrar, &mut active, &mut sticky_failure)
        .expect_err("first unregister failure must taint the manager");
    assert_eq!(first.0, "F1");
    assert!(first.1.contains("restart Horizon"));
    assert_eq!(sticky_failure, Some(first.clone()));
    assert_eq!(registrar.unregister_attempts, 1);
    assert_eq!(active.len(), 1);

    // Retry, reconfigure, and shutdown all enter through the same manager
    // cleanup guard. Even if the backend would now report success, none may
    // touch the ambiguous Carbon handle again in this process.
    registrar.fail_unregister_on = None;
    for _ in 0..3 {
        assert_eq!(
            unregister_with_sticky_failure(&mut registrar, &mut active, &mut sticky_failure),
            Err(first.clone())
        );
    }
    assert_eq!(registrar.unregister_attempts, 1);
    assert_eq!(active.len(), 1);
    assert!(registrar.registered.contains(&desired[0].hotkey));
}

#[test]
fn registration_adapter_reconfigures_and_unregisters_the_complete_set() {
    let mut config = SpeechConfig {
        enabled: true,
        dictate_outside_horizon: true,
        profiles: vec![
            SpeechProfile {
                name: "one".to_string(),
                hotkey: "F1".to_string(),
                ..SpeechProfile::default()
            },
            SpeechProfile {
                name: "two".to_string(),
                hotkey: "F2".to_string(),
                ..SpeechProfile::default()
            },
        ],
        ..SpeechConfig::default()
    };
    let mut registrar = FakeRegistrar::default();
    let mut active = register_transaction(&mut registrar, &desired_bindings(&config).expect("initial bindings"))
        .expect("initial registration");
    assert_eq!(registrar.registered.len(), 2);

    unregister_transaction(&mut registrar, &mut active).expect("unregister complete set");
    assert!(active.is_empty());
    assert!(registrar.registered.is_empty());

    config.profiles = vec![SpeechProfile {
        name: "replacement".to_string(),
        hotkey: "F3".to_string(),
        ..SpeechProfile::default()
    }];
    active = register_transaction(&mut registrar, &desired_bindings(&config).expect("replacement binding"))
        .expect("replacement registration");
    assert_eq!(active.len(), 1);
    assert_eq!(registrar.registered.len(), 1);

    unregister_transaction(&mut registrar, &mut active).expect("shutdown unregister");
    assert!(registrar.registered.is_empty());
}

#[cfg(not(all(feature = "speech", target_os = "macos")))]
#[test]
fn non_macos_stub_is_inert_and_reports_platform() {
    let config = SpeechConfig {
        enabled: true,
        dictate_outside_horizon: true,
        ..SpeechConfig::default()
    };
    let context = egui::Context::default();
    let mut globals = GlobalHotkeys::new(&context, &config);
    assert!(!globals.is_registered());
    assert_eq!(globals.status(), &GlobalHotkeyStatus::UnsupportedPlatform);
    assert!(globals.drain_events().is_empty());
    globals.set_rebinding_paused(true);
    assert_eq!(globals.status(), &GlobalHotkeyStatus::PausedForRebinding);
}
