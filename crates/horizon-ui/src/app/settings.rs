use egui::{Align, Button, Color32, Context, FontId, Layout, Margin, Stroke, Vec2};
use horizon_core::Config;

use crate::theme;

use super::util::{atomic_write, chrome_button, primary_button};
use super::yaml_highlight::highlight_yaml;
use super::{HorizonApp, SettingsEditor, SettingsStatus, resolve_shortcuts};

pub(super) const SETTINGS_BAR_ID: &str = "settings_bar";
pub(super) const SETTINGS_BAR_HEIGHT: f32 = 48.0;
pub(super) const SETTINGS_PANEL_ID: &str = "settings_panel";

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
            let content = self.load_or_generate_config_yaml();
            self.settings = Some(SettingsEditor {
                original: content.clone(),
                buffer: content,
                status: SettingsStatus::None,
            });
        }
    }

    fn load_or_generate_config_yaml(&self) -> String {
        if let Ok(content) = std::fs::read_to_string(&self.config_path)
            && Config::from_yaml(&content).is_ok()
        {
            return content;
        }
        self.template_config_yaml()
    }

    fn template_config_yaml(&self) -> String {
        self.template_config.to_yaml().unwrap_or_else(|_| {
            Config::default()
                .to_yaml()
                .unwrap_or_else(|_| "workspaces: []\n".to_string())
        })
    }

    pub(super) fn render_settings(&mut self, ctx: &Context) {
        let Some((buffer, original)) = self
            .settings
            .as_ref()
            .map(|editor| (editor.buffer.clone(), editor.original.clone()))
        else {
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
        let Some(editor) = self.settings.as_ref() else {
            return;
        };
        let (status_text, status_color) = settings_status(&editor.status);
        let action = Self::render_settings_bar(ctx, &status_text, status_color, is_valid, has_changes);
        self.apply_settings_action(action);

        let config_path = self.config_path.display().to_string();
        if let Some(editor) = self.settings.as_mut() {
            render_settings_editor(ctx, &config_path, &mut editor.buffer);
        }
    }

    fn render_settings_bar(
        ctx: &Context,
        status_text: &str,
        status_color: Color32,
        is_valid: bool,
        has_changes: bool,
    ) -> SettingsAction {
        let mut action = SettingsAction::None;

        egui::TopBottomPanel::bottom(SETTINGS_BAR_ID)
            .exact_height(SETTINGS_BAR_HEIGHT)
            .frame(
                egui::Frame::default()
                    .fill(theme::BG_ELEVATED)
                    .inner_margin(Margin::symmetric(24, 8))
                    .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 100))),
            )
            .show(ctx, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if !status_text.is_empty() {
                        ui.label(egui::RichText::new(status_text).color(status_color).size(12.0));
                    }
                    if !is_valid {
                        ui.label(
                            egui::RichText::new("Invalid config")
                                .color(theme::PALETTE_RED)
                                .size(12.0),
                        );
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(primary_button("Save")).clicked() {
                            action = SettingsAction::Save;
                        } else if ui.add(chrome_button("Reset Defaults")).clicked() {
                            action = SettingsAction::ResetDefaults;
                        } else if has_changes && ui.add(chrome_button("Revert")).clicked() {
                            action = SettingsAction::Revert;
                        } else if ui
                            .add(
                                Button::new(egui::RichText::new("Close").size(12.0).color(theme::FG_SOFT))
                                    .fill(theme::PANEL_BG_ALT)
                                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                                    .corner_radius(8),
                            )
                            .clicked()
                        {
                            action = SettingsAction::Close;
                        }
                    });
                });
            });

        action
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
                    if let Ok(config) = Config::from_yaml(&original) {
                        self.apply_live_preview(&config);
                    }
                    if let Some(editor) = self.settings.as_mut() {
                        editor.buffer = original;
                        editor.status = SettingsStatus::None;
                    }
                }
            }
            SettingsAction::ResetDefaults => {
                let default_yaml = Config::default()
                    .to_yaml()
                    .unwrap_or_else(|_| "workspaces: []\n".to_string());
                if let Some(editor) = self.settings.as_mut() {
                    editor.buffer = default_yaml;
                    editor.status = SettingsStatus::LivePreview;
                }
            }
            SettingsAction::Save => {
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
                                editor.original = buffer;
                                editor.status = SettingsStatus::Saved;
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
        }
    }

    pub(super) fn apply_runtime_config(&mut self, config: &Config) {
        self.template_config = config.clone();
        self.shortcuts = resolve_shortcuts(config);
        self.action_commands_cache =
            crate::command_registry::action_commands(&self.shortcuts, super::util::primary_shortcut_label());
        self.presets.clone_from(&config.presets);
        self.board.attention_enabled = config.features.attention_feed;
    }
}

pub(super) fn settings_panel_default_width(viewport_width: f32) -> f32 {
    (viewport_width * 0.3).clamp(340.0, 900.0)
}

fn settings_status(status: &SettingsStatus) -> (String, Color32) {
    match status {
        SettingsStatus::None => (String::new(), theme::FG_DIM),
        SettingsStatus::LivePreview => ("Live preview".to_string(), theme::FG_DIM),
        SettingsStatus::Saved => ("Saved".to_string(), theme::PALETTE_GREEN),
        SettingsStatus::Error(message) => (message.clone(), theme::PALETTE_RED),
    }
}

fn render_settings_editor(ctx: &Context, config_path: &str, buffer: &mut String) {
    let font_id = FontId::monospace(13.0);
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap_width: f32| {
        let job = highlight_yaml(text.as_str(), &ui.style().text_styles[&egui::TextStyle::Monospace]);
        ui.fonts_mut(|f| f.layout_job(job))
    };

    let viewport_width = super::util::viewport_local_rect(ctx).width();
    let default_width = settings_panel_default_width(viewport_width);

    egui::SidePanel::right(SETTINGS_PANEL_ID)
        .default_width(default_width)
        .min_width(viewport_width * 0.15)
        .max_width(viewport_width * 0.5)
        .frame(
            egui::Frame::default()
                .fill(theme::BG_ELEVATED)
                .inner_margin(Margin::symmetric(24, 16))
                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Config File").color(theme::FG).size(18.0).strong());
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(config_path)
                        .color(theme::FG_DIM)
                        .size(12.0)
                        .monospace(),
                );
            });
            ui.add_space(12.0);

            let available = ui.available_size() - Vec2::new(0.0, 8.0);
            egui::Frame::default()
                .fill(theme::PANEL_BG)
                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                .corner_radius(8)
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(available.y)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(buffer)
                                    .font(font_id)
                                    .desired_width(available.x)
                                    .desired_rows(40)
                                    .frame(false)
                                    .layouter(&mut layouter),
                            );
                        });
                });
        });
}
