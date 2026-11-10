mod bar;
mod general;
mod presets;
mod remote_browsers;
mod remote_recovery;
mod remote_usage;
mod shortcuts;
#[cfg(test)]
mod tests;
mod yaml_editor;

use egui::{Color32, Margin, Stroke, Vec2};
use horizon_core::Config;
use horizon_core::remote_browser_credential::CredentialWorkbench;

use super::settings_io::SettingsReply;
use super::util;
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
    RemoteBrowsers,
    Yaml,
}

impl SettingsTab {
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Shortcuts => "Shortcuts",
            Self::Presets => "Presets",
            Self::RemoteBrowsers => "Remote browsers",
            Self::Yaml => "YAML",
        }
    }

    const ALL: &[Self] = &[
        Self::General,
        Self::Shortcuts,
        Self::Presets,
        Self::RemoteBrowsers,
        Self::Yaml,
    ];
}

pub(super) enum SettingsStatus {
    None,
    LivePreview,
    /// The write runs on the settings thread.
    Saving,
    Saved,
    Error(String),
}

pub(super) struct SettingsEditor {
    pub(super) buffer: String,
    pub(super) original: String,
    pub(super) status: SettingsStatus,
    active_tab: SettingsTab,
    editing_config: Option<Config>,
    credential_inputs: remote_browsers::CredentialInputs,
    portable_profile: remote_browsers::PortableProfilePanel,
    provider_usage: remote_usage::UsagePanels,
}

impl SettingsEditor {
    /// Adopt config text that was just written to disk by another path, so
    /// the YAML buffer, the saved baseline and the GUI tabs' parsed snapshot
    /// all describe the file again.
    pub(super) fn adopt_saved_text(&mut self, yaml: String) {
        self.editing_config = Config::from_yaml(&yaml).ok();
        self.buffer.clone_from(&yaml);
        self.original = yaml;
        self.status = SettingsStatus::Saved;
    }

