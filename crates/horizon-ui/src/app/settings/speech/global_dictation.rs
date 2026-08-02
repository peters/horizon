//! macOS focused-field dictation controls. The settings UI only renders a
//! cached runtime status and emits explicit actions; it never performs an
//! Accessibility trust check or opens a system permission prompt itself.

use egui::Ui;
use horizon_core::Config;

use crate::app::speech::global_hotkeys::{GlobalHotkeyAction, GlobalHotkeyStatus};
use crate::theme;

const CHECKBOX_LABEL: &str = "Use speech hotkeys outside Horizon";
#[cfg(not(target_os = "macos"))]
const UNSUPPORTED_PLATFORM_COPY: &str =
    "Focused-field dictation is available only on macOS. This setting remains valid in shared YAML configurations.";

pub(super) struct Response {
    pub changed: bool,
    pub action: Option<GlobalHotkeyAction>,
}

pub(super) fn render(
    ui: &mut Ui,
    config: &mut Config,
    cached_status: &GlobalHotkeyStatus,
    rebinding: bool,
) -> Response {
    let supported_platform = cfg!(target_os = "macos");
    let built_with_speech = crate::app::speech::built_with_speech();
    let speech = &mut config.features.speech;
    let speech_enabled = speech.enabled;
    let checkbox = egui::Checkbox::new(
        &mut speech.dictate_outside_horizon,
        egui::RichText::new(CHECKBOX_LABEL).color(theme::FG()).size(12.0),
    );
    let changed = ui
        .add_enabled(speech_enabled && supported_platform && built_with_speech, checkbox)
        .changed();

    #[cfg(not(target_os = "macos"))]
    if !supported_platform {
        super::super::dim_label(ui, UNSUPPORTED_PLATFORM_COPY);
        render_status(ui, &GlobalHotkeyStatus::UnsupportedPlatform);
        return Response { changed, action: None };
    }

    #[cfg(target_os = "macos")]
    if !built_with_speech {
        super::super::dim_label(
            ui,
            "A speech-enabled Horizon build is required before global hotkeys can be registered.",
        );
        render_status(ui, &GlobalHotkeyStatus::Off);
        return Response { changed, action: None };
    }

    super::super::dim_label(
        ui,
        "Configured speech keys are reserved system-wide while active. With F1/F2/F3 profiles, Firefox Help/Find will not receive F1/F3.",
    );
    super::super::dim_label(
        ui,
        "External dictation requires Microphone and Accessibility permission. Horizon does not request Input Monitoring.",
    );

    let effective_status = effective_status(rebinding, cached_status);
    render_status(ui, &effective_status);

    let (grant_accessibility, retry_registration) = actions_for_status(&effective_status);
    let mut action = None;
    ui.horizontal(|ui| {
        if grant_accessibility && ui.button("Grant Accessibility…").clicked() {
            action = Some(GlobalHotkeyAction::GrantAccessibility);
        }
        if retry_registration && ui.button("Retry").clicked() {
            action = Some(GlobalHotkeyAction::RetryRegistration);
        }
    });

    Response { changed, action }
}

fn render_status(ui: &mut Ui, status: &GlobalHotkeyStatus) {
    let (text, error) = status_text(status);
    let color = if error { theme::PALETTE_RED() } else { theme::FG_DIM() };
    ui.add(egui::Label::new(egui::RichText::new(text).color(color).size(11.0)).wrap());
}

fn status_text(status: &GlobalHotkeyStatus) -> (String, bool) {
    match status {
        GlobalHotkeyStatus::Off => ("Status: Off".to_string(), false),
        GlobalHotkeyStatus::AccessibilityRequired => ("Status: Accessibility required".to_string(), true),
        GlobalHotkeyStatus::Registered { bindings } => {
            let bindings = if bindings.is_empty() {
                "no configured bindings".to_string()
            } else {
                bindings.join(", ")
            };
            (format!("Status: Registered — {bindings}"), false)
        }
        GlobalHotkeyStatus::PausedForRebinding => ("Status: Temporarily paused for rebinding".to_string(), false),
        GlobalHotkeyStatus::UnsupportedBinding { binding, reason } => {
            (format!("Status: Unsupported binding — `{binding}`: {reason}"), true)
        }
        GlobalHotkeyStatus::RegistrationConflict { binding, reason, .. } => {
            (format!("Status: Registration conflict — `{binding}`: {reason}"), true)
        }
        #[cfg(not(all(feature = "speech", target_os = "macos")))]
        GlobalHotkeyStatus::UnsupportedPlatform => ("Status: Off — unsupported platform".to_string(), false),
    }
}

