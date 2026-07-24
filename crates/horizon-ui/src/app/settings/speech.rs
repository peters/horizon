//! Speech Input settings section: profile summary, model metadata, source
//! language / output pickers, backend gating, and the press-to-bind hotkey
//! capture flow. Split from `general.rs` per the module-size guardrail.

use egui::Ui;
use horizon_core::speech_model::SpeechModelInfo;
use horizon_core::{Config, SpeechBackend, SpeechHotkeyMode, SpeechTask};

use crate::app::shortcuts::{is_clipboard_pseudo_event, mark_captured_clipboard_event};
use crate::app::speech::built_with_speech;

const CLIPBOARD_HOTKEY_ERROR: &str =
    "Clipboard shortcuts (Ctrl/Cmd+C, X, or V) are reserved and cannot be used as speech hotkeys";

mod model_info;
pub(in crate::app) use model_info::SpeechModelInfoCache;
use model_info::SpeechModelInfoState;

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

pub(super) fn render(ui: &mut Ui, config: &mut Config, model_info_cache: &mut SpeechModelInfoCache) -> bool {
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

    // The microphone applies to every profile, so it renders in both the
    // profiles and the flat single-model layout.
    ui.add_space(6.0);
    egui::Grid::new("settings_speech_mic_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            changed |= speech_microphone_row(ui, config);
        });

    // With named profiles, the flat single-model editor below would
    // mislead; show one row per profile with a press-to-bind hotkey flow,
    // and defer model/language editing to the YAML tab (which live-previews
    // and validates on save).
    if !config.features.speech.profiles.is_empty() {
        egui::Grid::new("settings_speech_profiles_grid")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for index in 0..config.features.speech.profiles.len() {
                    changed |= render_hotkey_binder_slot(ui, config, Some(index));
                    let profile = &config.features.speech.profiles[index];
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
            "Hotkeys rebind here and apply on save; profile models and languages are edited in the YAML tab (features.speech.profiles).",
        );
        super::dim_label(
            ui,
            "Push-to-talk presses are paused while this window is open — Rebind… also shows what any key press delivers.",
        );
        return changed;
    }

    let model_path = config.features.speech.model.clone();
    let model_info_state = model_info_cache.model_info(ui.ctx(), &model_path);
    let (model_info, model_info_pending) = match &model_info_state {
        SpeechModelInfoState::Available(info) => (Some(info), false),
        SpeechModelInfoState::Pending => (None, true),
        SpeechModelInfoState::Unavailable => (None, false),
    };

    ui.add_space(6.0);
    egui::Grid::new("settings_speech_grid")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            changed |= speech_model_rows(ui, config, &model_info_state);
            changed |= speech_language_row(ui, config, model_info);
            changed |= speech_output_row(ui, config, model_info, model_info_pending);
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

/// Cached result of the background input-device enumeration.
#[derive(Clone, Default)]
enum DeviceListState {
    #[default]
    Pending,
    Loaded(std::sync::Arc<Vec<String>>),
}

const DEVICE_LIST_ID: &str = "speech_input_devices";

/// The input devices the host reports, enumerated once on a background
/// thread (cpal enumeration can block) and cached in egui memory. `None`
/// while the scan is still running.
fn input_device_list(ui: &Ui) -> Option<std::sync::Arc<Vec<String>>> {
    let id = egui::Id::new(DEVICE_LIST_ID);
    let state: Option<DeviceListState> = ui.data(|data| data.get_temp(id));
    match state {
        Some(DeviceListState::Loaded(devices)) => Some(devices),
        Some(DeviceListState::Pending) => None,
        None => {
            ui.data_mut(|data| data.insert_temp(id, DeviceListState::Pending));
            let ctx = ui.ctx().clone();
            let spawned = std::thread::Builder::new()
                .name("speech-device-list".to_string())
                .spawn(move || {
                    let devices = std::sync::Arc::new(crate::app::speech::list_input_devices());
                    ctx.data_mut(|data| data.insert_temp(id, DeviceListState::Loaded(devices)));
                    ctx.request_repaint();
                });
            if let Err(error) = spawned {
                tracing::warn!(%error, "failed to spawn audio device enumeration thread");
                let empty = DeviceListState::Loaded(std::sync::Arc::new(Vec::new()));
                ui.data_mut(|data| data.insert_temp(id, empty));
            }
            None
        }
    }
}

