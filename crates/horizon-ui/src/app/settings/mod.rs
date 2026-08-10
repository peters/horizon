mod bar;
mod general;
mod presets;
mod shortcuts;
mod speech;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(in crate::app) use speech::ClipboardCapture;
pub(in crate::app) use speech::PendingCapture;
pub(in crate::app) use speech::SpeechModelInfoCache;
#[cfg(test)]
pub(in crate::app) use speech::SpeechSetupAgent;
pub(in crate::app) use speech::{SpeechSetupRequest, speech_setup_prompt};
mod yaml_editor;

use egui::{Color32, Context, Margin, Stroke, Vec2};
use horizon_core::Config;

use super::util::{self, atomic_write};
use super::{HorizonApp, resolve_shortcuts};
use crate::theme;

pub(super) const SETTINGS_BAR_ID: &str = "settings_bar";
pub(super) const SETTINGS_BAR_HEIGHT: f32 = 48.0;
pub(super) const SETTINGS_PANEL_ID: &str = "settings_panel";

const TAB_CORNER_RADIUS: f32 = 8.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Shortcuts,
    Presets,
    Yaml,
}

impl SettingsTab {
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Shortcuts => "Shortcuts",
            Self::Presets => "Presets",
            Self::Yaml => "YAML",
        }
    }

    const ALL: [Self; 4] = [Self::General, Self::Shortcuts, Self::Presets, Self::Yaml];
}

pub(super) enum SettingsStatus {
    None,
    LivePreview,
    Saved,
    Error(String),
}

pub(super) struct SettingsEditor {
    pub(super) buffer: String,
    pub(super) original: String,
    pub(super) status: SettingsStatus,
    has_valid_saved_config: bool,
    active_tab: SettingsTab,
    editing_config: Option<Config>,
    speech_agent_setup: speech::SpeechAgentSetupState,
}

struct LoadedSettingsYaml {
    content: String,
    has_valid_saved_config: bool,
}

#[derive(Clone, Copy)]
enum SettingsAction {
    None,
    Close,
    Revert,
    ResetDefaults,
    Save,
}

impl HorizonApp {
    pub(super) fn toggle_settings(&mut self) {
        if let Some(editor) = self.settings.take() {
            if let Ok(config) = Config::from_yaml(&editor.original) {
                self.apply_live_preview(&config);
            }
        } else {
            let loaded = self.load_or_generate_config_yaml();
            let content = loaded.content;
            let editing_config = Config::from_yaml(&content).ok();
            self.settings = Some(SettingsEditor {
                original: content.clone(),
                buffer: content,
                status: SettingsStatus::None,
                has_valid_saved_config: loaded.has_valid_saved_config,
                active_tab: SettingsTab::General,
                editing_config,
                speech_agent_setup: speech::SpeechAgentSetupState::new(),
            });
        }
    }

    fn load_or_generate_config_yaml(&self) -> LoadedSettingsYaml {
        load_settings_yaml(&self.config_path, self.template_config_yaml())
    }

    fn template_config_yaml(&self) -> String {
        self.template_config.to_yaml().unwrap_or_else(|_| {
            Config::default()
                .to_yaml()
                .unwrap_or_else(|_| "workspaces: []\n".to_string())
        })
    }

    pub(super) fn render_settings(&mut self, ctx: &Context) {
        let Some((buffer, original, has_valid_saved_config)) = self.settings.as_ref().map(|editor| {
            (
                editor.buffer.clone(),
                editor.original.clone(),
                editor.has_valid_saved_config,
            )
        }) else {
            return;
        };

        let parsed = Config::from_yaml(&buffer);
        let is_valid = parsed.is_ok();
        if let Ok(config) = &parsed {
            self.apply_live_preview(config);
        }
        if let Some(editor) = self.settings.as_mut() {
            match &parsed {
                Ok(_) if !matches!(editor.status, SettingsStatus::Saved) => {
                    editor.status = SettingsStatus::LivePreview;
                }
                Err(error) => {
                    editor.status = SettingsStatus::Error(error.to_string());
                }
                Ok(_) => {}
            }
        }

        let has_changes = buffer != original;
        let launch_gate = speech::SpeechSetupLaunchGate::new(is_valid && has_valid_saved_config, has_changes);
        let Some(editor) = self.settings.as_ref() else {
            return;
        };
        let (status_text, status_color) = settings_status(&editor.status);
        let action = bar::render(ctx, &status_text, status_color, is_valid, has_changes);
        self.apply_settings_action(action);

        let config_path = self.config_path.display().to_string();
        let workspace_cwd = self
            .board
            .active_workspace
            .and_then(|workspace_id| self.board.workspace(workspace_id))
            .and_then(|workspace| workspace.cwd.clone());
        let setup_request = self.settings.as_mut().and_then(|editor| {
            render_settings_panel(
                ctx,
                &config_path,
                editor,
                &mut self.speech_model_info_cache,
                launch_gate,
                workspace_cwd.as_deref(),
            )
        });
        if let Some(request) = setup_request {
            let launch_gate_after_render =
                self.settings
                    .as_ref()
                    .map_or(speech::SpeechSetupLaunchGate::new(false, false), |editor| {
                        speech::SpeechSetupLaunchGate::new(
                            Config::from_yaml(&editor.buffer).is_ok() && editor.has_valid_saved_config,
                            editor.buffer != editor.original,
                        )
                    });
            if launch_gate_after_render.can_launch() {
                self.handle_speech_setup_request(ctx, &request);
            } else if let Some(editor) = self.settings.as_mut()
                && let Some(reason) = launch_gate_after_render.blocked_reason()
            {
                editor.speech_agent_setup.set_launch_error(reason);
                ctx.request_repaint();
            }
        }
    }

