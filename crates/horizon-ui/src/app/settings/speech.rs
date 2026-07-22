//! Speech Input settings section: profile summary, model metadata, source
//! language / output pickers, backend gating, and the press-to-bind hotkey
//! capture flow. Split from `general.rs` per the module-size guardrail.

use egui::Ui;
use horizon_core::speech_model::{SpeechModelInfo, read_speech_model_info};
use horizon_core::{Config, SpeechBackend, SpeechHotkeyMode, SpeechTask};

use crate::app::shortcuts::{is_clipboard_pseudo_event, mark_captured_clipboard_event};
use crate::app::speech::built_with_speech;

const CLIPBOARD_HOTKEY_ERROR: &str =
    "Clipboard shortcuts (Ctrl/Cmd+C, X, or V) are reserved and cannot be used as speech hotkeys";

/// A chord the binder just captured, whose key may still be held. The
/// terminal filter suppresses its repeats/release/text until the release is
/// observed — tracked by physical key because a shifted keycap's logical key
/// differs between press and release.
#[derive(Clone, Copy)]
pub(in crate::app) struct PendingCapture {
    pub key: egui::Key,
    pub physical_key: Option<egui::Key>,
    pub shifted: bool,
    pub clipboard: Option<ClipboardCapture>,
    /// egui input time when armed; the filter drops it after a short window
    /// so a never-delivered release cannot wedge input permanently.
    pub armed_at: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) enum ClipboardCapture {
    Copy,
    Cut,
    Paste,
}

impl ClipboardCapture {
    const fn fallback_key(self) -> egui::Key {
        match self {
            Self::Copy => egui::Key::C,
            Self::Cut => egui::Key::X,
            Self::Paste => egui::Key::V,
        }
    }

    pub(in crate::app) fn matches_release_key(self, key: egui::Key) -> bool {
        match self {
            Self::Copy => matches!(key, egui::Key::C | egui::Key::Insert | egui::Key::Copy),
            Self::Cut => matches!(key, egui::Key::X | egui::Key::Delete | egui::Key::Cut),
            Self::Paste => matches!(key, egui::Key::V | egui::Key::Insert | egui::Key::Paste),
        }
    }
}

enum HotkeyCaptureAttempt {
    Key {
        key: egui::Key,
        physical_key: Option<egui::Key>,
        modifiers: egui::Modifiers,
    },
    ClipboardReserved(ClipboardCapture),
}
use crate::app::util::primary_shortcut_label;
use crate::theme;