    #[cfg(test)]
    pub(super) fn editing_config(&self) -> Option<&Config> {
        self.editing_config.as_ref()
    }
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
        if self.settings.is_some() {
            self.close_settings();
        } else {
            // The store is the only settings source: the editor shows the settings
            // of this run, never a YAML copy that the person exported or edited.
            let content = self.template_config_yaml();
            let editing_config = Config::from_yaml(&content).ok();
            self.settings = Some(SettingsEditor {
                original: content.clone(),
                buffer: content,
                status: SettingsStatus::None,
                active_tab: SettingsTab::General,
                editing_config,
                credential_inputs: remote_browsers::CredentialInputs::default(),
                portable_profile: remote_browsers::PortableProfilePanel::new(&self.config_path),
                provider_usage: remote_usage::UsagePanels::default(),
            });
        }
    }

    fn template_config_yaml(&self) -> String {
        self.template_config.to_yaml().unwrap_or_else(|_| {
            Config::default()
                .to_yaml()
                .unwrap_or_else(|_| "workspaces: []\n".to_string())
        })
    }

    pub(super) fn render_settings(&mut self, ui: &mut egui::Ui) {
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
                // A save that is running or finished owns the status line: a
                // preview must not replace it on the next frame.
                Ok(_) if !matches!(editor.status, SettingsStatus::Saved | SettingsStatus::Saving) => {
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
        let configuration = true;
        let config_path = self.config_path.display().to_string();
        let panel_state = PanelState {
            config_path: &config_path,
            status_text: &status_text,
            status_color,
            is_valid,
            has_changes,
            configuration,
        };

        if let Some(editor) = self.settings.as_mut() {
            let action = render_settings_panel(
                ui,
                panel_state,
                editor,
                &mut self.remote_browser_credentials,
                &mut self.browser_create_host.remote_allocations,
            );
            // A press in the bar is applied after the panel drew it.
            self.apply_settings_action(action);
        }
    }

    fn apply_live_preview(&mut self, config: &Config) {
        self.board.sync_workspace_metadata(config);
        self.apply_runtime_config(config);
    }

    fn close_settings(&mut self) {
        if let Some(editor) = self.settings.take()
            && let Ok(config) = Config::from_yaml(&editor.original)
        {
            self.apply_live_preview(&config);
        }
    }

    fn apply_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::None => {}
            SettingsAction::Close => self.close_settings(),
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
        let Some(buffer) = self.settings.as_ref().map(|editor| editor.buffer.clone()) else {
            return;
        };
        // The text is checked here, in memory; the write waits on the settings
        // thread, and a reply applies the saved settings in a later frame.
        match Config::from_yaml(&buffer) {
            Ok(config) => {
                self.apply_runtime_config(&config);
                self.settings_io.save_text(self.config_path.clone(), buffer);
                if let Some(editor) = self.settings.as_mut() {
                    editor.status = SettingsStatus::Saving;
                }
            }
            Err(error) => {
                if let Some(editor) = self.settings.as_mut() {
                    editor.status = SettingsStatus::Error(error.to_string());
                }
                tracing::error!("failed to validate config before save: {error}");
            }
        }
    }

    /// Apply what the settings thread finished, in this frame.
    pub(super) fn apply_settings_io(&mut self, ctx: &egui::Context) {
        while self.take_settings_reply() {}
        if self.settings_io.is_busy() {
            // An answer arrives off the UI thread; keep frames coming until it
            // reaches this window.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// Apply one finished request. Returns whether there was one.
    pub(super) fn take_settings_reply(&mut self) -> bool {
        let Some(reply) = self.settings_io.poll() else {
            return false;
        };
        {
            match reply {
                SettingsReply::Saved { path, text, config } => {
                    self.template_config = *config;
                    if let Some(editor) = self.settings.as_mut() {
                        editor.adopt_saved_text(text);
                    }
                    // Another window may have reloaded the presets meanwhile.
                    self.board.sync_workspace_metadata(&self.template_config);
                    tracing::info!(path = %path.display(), "settings saved");
                }
                SettingsReply::Reloaded { path, config } => {
                    self.settings_reload_pending = false;
                    if let Some(config) = config {
                        tracing::info!(path = %path.display(), "settings changed, reloading presets");
                        self.apply_runtime_config(&config);
                        self.board.sync_workspace_metadata(&config);
                    }
                }
                SettingsReply::Failed { what, error } => {
                    if what == "reload" {
                        self.settings_reload_pending = false;
                    }
                    if let Some(editor) = self.settings.as_mut() {
                        editor.status = SettingsStatus::Error(error.clone());
                    }
                    tracing::error!(request = what, "settings request failed: {error}");
                }
            }
        }
        true
    }

    /// Wait for every queued settings request, for a test that reads the files
    /// that a save writes.
    #[cfg(test)]
    pub(super) fn wait_for_settings_io(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.settings_io.is_busy() {
            assert!(
                std::time::Instant::now() < deadline,
                "the settings thread answered nothing"
            );
            self.take_settings_reply();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    pub(super) fn apply_runtime_config(&mut self, config: &Config) {
        if self.template_config.browser.remote != config.browser.remote {
            self.remote_browser_credentials
                .load_environment_bindings(&config.browser.remote);
        }
        self.template_config = config.clone();
        self.shortcuts = resolve_shortcuts(config);
        self.action_commands_cache =
            crate::command_registry::action_commands(&self.shortcuts, util::primary_shortcut_label());
        self.presets = config.resolved_presets();
        if self.appearance_theme != config.appearance.theme {
            self.appearance_theme = config.appearance.theme;
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
        SettingsStatus::Saving => ("Saving".to_string(), theme::FG_DIM()),
        SettingsStatus::Saved => ("Saved".to_string(), theme::PALETTE_GREEN()),
        SettingsStatus::Error(message) => (message.clone(), theme::PALETTE_RED()),
    }
}

/// What the Settings panel shows besides its editor: the status bar and the
/// path of the configuration that the YAML tab names.
#[derive(Clone, Copy)]
struct PanelState<'a> {
    config_path: &'a str,
    status_text: &'a str,
    status_color: Color32,
    is_valid: bool,
    has_changes: bool,
    configuration: bool,
}

fn render_settings_panel(
    ui: &mut egui::Ui,
    panel_state: PanelState<'_>,
    editor: &mut SettingsEditor,
    credentials: &mut CredentialWorkbench,
    allocations: &mut horizon_core::browser::remote_recovery::RemoteAllocations,
) -> SettingsAction {
    let viewport_width = util::viewport_local_rect(ui).width();
    let default_width = settings_panel_default_width(viewport_width);

    egui::Panel::right(SETTINGS_PANEL_ID)
        .default_size(default_width)
        .min_size(viewport_width * 0.15)
        .max_size(viewport_width * 0.5)
        .frame(
            egui::Frame::default()
                .fill(theme::BG_ELEVATED())
                .inner_margin(Margin::symmetric(24, 16))
                .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE())),
        )
        .show(ui, |ui| {
            // The panel is a fraction of the viewport, so its content draws
            // inside its own rect: a wide child scrolls or clips instead of
            // painting over the canvas beside it.
            ui.shrink_clip_rect(ui.max_rect());
            // The bar belongs to this panel, so the status line and its buttons
            // sit with the settings instead of under the canvas.
            let action = bar::render(
                ui,
                if panel_state.configuration {
                    panel_state.status_text
                } else {
                    ""
                },
                panel_state.status_color,
                panel_state.is_valid,
                panel_state.has_changes,
                panel_state.configuration,
            );
            ui.label(egui::RichText::new("Settings").color(theme::FG()).size(18.0).strong());
            ui.add_space(16.0);

            render_tab_bar(ui, editor);
            ui.add_space(16.0);

            let available = ui.available_size() - Vec2::new(0.0, 8.0);
            match editor.active_tab {
                SettingsTab::Yaml => {
                    yaml_editor::render(ui, panel_state.config_path, &mut editor.buffer, available);
                }
                tab => render_gui_tab(ui, tab, editor, credentials, allocations, available),
            }
            action
        })
        .inner
}

fn render_gui_tab(
    ui: &mut egui::Ui,
    tab: SettingsTab,
    editor: &mut SettingsEditor,
    credentials: &mut CredentialWorkbench,
    allocations: &mut horizon_core::browser::remote_recovery::RemoteAllocations,
    available: Vec2,
) {
    let SettingsEditor {
        editing_config,
        credential_inputs,
        portable_profile,
        provider_usage,
        buffer,
        ..
    } = editor;
    let Some(config) = editing_config else {
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
                SettingsTab::RemoteBrowsers => {
                    remote_recovery::render(ui, allocations);
                    remote_browsers::render(
                        ui,
                        config,
                        credentials,
                        credential_inputs,
                        portable_profile,
                        provider_usage,
                    )
                }
                // Yaml is handled before this function is called.
                SettingsTab::Yaml => return,
            };
            if changed && let Ok(yaml) = config.to_yaml() {
                *buffer = yaml;
            }
        });
}

fn render_tab_bar(ui: &mut egui::Ui, editor: &mut SettingsEditor) {
    let old_tab = editor.active_tab;

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 6.0);
        for &tab in SettingsTab::ALL {
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

fn section_card<R>(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let result = egui::Frame::default()
        .fill(theme::PANEL_BG())
        .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE()))
        .corner_radius(10)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            content(ui)
        })
        .inner;
    ui.add_space(12.0);
    result
}

fn dim_label(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).color(theme::FG_DIM()).size(11.0)).wrap());
}