    fn handle_speech_setup_request(&mut self, ctx: &Context, request: &SpeechSetupRequest) {
        let Some((launch_gate, expected_saved_contents)) = self.settings.as_ref().map(|editor| {
            (
                speech::SpeechSetupLaunchGate::new(
                    Config::from_yaml(&editor.buffer).is_ok() && editor.has_valid_saved_config,
                    editor.buffer != editor.original,
                ),
                editor.original.clone(),
            )
        }) else {
            return;
        };
        if let Some(reason) = launch_gate.blocked_reason() {
            if let Some(editor) = self.settings.as_mut() {
                editor.speech_agent_setup.set_launch_error(reason);
            }
            ctx.request_repaint();
            return;
        }
        if let Err(error) = speech::validate_setup_saved_config(&self.config_path, &expected_saved_contents) {
            if let Some(editor) = self.settings.as_mut() {
                record_setup_saved_config_failure(editor, error);
            }
            ctx.request_repaint();
            return;
        }

        match self.launch_speech_setup_agent(ctx, request) {
            Ok(_) => {
                self.settings = None;
            }
            Err(error) => {
                if let Some(editor) = self.settings.as_mut() {
                    editor.speech_agent_setup.set_launch_error(error.to_string());
                    ctx.request_repaint();
                }
            }
        }
    }

    fn apply_live_preview(&mut self, config: &Config) {
        self.board.sync_workspace_metadata(config);
        self.apply_runtime_config(config);
    }

    fn apply_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::None => {}
            SettingsAction::Close => {
                if let Some(editor) = self.settings.take()
                    && let Ok(config) = Config::from_yaml(&editor.original)
                {
                    self.apply_live_preview(&config);
                }
            }
            SettingsAction::Revert => {
                let original = self.settings.as_ref().map(|e| e.original.clone());
                if let Some(original) = original {
                    let parsed = Config::from_yaml(&original).ok();
                    if let Some(ref config) = parsed {
                        self.apply_live_preview(config);
                    }
                    if let Some(editor) = self.settings.as_mut() {
                        editor.buffer.clone_from(&original);
                        editor.editing_config = parsed;
                        editor.status = SettingsStatus::None;
                        editor.speech_agent_setup.clear_transient_launch_error();
                    }
                }
            }
            SettingsAction::ResetDefaults => {
                let default_yaml = Config::default()
                    .to_yaml()
                    .unwrap_or_else(|_| "workspaces: []\n".to_string());
                if let Some(editor) = self.settings.as_mut() {
                    editor.editing_config = Config::from_yaml(&default_yaml).ok();
                    editor.buffer = default_yaml;
                    editor.status = SettingsStatus::LivePreview;
                    editor.speech_agent_setup.clear_transient_launch_error();
                }
            }
            SettingsAction::Save => {
                self.save_settings();
            }
        }
    }

    fn save_settings(&mut self) {
        if let Some(parent) = self.config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Some(buffer) = self.settings.as_ref().map(|editor| editor.buffer.clone()) else {
            return;
        };
        match Config::from_yaml(&buffer) {
            Ok(config) => match atomic_write(&self.config_path, &buffer) {
                Ok(()) => {
                    self.apply_runtime_config(&config);
                    if let Some(editor) = self.settings.as_mut() {
                        editor.original.clone_from(&buffer);
                        editor.status = SettingsStatus::Saved;
                        editor.has_valid_saved_config = true;
                        editor.speech_agent_setup.clear_launch_error();
                    }
                    tracing::info!("config saved to {}", self.config_path.display());
                }
                Err(error) => {
                    if let Some(editor) = self.settings.as_mut() {
                        editor.status = SettingsStatus::Error(format!("Write error: {error}"));
                    }
                    tracing::error!("failed to write config: {error}");
                }
            },
            Err(error) => {
                if let Some(editor) = self.settings.as_mut() {
                    editor.status = SettingsStatus::Error(error.to_string());
                }
                tracing::error!("failed to validate config before save: {error}");
            }
        }
    }

    /// Whether the settings General tab — which hosts the speech hotkey
    /// binders (flat editor and per-profile rows alike) — is currently open.
    /// Used to auto-disarm a stale capture.
    pub(super) fn settings_speech_tab_open(&self) -> bool {
        self.settings.as_ref().is_some_and(|editor| {
            editor.active_tab == SettingsTab::General
                && editor
                    .editing_config
                    .as_ref()
                    .is_some_and(|config| config.features.speech.enabled)
        })
    }

    pub(super) fn apply_runtime_config(&mut self, config: &Config) {
        // Speech applies live: rebuild the subsystem whenever its config
        // changed (drops any in-flight recording, which is acceptable for a
        // settings change). Covers both settings saves and file reloads.
        if self.template_config.features.speech != config.features.speech {
            // Retire the old system BEFORE constructing the replacement: an
            // assignment's RHS runs first, and `from_config` can start
            // loading immediately (preloaded profiles), so the old workers
            // must already be registered as retiring or a preloader sails
            // past the retirement guard and loads beside a still-resident
            // model.
            self.speech = None;
            self.speech = super::speech::SpeechSystem::from_config(&config.features.speech);
            // Held bindings persist until their release is consumed (kitty
            // release safety); only stop-attribution is reset.
            self.speech_engaged_profile = None;
            self.speech_escape_cancelled = false;
            tracing::info!("speech configuration changed; speech system rebuilt");
        }
        self.template_config = config.clone();
        self.shortcuts = resolve_shortcuts(config);
        self.action_commands_cache =
            crate::command_registry::action_commands(&self.shortcuts, util::primary_shortcut_label());
        self.presets = config.resolved_presets();
        self.board.attention_enabled = config.features.attention_feed;
        if self.appearance_theme != config.appearance.theme {
            self.appearance_theme = config.appearance.theme;
            self.theme_applied = false;
        }
    }
}