pub(super) fn render(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;

    changed |= ui
        .checkbox(
            &mut config.features.speech.enabled,
            egui::RichText::new("Speech Input").color(theme::FG()).size(12.0),
        )
        .changed();
    if built_with_speech() {
        super::dim_label(
            ui,
            "Dictate into the focused panel with the mic button or the push-to-talk hotkey. Changes apply live on save.",
        );
    } else {
        super::dim_label(
            ui,
            "This build has no speech support. Rebuild with: cargo run --release --features speech",
        );
    }

    if !config.features.speech.enabled {
        return changed;
    }

    // With named profiles, the flat single-model editor below would
    // mislead; show the profile summary and defer editing to the YAML tab
    // (which live-previews and validates on save).
    if !config.features.speech.profiles.is_empty() {
        ui.add_space(6.0);
        egui::Grid::new("settings_speech_profiles_grid")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for profile in &config.features.speech.profiles {
                    let key = if profile.hotkey.trim().is_empty() {
                        "(mic button)".to_string()
                    } else {
                        profile.hotkey.clone()
                    };
                    ui.label(egui::RichText::new(key).color(theme::FG()).size(12.0).monospace());
                    let model_name = std::path::Path::new(&profile.model)
                        .file_name()
                        .map_or_else(|| profile.model.clone(), |name| name.to_string_lossy().into_owned());
                    let output = match profile.task {
                        SpeechTask::Transcribe => profile.language.clone(),
                        SpeechTask::Translate => format!("{} → {}", profile.language, profile.target_language),
                    };
                    super::dim_label(ui, &format!("{} — {output} · {model_name}", profile.name));
                    ui.end_row();
                }
            });
        super::dim_label(
            ui,
            "Speech profiles are edited in the YAML tab (features.speech.profiles).",
        );
        return changed;
    }

    let model_path = config.features.speech.model.clone();
    let model_info = cached_speech_model_info(ui, &model_path);

    ui.add_space(6.0);
    egui::Grid::new("settings_speech_grid")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            changed |= speech_model_rows(ui, config, model_info.as_ref());
            changed |= speech_language_row(ui, config, model_info.as_ref());
            changed |= speech_output_row(ui, config, model_info.as_ref());
            changed |= speech_backend_row(ui, config);

            ui.label(egui::RichText::new("Push-to-talk").color(theme::FG_SOFT()).size(12.0));
            changed |= render_hotkey_binder(ui, config);
            ui.end_row();

            // `config` is reborrowed by the hotkey binder above.
            let speech_reborrow = &mut config.features.speech;
            ui.label(egui::RichText::new("Hotkey mode").color(theme::FG_SOFT()).size(12.0));
            egui::ComboBox::from_id_salt("settings_speech_hotkey_mode")
                .selected_text(match speech_reborrow.hotkey_mode {
                    SpeechHotkeyMode::Hold => "Hold (Ventrilo-style)",
                    SpeechHotkeyMode::Toggle => "Toggle",
                })
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(
                            &mut speech_reborrow.hotkey_mode,
                            SpeechHotkeyMode::Hold,
                            "Hold (Ventrilo-style)",
                        )
                        .changed();
                    changed |= ui
                        .selectable_value(&mut speech_reborrow.hotkey_mode, SpeechHotkeyMode::Toggle, "Toggle")
                        .changed();
                });
            ui.end_row();
        });

    changed
}

fn speech_model_rows(ui: &mut Ui, config: &mut Config, model_info: Option<&SpeechModelInfo>) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new("Model (GGUF)").color(theme::FG_SOFT()).size(12.0));
    changed |= ui
        .add(egui::TextEdit::singleline(&mut config.features.speech.model).desired_width(260.0))
        .changed();
    ui.end_row();

    ui.label(String::new());
    match model_info {
        Some(info) => super::dim_label(
            ui,
            &format!(
                "{} languages · translate: {}",
                info.languages.len(),
                match info.supports_translate {
                    Some(true) => "yes",
                    Some(false) => "no",
                    None => "family default",
                }
            ),
        ),
        None => super::dim_label(ui, "model metadata unavailable — free-form language entry"),
    }
    ui.end_row();
    changed
}

fn speech_language_row(ui: &mut Ui, config: &mut Config, model_info: Option<&SpeechModelInfo>) -> bool {
    let mut changed = false;
    ui.label(
        egui::RichText::new("Spoken language")
            .color(theme::FG_SOFT())
            .size(12.0),
    );
    match model_info {
        Some(info) if !info.languages.is_empty() => {
            // A model that forbids auto-detect needs an explicit source; if
            // the value is still `auto`, adopt the first declared language.
            if info.supports_lang_detect == Some(false) && config.features.speech.language.trim() == "auto" {
                config.features.speech.language.clone_from(&info.languages[0]);
                changed = true;
            }
            egui::ComboBox::from_id_salt("settings_speech_language")
                .selected_text(config.features.speech.language.clone())
                .height(240.0)
                .show_ui(ui, |ui| {
                    // Some models declare stt.capability.lang_detect = false
                    // and require an explicit source language.
                    if info.supports_lang_detect != Some(false) {
                        changed |= ui
                            .selectable_value(
                                &mut config.features.speech.language,
                                "auto".to_string(),
                                "auto (detect)",
                            )
                            .changed();
                    }
                    for code in &info.languages {
                        changed |= ui
                            .selectable_value(&mut config.features.speech.language, code.clone(), code)
                            .changed();
                    }
                });
        }
        _ => {
            changed |= ui
                .add(egui::TextEdit::singleline(&mut config.features.speech.language).desired_width(70.0))
                .changed();
        }
    }
    ui.end_row();
    changed
}