fn effective_status(rebinding: bool, cached_status: &GlobalHotkeyStatus) -> GlobalHotkeyStatus {
    if rebinding && !matches!(cached_status, GlobalHotkeyStatus::RegistrationConflict { .. }) {
        GlobalHotkeyStatus::PausedForRebinding
    } else {
        cached_status.clone()
    }
}

const fn actions_for_status(status: &GlobalHotkeyStatus) -> (bool, bool) {
    match status {
        GlobalHotkeyStatus::AccessibilityRequired => (true, true),
        GlobalHotkeyStatus::UnsupportedBinding { .. } => (false, true),
        GlobalHotkeyStatus::RegistrationConflict { retryable, .. } => (false, *retryable),
        GlobalHotkeyStatus::Off | GlobalHotkeyStatus::Registered { .. } | GlobalHotkeyStatus::PausedForRebinding => {
            (false, false)
        }
        #[cfg(not(all(feature = "speech", target_os = "macos")))]
        GlobalHotkeyStatus::UnsupportedPlatform => (false, false),
    }
}

#[cfg(test)]
mod tests {
    use crate::app::speech::global_hotkeys::GlobalHotkeyStatus;

    #[cfg(not(target_os = "macos"))]
    use super::{CHECKBOX_LABEL, UNSUPPORTED_PLATFORM_COPY};
    use super::{actions_for_status, effective_status, status_text};

    #[test]
    fn status_copy_names_bindings_and_runtime_failures() {
        assert_eq!(
            status_text(&GlobalHotkeyStatus::Registered {
                bindings: vec!["F1".to_string(), "F2".to_string(), "F3".to_string()],
            })
            .0,
            "Status: Registered — F1, F2, F3"
        );
        assert_eq!(
            status_text(&GlobalHotkeyStatus::UnsupportedBinding {
                binding: "Control+F35".to_string(),
                reason: "F35 is unavailable to Carbon".to_string(),
            })
            .0,
            "Status: Unsupported binding — `Control+F35`: F35 is unavailable to Carbon"
        );
        assert_eq!(
            status_text(&GlobalHotkeyStatus::RegistrationConflict {
                binding: "F3".to_string(),
                reason: "already registered by another application".to_string(),
                retryable: true,
            })
            .0,
            "Status: Registration conflict — `F3`: already registered by another application"
        );
    }

    #[test]
    fn permission_and_retry_actions_are_explicit() {
        assert_eq!(
            actions_for_status(&GlobalHotkeyStatus::AccessibilityRequired),
            (true, true)
        );
        assert_eq!(
            actions_for_status(&GlobalHotkeyStatus::RegistrationConflict {
                binding: "F1".to_string(),
                reason: "already registered".to_string(),
                retryable: true,
            }),
            (false, true)
        );
        assert_eq!(actions_for_status(&GlobalHotkeyStatus::Off), (false, false));
        assert_eq!(
            actions_for_status(&GlobalHotkeyStatus::RegistrationConflict {
                binding: "F1".to_string(),
                reason: "restart Horizon to release the affected system key".to_string(),
                retryable: false,
            }),
            (false, false)
        );
        assert_eq!(
            actions_for_status(&GlobalHotkeyStatus::PausedForRebinding),
            (false, false)
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_platform_copy_is_explicit_and_keeps_yaml_portable() {
        assert_eq!(CHECKBOX_LABEL, "Use speech hotkeys outside Horizon");
        assert!(UNSUPPORTED_PLATFORM_COPY.contains("only on macOS"));
        assert!(UNSUPPORTED_PLATFORM_COPY.contains("shared YAML"));
        assert_eq!(
            status_text(&GlobalHotkeyStatus::UnsupportedPlatform).0,
            "Status: Off — unsupported platform"
        );
    }

    #[test]
    fn unsaved_disable_keeps_cached_registration_until_save() {
        let registered = GlobalHotkeyStatus::Registered {
            bindings: vec!["F1".to_string()],
        };
        // The checkbox edits a draft. Until Save reconfigures the manager,
        // its cached Registered state remains the truthful runtime status.
        assert_eq!(effective_status(false, &registered), registered);
    }

    #[test]
    fn rebinding_pause_overrides_cached_registration() {
        let registered = GlobalHotkeyStatus::Registered {
            bindings: vec!["F1".to_string()],
        };
        assert!(matches!(
            effective_status(true, &registered),
            GlobalHotkeyStatus::PausedForRebinding
        ));

        let conflict = GlobalHotkeyStatus::RegistrationConflict {
            binding: "F1".to_string(),
            reason: "failed to unregister".to_string(),
            retryable: false,
        };
        assert_eq!(effective_status(true, &conflict), conflict);
    }
}
