use egui::Ui;
use horizon_core::speech_model::{SpeechModelInfo, read_speech_model_info};
use horizon_core::{AppearanceTheme, Config, SpeechBackend, SpeechHotkeyMode, SpeechTask};

use crate::app::speech::built_with_speech;
use crate::theme;

/// Render the General settings tab: window dimensions, feature toggles,
/// and overlay sizes.  Returns `true` when any value was modified.
pub(super) fn render(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    changed |= render_window_section(ui, config);
    changed |= render_appearance_section(ui, config);
    changed |= render_features_section(ui, config);
    changed |= render_overlays_section(ui, config);
    changed
}

fn render_window_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Window");
    super::section_card(ui, |ui| {
        super::dim_label(ui, "Default window size and position on launch.");
        ui.add_space(8.0);

        egui::Grid::new("settings_window_grid")
            .num_columns(4)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Width").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.window.width)
                            .range(400.0..=8000.0)
                            .speed(2.0)
                            .suffix(" px"),
                    )
                    .changed();

                ui.label(egui::RichText::new("Height").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.window.height)
                            .range(300.0..=5000.0)
                            .speed(2.0)
                            .suffix(" px"),
                    )
                    .changed();
                ui.end_row();

                ui.label(egui::RichText::new("X").color(theme::FG_SOFT()).size(12.0));
                changed |= optional_f32_drag(ui, &mut config.window.x, "px");

                ui.label(egui::RichText::new("Y").color(theme::FG_SOFT()).size(12.0));
                changed |= optional_f32_drag(ui, &mut config.window.y, "px");
                ui.end_row();
            });
    });
    changed
}

fn render_appearance_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Appearance");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "Follow the system theme by default, or force a light or dark override.",
        );
        ui.add_space(8.0);

        egui::ComboBox::from_id_salt("settings_appearance_theme")
            .selected_text(theme_label(config.appearance.theme))
            .show_ui(ui, |ui| {
                changed |= ui
                    .selectable_value(
                        &mut config.appearance.theme,
                        AppearanceTheme::Auto,
                        theme_label(AppearanceTheme::Auto),
                    )
                    .changed();
                changed |= ui
                    .selectable_value(
                        &mut config.appearance.theme,
                        AppearanceTheme::Dark,
                        theme_label(AppearanceTheme::Dark),
                    )
                    .changed();
                changed |= ui
                    .selectable_value(
                        &mut config.appearance.theme,
                        AppearanceTheme::Light,
                        theme_label(AppearanceTheme::Light),
                    )
                    .changed();
            });
    });
    changed
}

fn render_features_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Features");
    super::section_card(ui, |ui| {
        changed |= ui
            .checkbox(
                &mut config.features.attention_feed,
                egui::RichText::new("Attention Feed").color(theme::FG()).size(12.0),
            )
            .changed();
        super::dim_label(ui, "Show a notification feed for agent activity.");

        ui.add_space(10.0);
        changed |= render_speech_settings(ui, config);
    });
    changed
}