fn speech_output_row(ui: &mut Ui, config: &mut Config, model_info: Option<&SpeechModelInfo>) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new("Output").color(theme::FG_SOFT()).size(12.0));
    let targets: Vec<String> = match model_info {
        Some(info) if info.supports_translate == Some(false) => Vec::new(),
        // Honor declared src→tgt pair restrictions for the chosen source.
        Some(info) if !info.translate_targets.is_empty() || !info.translate_pairs.is_empty() => {
            info.targets_for_source(&config.features.speech.language)
        }
        // Absent metadata: the family default applies, so still offer English.
        _ => vec!["en".to_string()],
    };
    // If the spoken language changed so the configured target is no longer
    // valid, snap to the first valid target, or fall back to transcription.
    if config.features.speech.task == SpeechTask::Translate
        && !targets.is_empty()
        && !targets.contains(&config.features.speech.target_language)
    {
        config.features.speech.target_language.clone_from(&targets[0]);
        changed = true;
    }
    if config.features.speech.task == SpeechTask::Translate && targets.is_empty() {
        config.features.speech.task = SpeechTask::Transcribe;
        changed = true;
    }
    let selected = match config.features.speech.task {
        SpeechTask::Transcribe => "Transcribe (keep spoken language)".to_string(),
        SpeechTask::Translate => format!("Translate to {}", config.features.speech.target_language),
    };
    egui::ComboBox::from_id_salt("settings_speech_output")
        .selected_text(selected)
        .show_ui(ui, |ui| {
            changed |= ui
                .selectable_value(
                    &mut config.features.speech.task,
                    SpeechTask::Transcribe,
                    "Transcribe (keep spoken language)",
                )
                .changed();
            for target in &targets {
                let is_selected = config.features.speech.task == SpeechTask::Translate
                    && config.features.speech.target_language == *target;
                if ui
                    .selectable_label(is_selected, format!("Translate to {target}"))
                    .clicked()
                {
                    config.features.speech.task = SpeechTask::Translate;
                    config.features.speech.target_language.clone_from(target);
                    changed = true;
                }
            }
        });
    // Deliberately not conditioned on the task. A model declaring no translate
    // support yields an empty `targets`, which forces the task back to
    // Transcribe above and leaves the picker with no translate entries to
    // select — so `task == Translate` could never hold here, making the
    // warning unreachable and the downgrade silent. Shown unconditionally it
    // explains why the translate options are missing.
    if matches!(model_info, Some(info) if info.supports_translate == Some(false)) {
        ui.end_row();
        ui.label(String::new());
        super::dim_label(ui, "⚠ this model does not support the translate task");
    }
    ui.end_row();
    changed
}

fn speech_backend_row(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new("Backend").color(theme::FG_SOFT()).size(12.0));
    ui.horizontal(|ui| {
        // Explicit backends are required, not best-effort: offering one this
        // binary did not compile guarantees a load failure. Auto/CPU always
        // work; the GPU entries follow the compiled feature set.
        let mut backends = vec![(SpeechBackend::Auto, "Auto"), (SpeechBackend::Cpu, "CPU")];
        if cfg!(feature = "speech-cuda") {
            backends.push((SpeechBackend::Cuda, "CUDA"));
        }
        if cfg!(feature = "speech-vulkan") {
            backends.push((SpeechBackend::Vulkan, "Vulkan"));
        }
        if cfg!(target_os = "macos") {
            backends.push((SpeechBackend::Metal, "Metal"));
        }
        let selected_available = backends
            .iter()
            .any(|(value, _)| *value == config.features.speech.backend);
        egui::ComboBox::from_id_salt("settings_speech_backend")
            .selected_text(format!("{:?}", config.features.speech.backend))
            .show_ui(ui, |ui| {
                for (value, label) in backends {
                    changed |= ui
                        .selectable_value(&mut config.features.speech.backend, value, label)
                        .changed();
                }
            });
        if !selected_available {
            super::dim_label(
                ui,
                &format!(
                    "⚠ {:?} is not compiled into this build — rebuild with the matching speech feature",
                    config.features.speech.backend
                ),
            );
        }
        let active: Option<String> = ui.data(|data| data.get_temp(egui::Id::new("speech_active_backend")));
        if let Some(active) = active {
            super::dim_label(ui, &format!("active: {active}"));
        }
    });
    ui.end_row();
    changed
}