fn record_setup_saved_config_failure(editor: &mut SettingsEditor, error: String) {
    editor.has_valid_saved_config = false;
    editor.speech_agent_setup.set_saved_config_error(error);
}

fn load_settings_yaml(config_path: &std::path::Path, fallback: String) -> LoadedSettingsYaml {
    match std::fs::read_to_string(config_path) {
        Ok(content) if Config::from_yaml(&content).is_ok() => LoadedSettingsYaml {
            content,
            has_valid_saved_config: true,
        },
        Ok(_) | Err(_) => LoadedSettingsYaml {
            content: fallback,
            has_valid_saved_config: false,
        },
    }
}

pub(super) fn settings_panel_default_width(viewport_width: f32) -> f32 {
    (viewport_width * 0.3).clamp(340.0, 900.0)
}

fn settings_status(status: &SettingsStatus) -> (String, Color32) {
    match status {
        SettingsStatus::None => (String::new(), theme::FG_DIM()),
        SettingsStatus::LivePreview => ("Live preview".to_string(), theme::FG_DIM()),
        SettingsStatus::Saved => ("Saved".to_string(), theme::PALETTE_GREEN()),
        SettingsStatus::Error(message) => (message.clone(), theme::PALETTE_RED()),
    }
}

fn render_settings_panel(
    ctx: &Context,
    config_path: &str,
    editor: &mut SettingsEditor,
    model_info_cache: &mut speech::SpeechModelInfoCache,
    launch_gate: speech::SpeechSetupLaunchGate,
    workspace_cwd: Option<&std::path::Path>,
) -> Option<SpeechSetupRequest> {
    let viewport_width = util::viewport_local_rect(ctx).width();
    let default_width = settings_panel_default_width(viewport_width);
    let mut setup_request = None;

    egui::SidePanel::right(SETTINGS_PANEL_ID)
        .default_width(default_width)
        .min_width(viewport_width * 0.15)
        .max_width(viewport_width * 0.5)
        .frame(
            egui::Frame::default()
                .fill(theme::BG_ELEVATED())
                .inner_margin(Margin::symmetric(24, 16))
                .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE())),
        )
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Settings").color(theme::FG()).size(18.0).strong());
            ui.add_space(16.0);

            render_tab_bar(ui, editor);
            ui.add_space(16.0);

            let available = ui.available_size() - Vec2::new(0.0, 8.0);
            match editor.active_tab {
                SettingsTab::Yaml => {
                    yaml_editor::render(ui, config_path, &mut editor.buffer, available);
                }
                tab => {
                    setup_request =
                        render_gui_tab(ui, tab, editor, model_info_cache, launch_gate, workspace_cwd, available);
                }
            }
        });

    setup_request
}