/// Global microphone picker. Dictating into one microphone while capture
/// reads another (e.g. a webcam mic as system default) yields empty
/// transcripts that present as broken hotkeys, so the device is explicit.
fn speech_microphone_row(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new("Microphone").color(theme::FG_SOFT()).size(12.0));
    ui.horizontal(|ui| {
        let devices = input_device_list(ui);
        let current = config.features.speech.input_device.trim().to_string();
        let selected = if current.is_empty() {
            "System default".to_string()
        } else {
            current.clone()
        };
        egui::ComboBox::from_id_salt("settings_speech_input_device")
            .width(240.0)
            .selected_text(egui::RichText::new(selected).size(12.0))
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_empty(), "System default").clicked() && !current.is_empty() {
                    config.features.speech.input_device.clear();
                    changed = true;
                }
                match devices.as_deref().map(Vec::as_slice) {
                    None => super::dim_label(ui, "scanning audio devices…"),
                    Some([]) => super::dim_label(ui, "no input devices reported"),
                    Some(device_names) => {
                        let mut current_listed = current.is_empty();
                        for device in device_names {
                            let is_current = *device == current;
                            current_listed |= is_current;
                            if ui.selectable_label(is_current, device).clicked() && !is_current {
                                config.features.speech.input_device.clone_from(device);
                                changed = true;
                            }
                        }
                        if !current_listed {
                            super::dim_label(
                                ui,
                                &format!("`{current}` not found — capture falls back to system default"),
                            );
                        }
                    }
                }
            });
        let refresh = ui
            .button(egui::RichText::new("↻").size(12.0))
            .on_hover_text("Re-scan audio input devices");
        if refresh.clicked() {
            ui.data_mut(|data| data.remove_temp::<DeviceListState>(egui::Id::new(DEVICE_LIST_ID)));
            ui.ctx().request_repaint();
        }
    });
    ui.end_row();
    changed
}