/// Parse a model's GGUF header, throttled to a few times a second and cached
/// in egui temp memory. The settings panel is immediate-mode, so without the
/// throttle an `fs::metadata` + header parse would run every frame while the
/// panel is open. Keyed on the path, so the last result is reused between
/// checks and a model replaced in place is picked up within the interval.
fn cached_speech_model_info(ui: &Ui, path: &str) -> Option<SpeechModelInfo> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let now = ui.input(|input| input.time);
    let throttle_id = egui::Id::new(("speech_model_info_at", trimmed));
    let identity_id = egui::Id::new(("speech_model_info_id", trimmed));
    let result_id = egui::Id::new(("speech_model_info_val", trimmed));

    // Reuse the cached result until the throttle window elapses without
    // touching the filesystem at all.
    let last = ui.data(|data| data.get_temp::<f64>(throttle_id));
    let cached = ui.data(|data| data.get_temp::<Option<SpeechModelInfo>>(result_id));
    if let (Some(last), Some(cached)) = (last, cached.clone())
        && now - last < 0.5
    {
        return cached;
    }

    // Throttle window elapsed: stat the file (cheap). Only re-parse the
    // header (potentially megabytes of tokenizer arrays) when its identity
    // (len + mtime) actually changed.
    let expanded = horizon_core::dir_search::expand_tilde(trimmed);
    let identity = std::fs::metadata(&expanded).map_or((0_u64, 0_u128), |meta| {
        // Full nanosecond mtime, so a same-size in-place replacement within a
        // second still invalidates the cached metadata.
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        (meta.len(), modified)
    });
    let prev_identity = ui.data(|data| data.get_temp::<(u64, u128)>(identity_id));
    if prev_identity == Some(identity)
        && let Some(cached) = cached
    {
        // Unchanged file: refresh the throttle timestamp, keep the parse.
        ui.data_mut(|data| data.insert_temp(throttle_id, now));
        return cached;
    }
    let parsed = read_speech_model_info(&expanded);
    ui.data_mut(|data| {
        data.insert_temp(throttle_id, now);
        data.insert_temp(identity_id, identity);
        data.insert_temp(result_id, parsed.clone());
    });
    parsed
}