fn render_gui_tab(
    ui: &mut egui::Ui,
    tab: SettingsTab,
    editor: &mut SettingsEditor,
    model_info_cache: &mut speech::SpeechModelInfoCache,
    launch_gate: speech::SpeechSetupLaunchGate,
    workspace_cwd: Option<&std::path::Path>,
    available: Vec2,
) -> Option<SpeechSetupRequest> {
    match deserialize_gui_config(&editor.buffer) {
        Ok(config) if editor.editing_config.is_none() => {
            editor.editing_config = Some(config);
        }
        Ok(_) => {}
        Err(_) => {
            return render_invalid_gui_tab(ui, tab, editor, model_info_cache, launch_gate, workspace_cwd, available);
        }
    }

    let Some(ref mut config) = editor.editing_config else {
        ui.label(
            egui::RichText::new("Unable to parse current configuration")
                .color(theme::PALETTE_RED())
                .size(12.0),
        );
        return None;
    };

    let mut setup_request = None;
    egui::ScrollArea::vertical()
        .max_height(available.y)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let changed = match tab {
                SettingsTab::General => {
                    let result = general::render(
                        ui,
                        config,
                        model_info_cache,
                        &mut editor.speech_agent_setup,
                        launch_gate,
                        workspace_cwd,
                    );
                    setup_request = result.speech_setup_request;
                    result.changed
                }
                SettingsTab::Shortcuts => shortcuts::render(ui, config),
                SettingsTab::Presets => presets::render(ui, config),
                // Yaml is handled before this function is called.
                SettingsTab::Yaml => return,
            };
            if changed && let Ok(yaml) = config.to_yaml() {
                editor.buffer = yaml;
            }
        });

    setup_request
}

fn render_invalid_gui_tab(
    ui: &mut egui::Ui,
    tab: SettingsTab,
    editor: &mut SettingsEditor,
    model_info_cache: &mut speech::SpeechModelInfoCache,
    launch_gate: speech::SpeechSetupLaunchGate,
    workspace_cwd: Option<&std::path::Path>,
    available: Vec2,
) -> Option<SpeechSetupRequest> {
    let fallback_config = editor
        .editing_config
        .clone()
        .or_else(|| Config::from_yaml(&editor.original).ok());
    let mut setup_request = None;

    egui::ScrollArea::vertical()
        .max_height(available.y)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("Unable to parse current configuration")
                    .color(theme::PALETTE_RED())
                    .size(12.0),
            );
            dim_label(ui, "Fix the YAML or revert your changes to edit settings here.");

            if tab == SettingsTab::General
                && let Some(config) = fallback_config.as_ref()
            {
                ui.add_space(12.0);
                section_heading(ui, "Features");
                section_card(ui, |ui| {
                    setup_request = speech::render_read_only(
                        ui,
                        config,
                        model_info_cache,
                        &mut editor.speech_agent_setup,
                        launch_gate,
                        workspace_cwd,
                    );
                });
            }
        });

    setup_request
}

fn render_tab_bar(ui: &mut egui::Ui, editor: &mut SettingsEditor) {
    let old_tab = editor.active_tab;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for tab in SettingsTab::ALL {
            let selected = editor.active_tab == tab;
            let (fill, text_color) = if selected {
                (theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.2), theme::FG())
            } else {
                (Color32::TRANSPARENT, theme::FG_DIM())
            };

            let stroke = if selected {
                Stroke::new(1.0_f32, theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), 0.5))
            } else {
                Stroke::NONE
            };

            let btn = egui::Button::new(egui::RichText::new(tab.label()).size(12.0).color(text_color))
                .fill(fill)
                .stroke(stroke)
                .corner_radius(TAB_CORNER_RADIUS);

            if ui.add(btn).clicked() {
                editor.active_tab = tab;
            }
        }
    });

    // Re-deserialize the buffer when switching from YAML to a GUI tab. Keep
    // semantically invalid configurations editable so users can complete a
    // multi-step GUI edit (for example, enable speech and then choose a
    // model). Only malformed/unrenderable YAML keeps the last good snapshot.
    if old_tab == SettingsTab::Yaml
        && editor.active_tab != SettingsTab::Yaml
        && let Ok(config) = deserialize_gui_config(&editor.buffer)
    {
        editor.editing_config = Some(config);
    }
}

fn deserialize_gui_config(contents: &str) -> Result<Config, serde_yaml::Error> {
    serde_yaml::from_str(contents)
}

// -- Shared section helpers used by tab modules --------------------------

fn section_heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(title).color(theme::FG_SOFT()).size(13.0).strong());
    ui.add_space(6.0);
}

fn section_card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default()
        .fill(theme::PANEL_BG())
        .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE()))
        .corner_radius(10)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            content(ui);
        });
    ui.add_space(12.0);
}

fn dim_label(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).color(theme::FG_DIM()).size(11.0)).wrap());
}