fn speech_model_rows(ui: &mut Ui, config: &mut Config, model_info: &SpeechModelInfoState) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new("Model (GGUF)").color(theme::FG_SOFT()).size(12.0));
    changed |= ui
        .add(egui::TextEdit::singleline(&mut config.features.speech.model).desired_width(260.0))
        .changed();
    ui.end_row();

    ui.label(String::new());
    match model_info {
        SpeechModelInfoState::Available(info) => super::dim_label(
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
        SpeechModelInfoState::Pending => super::dim_label(ui, "loading model metadata…"),
        SpeechModelInfoState::Unavailable => {
            super::dim_label(ui, "model metadata unavailable — free-form language entry");
        }
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

fn speech_output_row(
    ui: &mut Ui,
    config: &mut Config,
    model_info: Option<&SpeechModelInfo>,
    model_info_pending: bool,
) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new("Output").color(theme::FG_SOFT()).size(12.0));
    let targets: Vec<String> = if model_info_pending {
        match config.features.speech.task {
            SpeechTask::Transcribe => Vec::new(),
            SpeechTask::Translate => vec![config.features.speech.target_language.clone()],
        }
    } else {
        match model_info {
            Some(info) if info.supports_translate == Some(false) => Vec::new(),
            // Honor declared src→tgt pair restrictions for the chosen source.
            Some(info) if !info.translate_targets.is_empty() || !info.translate_pairs.is_empty() => {
                info.targets_for_source(&config.features.speech.language)
            }
            // Absent metadata: the family default applies, so still offer English.
            _ => vec!["en".to_string()],
        }
    };
    // If the spoken language changed so the configured target is no longer
    // valid, snap to the first valid target, or fall back to transcription.
    if !model_info_pending
        && config.features.speech.task == SpeechTask::Translate
        && !targets.is_empty()
        && !targets.contains(&config.features.speech.target_language)
    {
        config.features.speech.target_language.clone_from(&targets[0]);
        changed = true;
    }
    if !model_info_pending && config.features.speech.task == SpeechTask::Translate && targets.is_empty() {
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
    if !model_info_pending && matches!(model_info, Some(info) if info.supports_translate == Some(false)) {
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

/// Current binding label plus a press-to-bind capture flow for the flat
/// (single-profile) editor. Returns whether the hotkey changed.
fn render_hotkey_binder(ui: &mut Ui, config: &mut Config) -> bool {
    render_hotkey_binder_slot(ui, config, None)
}

/// The hotkey a binder slot edits: the flat field (`None`) or one profile's.
fn slot_hotkey(config: &Config, slot: Option<usize>) -> &str {
    match slot {
        None => &config.features.speech.hotkey,
        Some(index) => config
            .features
            .speech
            .profiles
            .get(index)
            .map_or("", |profile| profile.hotkey.as_str()),
    }
}

fn set_slot_hotkey(config: &mut Config, slot: Option<usize>, hotkey: String) {
    match slot {
        None => config.features.speech.hotkey = hotkey,
        Some(index) => {
            if let Some(profile) = config.features.speech.profiles.get_mut(index) {
                profile.hotkey = hotkey;
            }
        }
    }
}

/// One binding label plus press-to-bind capture flow. `slot` selects the flat
/// hotkey (`None`) or a profile row; all rows share one capture/error state,
/// scoped by the armed slot so only the row that was clicked captures.
fn render_hotkey_binder_slot(ui: &mut Ui, config: &mut Config, slot: Option<usize>) -> bool {
    let capture_id = egui::Id::new("speech_hotkey_capturing");
    let capture_slot_id = egui::Id::new("speech_hotkey_capture_slot");
    let error_id = egui::Id::new("speech_hotkey_error");
    let error_slot_id = egui::Id::new("speech_hotkey_error_slot");
    let mut changed = false;
    let capturing_any: bool = ui.data(|data| data.get_temp(capture_id)).unwrap_or(false);
    let armed_slot: Option<usize> = ui.data(|data| data.get_temp(capture_slot_id)).unwrap_or(None);
    let capturing = capturing_any && armed_slot == slot;

    ui.horizontal(|ui| {
        let current = slot_hotkey(config, slot).trim().to_string();
        let label = if current.is_empty() {
            // The first profile without a hotkey is reachable via the mic
            // button; the flat editor just shows the binding as absent.
            if slot == Some(0) {
                "(mic button)".to_string()
            } else {
                "(none)".to_string()
            }
        } else {
            // Render with the platform-primary label so macOS shows `Cmd`.
            horizon_core::ShortcutBinding::parse(&current).map_or_else(
                |_| current.clone(),
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
                            match captured_binding_string(key, physical_key, modifiers, config, slot) {
                                Ok(binding) => {
                                    set_slot_hotkey(config, slot, binding);
                                    ui.data_mut(|data| {
                                        data.remove_temp::<String>(error_id);
                                        data.remove_temp::<Option<usize>>(error_slot_id);
                                    });
                                    changed = true;
                                }
                                Err(message) => {
                                    ui.data_mut(|data| {
                                        data.insert_temp(error_id, message);
                                        data.insert_temp(error_slot_id, slot);
                                    });
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
                        ui.data_mut(|data| {
                            data.insert_temp(error_id, CLIPBOARD_HOTKEY_ERROR.to_string());
                            data.insert_temp(error_slot_id, slot);
                        });
                    }
                }
            }
        } else {
            if ui.button("Rebind…").clicked() {
                ui.data_mut(|data| {
                    data.insert_temp(capture_id, true);
                    data.insert_temp(capture_slot_id, slot);
                    // Clear any stale error from a previous attempt.
                    data.remove_temp::<String>(error_id);
                    data.remove_temp::<Option<usize>>(error_slot_id);
                });
            }
            // Only the first profile may go hotkey-less (mic-button default);
            // clearing a later profile would make it unreachable, which
            // validation rejects — so don't offer it.
            let clearable = slot.is_none_or(|index| index == 0);
            if clearable && !current.is_empty() && ui.button("Clear").clicked() {
                set_slot_hotkey(config, slot, String::new());
                changed = true;
            }
        }
    });
    let error: Option<String> = ui.data(|data| data.get_temp(error_id));
    let error_slot: Option<usize> = ui.data(|data| data.get_temp(error_slot_id)).unwrap_or(None);
    if let Some(error) = error
        && error_slot == slot
    {
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

/// Whether an egui key is a function key (`F1`–`F35`).
fn is_function_key(key: egui::Key) -> bool {
    let name = key.name();
    name.len() >= 2 && name.starts_with('F') && name[1..].chars().all(|c| c.is_ascii_digit())
}

/// Build and validate a shortcut string from a captured key event. Rejects
/// unsupported keys, malformed chords, overlaps with global shortcuts, and
/// overlaps with other profiles' hotkeys (`slot` names the profile being
/// rebound so its own current key is not a conflict; `None` = flat editor).
fn captured_binding_string(
    key: egui::Key,
    physical_key: Option<egui::Key>,
    modifiers: egui::Modifiers,
    config: &Config,
    slot: Option<usize>,
) -> Result<String, String> {
    // A physically pressed F-key whose logical key was mangled (input-method
    // bridge, remapped layout) must still bind as that F-key — the mirror of
    // `key_matches`, which matches function bindings by physical key too.
    // Without this, the exact keys most likely to need the binder-as-probe
    // flow would be the ones it cannot bind.
    let key = match physical_key {
        Some(physical) if is_function_key(physical) && !is_function_key(key) => physical,
        _ => key,
    };
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
    // Reject a chord that overlaps another profile's hotkey (same rule the
    // config validator applies on save, surfaced at capture time instead).
    let binding = horizon_core::ShortcutBinding::parse(&candidate).map_err(|error| error.to_string())?;
    for (index, profile) in config.features.speech.profiles.iter().enumerate() {
        if slot == Some(index) {
            continue;
        }
        let Ok(existing) = horizon_core::ShortcutBinding::parse(profile.hotkey.trim()) else {
            continue;
        };
        if binding.overlaps(existing) {
            let name = if profile.name.trim().is_empty() {
                format!("profile #{}", index + 1)
            } else {
                format!("profile `{}`", profile.name.trim())
            };
            return Err(format!("`{candidate}` overlaps {name}'s hotkey `{}`", profile.hotkey));
        }
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, Modifiers};
    use horizon_core::{Config, SpeechTask};

    use crate::app::shortcuts::take_captured_clipboard_event;

    use super::{
        CLIPBOARD_HOTKEY_ERROR, ClipboardCapture, HotkeyCaptureAttempt, captured_binding_string,
        hotkey_capture_attempt, render_hotkey_binder, speech_output_row,
    };

    #[test]
    fn pending_model_metadata_does_not_rewrite_translation_target() {
        let ctx = egui::Context::default();
        let mut config = Config::default();
        config.features.speech.task = SpeechTask::Translate;
        config.features.speech.target_language = "de".to_string();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(!speech_output_row(ui, &mut config, None, true));
            });
        });

        assert_eq!(config.features.speech.task, SpeechTask::Translate);
        assert_eq!(config.features.speech.target_language, "de");
    }

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
        let bare = captured_binding_string(Key::K, None, Modifiers::NONE, &config, None)
            .expect_err("a bare letter must be rejected");
        assert!(bare.contains("needs a modifier"), "{bare}");

        let function_key = captured_binding_string(Key::F9, None, Modifiers::NONE, &config, None)
            .expect("a bare function key is a valid push-to-talk hotkey");
        assert_eq!(function_key, "F9");
    }

    #[test]
    fn capture_binds_the_physical_function_key_when_the_logical_key_is_mangled() {
        let config = Config::default();
        // An input-method bridge can deliver an F-key press whose logical
        // key is something else entirely; the keycap must still bind (the
        // mirror of `key_matches`, which matches F-keys physically too).
        let mangled = captured_binding_string(Key::K, Some(Key::F1), Modifiers::NONE, &config, None)
            .expect("physical F1 must bind despite a mangled logical key");
        assert_eq!(mangled, "F1");
        // A logical F-key wins over its own physical position…
        let logical =
            captured_binding_string(Key::F2, Some(Key::F2), Modifiers::NONE, &config, None).expect("plain F2 binds");
        assert_eq!(logical, "F2");
        // …and a remapped layout that produces a logical F-key from a
        // non-F physical key still binds the logical key.
        let remapped = captured_binding_string(Key::F5, Some(Key::K), Modifiers::NONE, &config, None)
            .expect("logical F5 binds regardless of physical position");
        assert_eq!(remapped, "F5");
    }

    fn profiles_config() -> Config {
        let mut config = Config::default();
        config.features.speech.enabled = true;
        config.features.speech.profiles = vec![
            horizon_core::SpeechProfile {
                name: "Norsk".to_string(),
                model: "/no.gguf".to_string(),
                hotkey: "F1".to_string(),
                ..horizon_core::SpeechProfile::default()
            },
            horizon_core::SpeechProfile {
                name: "English".to_string(),
                model: "/en.gguf".to_string(),
                hotkey: "F2".to_string(),
                ..horizon_core::SpeechProfile::default()
            },
        ];
        config
    }

    #[test]
    fn captured_binding_rejects_other_profiles_hotkey_but_not_its_own() {
        let config = profiles_config();
        // Rebinding profile 1 to profile 0's key must be refused…
        let error = captured_binding_string(Key::F1, None, Modifiers::NONE, &config, Some(1))
            .expect_err("another profile's key must be rejected");
        assert!(error.contains("Norsk"), "{error}");
        // …while re-capturing its own current key is not a conflict.
        let own =
            captured_binding_string(Key::F2, None, Modifiers::NONE, &config, Some(1)).expect("own key is no conflict");
        assert_eq!(own, "F2");
        // A fresh key is accepted.
        let fresh =
            captured_binding_string(Key::F3, None, Modifiers::NONE, &config, Some(1)).expect("unused key passes");
        assert_eq!(fresh, "F3");
    }

    #[test]
    fn profile_rebind_captures_into_the_armed_profile_only() {
        let ctx = egui::Context::default();
        ctx.data_mut(|data| {
            data.insert_temp(egui::Id::new("speech_hotkey_capturing"), true);
            data.insert_temp(egui::Id::new("speech_hotkey_capture_slot"), Some(1_usize));
        });
        let mut config = profiles_config();

        let mut changed_slot0 = false;
        let mut changed_slot1 = false;
        let _ = ctx.run(
            egui::RawInput {
                events: vec![Event::Key {
                    key: Key::F5,
                    physical_key: Some(Key::F5),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }],
                ..egui::RawInput::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // The unarmed row must ignore the press entirely.
                    changed_slot0 = super::render_hotkey_binder_slot(ui, &mut config, Some(0));
                    changed_slot1 = super::render_hotkey_binder_slot(ui, &mut config, Some(1));
                });
            },
        );

        assert!(!changed_slot0);
        assert!(changed_slot1);
        assert_eq!(config.features.speech.profiles[0].hotkey, "F1");
        assert_eq!(config.features.speech.profiles[1].hotkey, "F5");
        let capturing: bool = ctx
            .data(|data| data.get_temp(egui::Id::new("speech_hotkey_capturing")))
            .unwrap_or(false);
        assert!(!capturing, "capture must disarm after a successful bind");
    }
}