/// Current binding label plus a press-to-bind capture flow. Returns whether
/// the hotkey changed.
fn render_hotkey_binder(ui: &mut Ui, config: &mut Config) -> bool {
    let capture_id = egui::Id::new("speech_hotkey_capturing");
    let error_id = egui::Id::new("speech_hotkey_error");
    let mut changed = false;
    let capturing: bool = ui.data(|data| data.get_temp(capture_id)).unwrap_or(false);

    ui.horizontal(|ui| {
        let current = config.features.speech.hotkey.trim();
        let label = if current.is_empty() {
            "(none)".to_string()
        } else {
            // Render with the platform-primary label so macOS shows `Cmd`.
            horizon_core::ShortcutBinding::parse(current).map_or_else(
                |_| current.to_string(),
                |binding| binding.display_label(primary_shortcut_label()),
            )
        };
        ui.label(egui::RichText::new(label).color(theme::FG()).size(12.0).monospace());

        if capturing {
            super::dim_label(ui, "press a key combination… (Esc cancels)");
            let (captured, captured_clipboard_event) = ui.input(|input| {
                let captured = hotkey_capture_attempt(&input.events);
                let captured_clipboard_event = captured.is_some() && input.events.iter().any(is_clipboard_pseudo_event);
                (captured, captured_clipboard_event)
            });
            if let Some(captured) = captured {
                if captured_clipboard_event {
                    // The terminal filter runs after settings rendering, when
                    // `capture_id` has already been cleared below. Preserve a
                    // one-frame claim for synthetic Copy/Cut/Paste events.
                    mark_captured_clipboard_event(ui.ctx());
                }
                ui.data_mut(|data| data.insert_temp(capture_id, false));
                match captured {
                    HotkeyCaptureAttempt::Key {
                        key,
                        physical_key,
                        modifiers,
                    } => {
                        // The terminal event filter swallows this key's repeats,
                        // release, and text until the release is seen, so the
                        // captured chord never types into the focused terminal. The
                        // PHYSICAL key is recorded alongside the logical one: a
                        // shifted keycap presses as one logical key (Shift+1 ->
                        // Exclamationmark) and releases as another (Num1) when Shift
                        // is lifted first — only the physical key is stable.
                        arm_pending_capture(ui, key, physical_key, modifiers.shift, None);
                        if key != egui::Key::Escape {
                            match captured_binding_string(key, modifiers, config) {
                                Ok(binding) => {
                                    config.features.speech.hotkey = binding;
                                    ui.data_mut(|data| data.remove_temp::<String>(error_id));
                                    changed = true;
                                }
                                Err(message) => {
                                    ui.data_mut(|data| data.insert_temp(error_id, message));
                                }
                            }
                        }
                    }
                    HotkeyCaptureAttempt::ClipboardReserved(clipboard) => {
                        // egui-winit substitutes the pseudo-event for the key
                        // press but still emits a normal source-key release
                        // later. Keep it out of kitty-protocol terminals.
                        let key = clipboard.fallback_key();
                        arm_pending_capture(ui, key, Some(key), false, Some(clipboard));
                        ui.data_mut(|data| data.insert_temp(error_id, CLIPBOARD_HOTKEY_ERROR.to_string()));
                    }
                }
            }
        } else {
            if ui.button("Rebind…").clicked() {
                ui.data_mut(|data| {
                    data.insert_temp(capture_id, true);
                    // Clear any stale error from a previous attempt.
                    data.remove_temp::<String>(error_id);
                });
            }
            if !current.is_empty() && ui.button("Clear").clicked() {
                config.features.speech.hotkey = String::new();
                changed = true;
            }
        }
    });
    let error: Option<String> = ui.data(|data| data.get_temp(error_id));
    if let Some(error) = error {
        ui.end_row();
        ui.label(String::new());
        super::dim_label(ui, &format!("⚠ {error}"));
    }
    changed
}

fn arm_pending_capture(
    ui: &Ui,
    key: egui::Key,
    physical_key: Option<egui::Key>,
    shifted: bool,
    clipboard: Option<ClipboardCapture>,
) {
    let armed_at = ui.input(|input| input.time);
    ui.data_mut(|data| {
        data.insert_temp(
            egui::Id::new("speech_captured_key"),
            Some(PendingCapture {
                key,
                physical_key,
                shifted,
                clipboard,
                armed_at,
            }),
        );
    });
}

fn hotkey_capture_attempt(events: &[egui::Event]) -> Option<HotkeyCaptureAttempt> {
    events.iter().find_map(|event| match event {
        egui::Event::Key {
            key,
            physical_key,
            pressed: true,
            repeat: false,
            modifiers,
        } => Some(HotkeyCaptureAttempt::Key {
            key: *key,
            physical_key: *physical_key,
            modifiers: *modifiers,
        }),
        egui::Event::Copy => Some(HotkeyCaptureAttempt::ClipboardReserved(ClipboardCapture::Copy)),
        egui::Event::Cut => Some(HotkeyCaptureAttempt::ClipboardReserved(ClipboardCapture::Cut)),
        egui::Event::Paste(_) => Some(HotkeyCaptureAttempt::ClipboardReserved(ClipboardCapture::Paste)),
        _ => None,
    })
}

