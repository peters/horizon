mod bar;
mod general;
mod presets;
mod shortcuts;
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
    active_tab: SettingsTab,
    editing_config: Option<Config>,
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
            let content = self.load_or_generate_config_yaml();
            let editing_config = Config::from_yaml(&content).ok();
            self.settings = Some(SettingsEditor {
                original: content.clone(),
                buffer: content,
                status: SettingsStatus::None,
                active_tab: SettingsTab::General,
                editing_config,
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
        let action = bar::render(ctx, &status_text, status_color, is_valid, has_changes);
        self.apply_settings_action(action);

        let config_path = self.config_path.display().to_string();
        if let Some(editor) = self.settings.as_mut() {
            render_settings_panel(ctx, &config_path, editor);
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

    pub(super) fn apply_runtime_config(&mut self, config: &Config) {
        self.template_config = config.clone();
        self.shortcuts = resolve_shortcuts(config);
        self.action_commands_cache =
            crate::command_registry::action_commands(&self.shortcuts, util::primary_shortcut_label());
        self.presets = config.resolved_presets();
        self.board.attention_enabled = config.features.attention_feed;
        if self.appearance_theme != config.appearance.theme {
            self.appearance_theme = config.appearance.theme;
            self.resolved_theme = match self.appearance_theme {
                horizon_core::AppearanceTheme::Auto => self.resolved_theme,
                horizon_core::AppearanceTheme::Dark => theme::ResolvedTheme::Dark,
                horizon_core::AppearanceTheme::Light => theme::ResolvedTheme::Light,
            };
            theme::set_theme(self.resolved_theme);
            self.theme_applied = false;
        }
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

fn render_settings_panel(ctx: &Context, config_path: &str, editor: &mut SettingsEditor) {
    let viewport_width = util::viewport_local_rect(ctx).width();
    let default_width = settings_panel_default_width(viewport_width);

    egui::SidePanel::right(SETTINGS_PANEL_ID)
        .default_width(default_width)
        .min_width(viewport_width * 0.15)
        .max_width(viewport_width * 0.5)
        .frame(
            egui::Frame::default()
                .fill(theme::BG_ELEVATED())
                .inner_margin(Margin::symmetric(24, 16))
                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE())),
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
                tab => render_gui_tab(ui, tab, editor, available),
            }
        });
}

fn render_gui_tab(ui: &mut egui::Ui, tab: SettingsTab, editor: &mut SettingsEditor, available: Vec2) {
    let Some(ref mut config) = editor.editing_config else {
        ui.label(
            egui::RichText::new("Unable to parse current configuration")
                .color(theme::PALETTE_RED())
                .size(12.0),
        );
        return;
    };

    egui::ScrollArea::vertical()
        .max_height(available.y)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let changed = match tab {
                SettingsTab::General => general::render(ui, config),
                SettingsTab::Shortcuts => shortcuts::render(ui, config),
                SettingsTab::Presets => presets::render(ui, config),
                // Yaml is handled before this function is called.
                SettingsTab::Yaml => return,
            };
            if changed && let Ok(yaml) = config.to_yaml() {
                editor.buffer = yaml;
            }
        });
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
                Stroke::new(1.0, theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), 0.5))
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

    // Re-parse buffer when switching from YAML to a GUI tab.
    if old_tab == SettingsTab::Yaml && editor.active_tab != SettingsTab::Yaml {
        editor.editing_config = Config::from_yaml(&editor.buffer).ok();
    }
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
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(10)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            content(ui);
        });
    ui.add_space(12.0);
}

fn dim_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::FG_DIM()).size(11.0));
}
