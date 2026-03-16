use egui::{Align, Button, Color32, Context, Layout, Margin, Pos2, Rect, Stroke, UiBuilder, Vec2};
use horizon_core::Config;

use crate::theme;

use super::util::{atomic_write, chrome_button, primary_button};
use super::{HorizonApp, SettingsEditor, SettingsStatus};

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
            if let Ok(config) = serde_yaml::from_str::<Config>(&editor.original) {
                self.board.sync_workspace_metadata(&config);
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
            && serde_yaml::from_str::<Config>(&content).is_ok()
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

        let parsed = serde_yaml::from_str::<Config>(&buffer);
        let is_valid = parsed.is_ok();
        if let Ok(config) = &parsed {
            self.board.sync_workspace_metadata(config);
            if let Some(editor) = self.settings.as_mut()
                && !matches!(editor.status, SettingsStatus::Saved)
            {
                editor.status = SettingsStatus::LivePreview;
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
        let canvas_rect = Self::canvas_rect(ctx, self.sidebar_visible);
        if let Some(editor) = self.settings.as_mut() {
            render_settings_editor(ctx, canvas_rect, &config_path, &mut editor.buffer);
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

        egui::TopBottomPanel::bottom("settings_bar")
            .exact_height(48.0)
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
                        ui.label(egui::RichText::new("Invalid YAML").color(theme::PALETTE_RED).size(12.0));
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

    fn apply_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::None => {}
            SettingsAction::Close => {
                if let Some(editor) = self.settings.take()
                    && let Ok(config) = serde_yaml::from_str::<Config>(&editor.original)
                {
                    self.board.sync_workspace_metadata(&config);
                }
            }
            SettingsAction::Revert => {
                if let Some(editor) = self.settings.as_mut() {
                    let original = editor.original.clone();
                    if let Ok(config) = serde_yaml::from_str::<Config>(&original) {
                        self.board.sync_workspace_metadata(&config);
                    }
                    editor.buffer = original;
                    editor.status = SettingsStatus::None;
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
                if let Some(editor) = self.settings.as_mut() {
                    match atomic_write(&self.config_path, &editor.buffer) {
                        Ok(()) => {
                            if let Ok(config) = serde_yaml::from_str::<Config>(&editor.buffer) {
                                self.template_config = config.clone();
                                self.presets.clone_from(&config.presets);
                            }
                            editor.original = editor.buffer.clone();
                            editor.status = SettingsStatus::Saved;
                            tracing::info!("config saved to {}", self.config_path.display());
                        }
                        Err(error) => {
                            editor.status = SettingsStatus::Error(format!("Write error: {error}"));
                            tracing::error!("failed to write config: {error}");
                        }
                    }
                }
            }
        }
    }
}

fn settings_status(status: &SettingsStatus) -> (String, Color32) {
    match status {
        SettingsStatus::None => (String::new(), theme::FG_DIM),
        SettingsStatus::LivePreview => ("Live preview".to_string(), theme::FG_DIM),
        SettingsStatus::Saved => ("Saved".to_string(), theme::PALETTE_GREEN),
        SettingsStatus::Error(message) => (message.clone(), theme::PALETTE_RED),
    }
}

fn render_settings_editor(ctx: &Context, canvas_rect: Rect, config_path: &str, buffer: &mut String) {
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(theme::BG_ELEVATED))
        .show(ctx, |ui| {
            let content_rect = Rect::from_min_max(
                Pos2::new(canvas_rect.min.x + 24.0, canvas_rect.min.y + 16.0),
                Pos2::new(canvas_rect.max.x - 24.0, ui.max_rect().max.y),
            );
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(content_rect)
                    .layout(Layout::top_down(Align::Min)),
                |ui| {
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
                                            .font(egui::FontId::monospace(13.0))
                                            .text_color(theme::FG)
                                            .desired_width(available.x)
                                            .desired_rows(40)
                                            .frame(false),
                                    );
                                });
                        });
                },
            );
        });
}