/// Build and validate a shortcut string from a captured key event. Rejects
/// unsupported keys, malformed chords, and overlaps with global shortcuts.
fn captured_binding_string(key: egui::Key, modifiers: egui::Modifiers, config: &Config) -> Result<String, String> {
    let mut parts: Vec<&str> = Vec::new();
    // `Ctrl` is the platform-primary token (Command on macOS). On macOS the
    // physical Control key is distinct and serializes as `Control`, so a
    // Cmd+Control chord must emit both. On Linux/Windows the Ctrl key sets
    // both `command` and `ctrl`, so only the primary token is emitted.
    if modifiers.command {
        parts.push("Ctrl");
    }
    if modifiers.ctrl && (modifiers.mac_cmd || !modifiers.command) {
        parts.push("Control");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    // `Key::name()` yields glyphs like `+` for Plus, which the shared
    // parser rejects (`Ctrl++` is an empty component); use canonical tokens.
    let key_name = match key {
        egui::Key::Plus => "Plus",
        egui::Key::Equals => "Equals",
        egui::Key::Minus => "Minus",
        egui::Key::Comma => "Comma",
        _ => key.name(),
    };
    parts.push(key_name);
    let candidate = parts.join("+");

    // Validate only the hotkey (parse/reserved/bare/clipboard/global overlap),
    // not the whole config — the model and other fields may still be empty
    // while the user is binding a key.
    let shortcuts = config.shortcuts.resolve().map_err(|error| error.to_string())?;
    horizon_core::validate_speech_hotkey(&candidate, &shortcuts).map_err(|error| error.to_string())?;
    // Reject a chord that duplicates another configured profile's hotkey.
    for profile in &config.features.speech.profiles {
        if profile.hotkey.trim().eq_ignore_ascii_case(candidate.trim()) {
            return Err(format!("`{candidate}` is already used by profile `{}`", profile.name));
        }
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, Modifiers};
    use horizon_core::Config;

    use crate::app::shortcuts::take_captured_clipboard_event;

    use super::{
        CLIPBOARD_HOTKEY_ERROR, ClipboardCapture, HotkeyCaptureAttempt, captured_binding_string,
        hotkey_capture_attempt, render_hotkey_binder,
    };

    #[test]
    fn clipboard_pseudo_events_disarm_capture_with_reserved_error() {
        for (event, expected_key, expected_clipboard) in [
            (Event::Copy, Key::C, ClipboardCapture::Copy),
            (Event::Cut, Key::X, ClipboardCapture::Cut),
            (Event::Paste("text".to_string()), Key::V, ClipboardCapture::Paste),
        ] {
            let ctx = egui::Context::default();
            let capture_id = egui::Id::new("speech_hotkey_capturing");
            let error_id = egui::Id::new("speech_hotkey_error");
            ctx.data_mut(|data| data.insert_temp(capture_id, true));
            let mut config = Config::default();

            let _ = ctx.run(
                egui::RawInput {
                    events: vec![event],
                    ..egui::RawInput::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        assert!(!render_hotkey_binder(ui, &mut config));
                    });
                },
            );

            let capturing: bool = ctx.data(|data| data.get_temp(capture_id)).unwrap_or(false);
            let error: Option<String> = ctx.data(|data| data.get_temp(error_id));
            let pending = ctx
                .data(|data| data.get_temp::<Option<super::PendingCapture>>(egui::Id::new("speech_captured_key")))
                .flatten();
            assert!(!capturing);
            assert_eq!(error.as_deref(), Some(CLIPBOARD_HOTKEY_ERROR));
            assert!(pending.is_some_and(|pending| {
                pending.key == expected_key
                    && pending.physical_key == Some(expected_key)
                    && pending.clipboard == Some(expected_clipboard)
            }));
            assert!(take_captured_clipboard_event(&ctx));
            assert!(!take_captured_clipboard_event(&ctx));
        }
    }

    #[test]
    fn clipboard_error_does_not_expand_the_settings_grid() {
        fn render_panel(ctx: &egui::Context, config: &mut Config) -> f32 {
            let mut panel_width = 0.0_f32;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1_600.0, 900.0))),
                    ..egui::RawInput::default()
                },
                |ctx| {
                    let panel = egui::SidePanel::right("speech_hotkey_width_test")
                        .default_width(480.0)
                        .min_width(240.0)
                        .max_width(800.0)
                        .show(ctx, |ui| {
                            egui::Grid::new("clipboard_error_width_test")
                                .num_columns(2)
                                .show(ui, |ui| {
                                    ui.label("Push-to-talk");
                                    assert!(!render_hotkey_binder(ui, config));
                                    ui.end_row();
                                });
                        });
                    panel_width = panel.response.rect.width();
                },
            );
            panel_width
        }

        let ctx = egui::Context::default();
        let error_id = egui::Id::new("speech_hotkey_error");
        let mut config = Config::default();
        let baseline = render_panel(&ctx, &mut config);
        ctx.data_mut(|data| data.insert_temp(error_id, CLIPBOARD_HOTKEY_ERROR.to_string()));
        let with_error = render_panel(&ctx, &mut config);

        assert!(
            with_error <= baseline + 1.0,
            "settings panel expanded from {baseline}px to {with_error}px"
        );
    }

    #[test]
    fn key_before_clipboard_pseudo_event_preserves_both_suppression_claims() {
        let ctx = egui::Context::default();
        let capture_id = egui::Id::new("speech_hotkey_capturing");
        let pending_id = egui::Id::new("speech_captured_key");
        ctx.data_mut(|data| data.insert_temp(capture_id, true));
        let mut config = Config::default();

        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    Event::Key {
                        key: Key::C,
                        physical_key: Some(Key::C),
                        pressed: true,
                        repeat: false,
                        modifiers: Modifiers::COMMAND,
                    },
                    Event::Copy,
                ],
                ..egui::RawInput::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    assert!(!render_hotkey_binder(ui, &mut config));
                });
            },
        );

        let pending = ctx
            .data(|data| data.get_temp::<Option<super::PendingCapture>>(pending_id))
            .flatten();
        assert!(pending.is_some_and(|pending| pending.key == Key::C));
        assert!(take_captured_clipboard_event(&ctx));
    }

    #[test]
    fn ordinary_key_presses_are_captured_but_repeats_are_not() {
        let key_press = Event::Key {
            key: Key::F9,
            physical_key: Some(Key::F9),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        assert!(matches!(
            hotkey_capture_attempt(std::slice::from_ref(&key_press)),
            Some(HotkeyCaptureAttempt::Key { key: Key::F9, .. })
        ));

        let repeat = Event::Key {
            key: Key::F9,
            physical_key: Some(Key::F9),
            pressed: true,
            repeat: true,
            modifiers: Modifiers::NONE,
        };
        assert!(hotkey_capture_attempt(&[repeat]).is_none());
        assert!(hotkey_capture_attempt(&[Event::Text("a".to_string())]).is_none());
    }

    #[test]
    fn bare_keys_are_rejected_but_function_keys_are_allowed() {
        let config = Config::default();
        let bare =
            captured_binding_string(Key::K, Modifiers::NONE, &config).expect_err("a bare letter must be rejected");
        assert!(bare.contains("needs a modifier"), "{bare}");

        let function_key = captured_binding_string(Key::F9, Modifiers::NONE, &config)
            .expect("a bare function key is a valid push-to-talk hotkey");
        assert_eq!(function_key, "F9");
    }
}