fn render_speech_settings(ui: &mut Ui, config: &mut Config) -> bool {
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
        Some(info) if !info.translate_targets.is_empty() => info.translate_targets.clone(),
        // Absent metadata: the family default applies, so still offer English.
        _ => vec!["en".to_string()],
    };
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
    if matches!(model_info, Some(info) if info.supports_translate == Some(false))
        && config.features.speech.task == SpeechTask::Translate
    {
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

/// Cache GGUF header parses per model path in egui temp memory so the file
/// is only re-read when the path changes.
fn cached_speech_model_info(ui: &Ui, path: &str) -> Option<SpeechModelInfo> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let expanded = horizon_core::dir_search::expand_tilde(trimmed);
    // Keying on file identity (length + mtime) as well as the path means a
    // model downloaded, replaced, or converted at the same location is
    // re-read instead of serving stale (or permanently-None) metadata.
    let identity = std::fs::metadata(&expanded).map_or((0, 0), |meta| {
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_secs());
        (meta.len(), modified)
    });
    let key = egui::Id::new(("speech_model_info", trimmed, identity));
    if let Some(cached) = ui.data(|data| data.get_temp::<Option<SpeechModelInfo>>(key)) {
        return cached;
    }
    let parsed = read_speech_model_info(&expanded);
    ui.data_mut(|data| data.insert_temp(key, parsed.clone()));
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
        let label = if current.is_empty() { "(none)" } else { current };
        ui.label(egui::RichText::new(label).color(theme::FG()).size(12.0).monospace());

        if capturing {
            super::dim_label(ui, "press a key combination… (Esc cancels)");
            let captured = ui.input(|input| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            if let Some((key, modifiers)) = captured {
                ui.data_mut(|data| data.insert_temp(capture_id, false));
                // The terminal event filter swallows this key's repeats,
                // release, and text until the release is seen, so the
                // captured chord never types into the focused terminal.
                ui.data_mut(|data| data.insert_temp(egui::Id::new("speech_captured_key"), Some(key)));
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
        } else {
            if ui.button("Rebind…").clicked() {
                ui.data_mut(|data| data.insert_temp(capture_id, true));
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

/// Build and validate a shortcut string from a captured key event. Rejects
/// unsupported keys, malformed chords, and overlaps with global shortcuts.
fn captured_binding_string(key: egui::Key, modifiers: egui::Modifiers, config: &Config) -> Result<String, String> {
    let mut parts: Vec<&str> = Vec::new();
    // `Ctrl` is the platform-primary token (Command on macOS); a physical
    // Control on macOS sets `ctrl` without `command` and serializes as the
    // distinct `Control` token.
    if modifiers.command {
        parts.push("Ctrl");
    } else if modifiers.ctrl {
        parts.push("Control");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    let key_name = key.name();
    parts.push(key_name);
    let candidate = parts.join("+");

    let mut probe = config.clone();
    probe.features.speech.enabled = true;
    probe.features.speech.hotkey.clone_from(&candidate);
    match probe.validate() {
        Ok(()) => Ok(candidate),
        Err(error) => Err(error.to_string()),
    }
}

fn render_overlays_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Overlays");
    super::section_card(ui, |ui| {
        super::dim_label(ui, "Dimensions of overlay widgets on the canvas.");
        ui.add_space(8.0);

        egui::Grid::new("settings_overlays_grid")
            .num_columns(4)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Feed Width").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.overlays.attention_feed_width)
                            .range(120.0..=800.0)
                            .speed(1.0)
                            .suffix(" px"),
                    )
                    .changed();

                ui.label(egui::RichText::new("Feed Height").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.overlays.attention_feed_height)
                            .range(100.0..=1200.0)
                            .speed(1.0)
                            .suffix(" px"),
                    )
                    .changed();
                ui.end_row();

                ui.label(egui::RichText::new("Map Width").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.overlays.minimap_width)
                            .range(80.0..=600.0)
                            .speed(1.0)
                            .suffix(" px"),
                    )
                    .changed();

                ui.label(egui::RichText::new("Map Height").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.overlays.minimap_height)
                            .range(60.0..=500.0)
                            .speed(1.0)
                            .suffix(" px"),
                    )
                    .changed();
                ui.end_row();
            });
    });
    changed
}

/// Render a `DragValue` for an `Option<f32>`.  An unchecked checkbox
/// clears the value to `None`.
fn optional_f32_drag(ui: &mut Ui, value: &mut Option<f32>, suffix: &str) -> bool {
    let mut changed = false;
    let mut enabled = value.is_some();

    if ui.checkbox(&mut enabled, "").changed() {
        *value = if enabled { Some(0.0) } else { None };
        changed = true;
    }

    if let Some(v) = value.as_mut() {
        changed |= ui
            .add(
                egui::DragValue::new(v)
                    .range(-10000.0..=10000.0)
                    .speed(1.0)
                    .suffix(format!(" {suffix}")),
            )
            .changed();
    }

    changed
}

fn theme_label(theme: AppearanceTheme) -> &'static str {
    match theme {
        AppearanceTheme::Auto => "Auto (system)",
        AppearanceTheme::Dark => "Dark",
        AppearanceTheme::Light => "Light",
    }
}
