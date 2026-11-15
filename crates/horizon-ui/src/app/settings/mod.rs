mod bar;
mod browser;
#[cfg(test)]
mod field_tests;
#[cfg(test)]
mod footer_tests;
mod general;
#[cfg(test)]
mod machines_tests;
#[cfg(test)]
mod panel_test_support;
#[cfg(test)]
mod panel_tests;
mod presets;
mod remote_browsers;
mod remote_recovery;
mod remote_usage;
mod secrets;
pub(in crate::app) mod server;
mod shortcuts;
mod speech;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) use tests::{at, frame_of, texts_hold};

use egui::{Color32, Margin, Rect, Stroke, Vec2};
use horizon_core::Config;
use horizon_core::named_secrets::NamedSecretWorkbench;
use horizon_core::remote_browser_credential::{CredentialWorkbench, KeyringImport};

use super::settings_io::SettingsReply;
use super::util;
use super::{HorizonApp, resolve_shortcuts};
use crate::theme;

pub(super) const SETTINGS_BAR_ID: &str = "settings_bar";
pub(super) const SETTINGS_BAR_HEIGHT: f32 = 48.0;
/// The height of the bar while a value of another computer waits: the sentence of
/// the question has a row of its own above the two choices, so the bar grows by one
/// row of text and the spacing beside it.
pub(super) const SETTINGS_BAR_CONFLICT_HEIGHT: f32 = 72.0;
pub(super) const SETTINGS_PANEL_ID: &str = "settings_panel";

const TAB_CORNER_RADIUS: f32 = 8.0;
/// The horizontal padding of one tab button. The eight tabs must fit one row of
/// the panel that a window of 1480 points opens, which is 396 points of content,
/// so the padding and the gap stay small and the labels stay short: the row needs
/// about 389 of the 396 points.
const TAB_BUTTON_PADDING: f32 = 3.0;
/// The gap between two tab buttons.
const TAB_ITEM_GAP: f32 = 2.0;
/// The size of a tab label.
const TAB_LABEL_SIZE: f32 = 12.0;
/// The margin that the panel frame takes on each side.
const SETTINGS_PANEL_MARGIN_X: i8 = 24;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Shortcuts,
    Presets,
    Browser,
    RemoteBrowsers,
    Secrets,
    Server,
    Speech,
}

impl SettingsTab {
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Shortcuts => "Keys",
            Self::Presets => "Presets",
            Self::Browser => "Browser",
            Self::RemoteBrowsers => "Remote",
            Self::Secrets => "Secrets",
            Self::Server => "Server",
            Self::Speech => "Speech",
        }
    }

    const ALL: &[Self] = &[
        Self::General,
        Self::Shortcuts,
        Self::Presets,
        Self::Browser,
        Self::RemoteBrowsers,
        Self::Secrets,
        Self::Server,
        Self::Speech,
    ];
}

/// The sections where a value of another computer arrived while the person has
/// unsaved edits, and the settings of the store that hold that value.
///
/// The draft of the tab keeps the edits of the person, and the footer offers the
/// two choices: take the value of the other computer, or keep editing. Every other
/// section of the store applies at once.
pub(super) struct SettingsConflict {
    pub(super) sections: Vec<String>,
    pub(super) stored: Box<Config>,
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
    /// The settings that the tabs edit.
    pub(super) config: Config,
    /// The settings of the store when the editor opened, for Revert.
    pub(super) original: Config,
    /// Whether a tab changed `config` since it was adopted.
    pub(super) dirty: bool,
    pub(super) status: SettingsStatus,
    /// A value of another computer for a section that the person edits.
    pub(super) conflict: Option<SettingsConflict>,
    /// The rectangle that the panel took in the last frame that drew it, so the
    /// canvas paints only beside it. A closed panel has none.
    pub(super) panel_rect: Option<Rect>,
    active_tab: SettingsTab,
    credential_inputs: remote_browsers::CredentialInputs,
    portable_profile: remote_browsers::PortableProfilePanel,
    provider_usage: remote_usage::UsagePanels,
    secret_inputs: secrets::SecretInputs,
    pub(in crate::app) server: server::ServerTabState,
}

impl SettingsEditor {
    /// Adopt settings that were just written to the store by another path, so
    /// the tabs and the saved baseline describe the store again.
    pub(super) fn adopt_saved(&mut self, config: Config) {
        self.config = config.clone();
        self.original = config;
        self.dirty = false;
        self.conflict = None;
        self.status = SettingsStatus::Saved;
        // A saved value is in the store, so the window keeps no copy of it.
        self.secret_inputs.scrub();
    }

    #[cfg(test)]
    pub(super) fn editing_config(&self) -> &Config {
        &self.config
    }
}

#[derive(Clone, Copy)]
enum SettingsAction {
    None,
    Close,
    Revert,
    ResetDefaults,
    Save,
    /// Take the value of another computer for the sections that this window kept
    /// in its draft.
    TakeRemote,
    /// Keep the edits of this window, and let the next Save write them.
    KeepEditing,
}

impl HorizonApp {
    pub(super) fn toggle_settings(&mut self) {
        if self.settings.is_some() {
            self.close_settings();
        } else {
            // The store is the only settings source: the editor shows the settings
            // of this run, never a file that a person exported and edited.
            let config = self.template_config.clone();
            let mut server = server::ServerTabState::default();
            // The tab offers the name that the first link of this computer would
            // take, so a person presses Join without typing one, whether or not the
            // tab was drawn before.
            server.propose_device();
            self.settings = Some(SettingsEditor {
                original: config.clone(),
                config,
                dirty: false,
                status: SettingsStatus::None,
                conflict: None,
                panel_rect: None,
                active_tab: SettingsTab::General,
                credential_inputs: remote_browsers::CredentialInputs::default(),
                portable_profile: remote_browsers::PortableProfilePanel::new(&self.config_path),
                provider_usage: remote_usage::UsagePanels::default(),
                secret_inputs: secrets::SecretInputs::default(),
                server,
            });
        }
    }

    /// Opens the settings, on the Presets tab: where the person changes which
    /// agent Set up starts.
    pub(super) fn open_settings_presets(&mut self) {
        if self.settings.is_none() {
            self.toggle_settings();
        }
        if let Some(editor) = self.settings.as_mut() {
            editor.active_tab = SettingsTab::Presets;
        }
    }

    pub(super) fn render_settings(&mut self, ui: &mut egui::Ui) {
        // The answers of the server that arrived since the last frame land in the
        // tab before it draws, so a frame with nothing to read allocates nothing.
        if let Some(editor) = self.settings.as_mut() {
            editor.server.poll();
        }
        let view = self.server_view();
        let Some(editor) = self.settings.as_mut() else {
            return;
        };
        let (status_text, status_color) = settings_status(&editor.status);
        let panel_state = PanelState {
            status_text: &status_text,
            status_color,
            has_changes: editor.dirty,
            conflict: editor.conflict.as_ref().map(|conflict| conflict.sections.join(", ")),
        };
        // The canvas takes this rectangle from the editor, so it ends at the panel
        // whatever the active tab asked for.
        editor.panel_rect = Some(panel_rect(ui));
        let missing_notification_program = self.notifications.missing_program_sentence();
        let general_view = general::GeneralView {
            missing_notification_program: missing_notification_program.as_deref(),
        };
        let speech_view = speech::SpeechView {
            answer: self.speech.try_answer(),
            running: self.speech.try_running(),
            kept: self.speech.kept_take(),
            secret: self
                .named_secrets
                .state_of(&editor.config.speech.transcriber.key_secret),
        };
        let (action, changed, speech_request) = render_settings_panel(
            ui,
            &panel_state,
            editor,
            &mut TabWorkbenches {
                credentials: &mut self.remote_browser_credentials,
                allocations: &mut self.browser_create_host.remote_allocations,
                named_secrets: &mut self.named_secrets,
                keyring: &mut self.keyring_import,
                speech: &speech_view,
            },
            &view,
            general_view,
        );
        if speech_request.try_take {
            self.try_speech(ui.ctx());
        }
        if speech_request.retry_take {
            self.retry_speech_take(ui.ctx());
        }
        if changed {
            if let Some(editor) = self.settings.as_mut() {
                editor.dirty = true;
                if !matches!(editor.status, SettingsStatus::Saved | SettingsStatus::Saving) {
                    editor.status = SettingsStatus::LivePreview;
                }
            }
            // The tabs edit the draft in the editor, so one clone per changed
            // frame carries it to the canvas behind the panel.
            if let Some(config) = self.settings.as_ref().map(|editor| editor.config.clone()) {
                self.apply_live_preview(&config);
            }
        }
        // A press in the bar is applied after the panel drew it, and so is a press
        // of the Server tab, which asks the server one question.
        self.apply_settings_action(action);
        if let Some(asked) = self.settings.as_mut().and_then(|editor| editor.server.action.take()) {
            self.apply_server_action(asked, ui.ctx().clone());
        }
        // A tab that waits for an answer keeps the frames coming, so the answer
        // reaches the window as soon as the worker has it.
        if self.settings.as_ref().is_some_and(|editor| editor.server.is_busy()) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn apply_live_preview(&mut self, config: &Config) {
        self.board.sync_workspace_metadata(config);
        self.apply_runtime_config(config);
    }

    fn close_settings(&mut self) {
        // The next open reads the hosts of the ssh config again.
        self.settings_hosts_refreshed = false;
        if let Some(mut editor) = self.settings.take() {
            editor.secret_inputs.scrub();
            self.apply_live_preview(&editor.original);
        }
    }

    fn apply_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::None => {}
            SettingsAction::Close => self.close_settings(),
            SettingsAction::Revert => {
                let original = self.settings.as_ref().map(|editor| editor.original.clone());
                if let Some(original) = original {
                    self.apply_live_preview(&original);
                    if let Some(editor) = self.settings.as_mut() {
                        editor.config = original;
                        editor.dirty = false;
                        editor.status = SettingsStatus::None;
                    }
                }
            }
            SettingsAction::ResetDefaults => {
                let defaults = Config::default();
                self.apply_live_preview(&defaults);
                if let Some(editor) = self.settings.as_mut() {
                    editor.config = defaults;
                    editor.dirty = true;
                    editor.status = SettingsStatus::LivePreview;
                }
            }
            SettingsAction::Save => {
                self.save_settings();
            }
            SettingsAction::TakeRemote => {
                self.take_remote_settings();
            }
            SettingsAction::KeepEditing => {
                if let Some(editor) = self.settings.as_mut() {
                    editor.conflict = None;
                }
            }
        }
    }

    /// Takes the value of the other computer for the sections that this window kept
    /// in its draft, and leaves every other edit of the person alone.
    fn take_remote_settings(&mut self) {
        let Some(editor) = self.settings.as_mut() else {
            return;
        };
        let Some(conflict) = editor.conflict.take() else {
            return;
        };
        let mut draft = editor.config.clone();
        for section in &conflict.sections {
            let Some(value) = horizon_core::settings_sync::body_of(&conflict.stored, section)
                .ok()
                .flatten()
            else {
                continue;
            };
            if let Ok(next) = horizon_core::settings_sync::with_section(&draft, section, &value) {
                draft = next;
            }
        }
        editor.dirty = draft_dirty(&draft, &editor.original);
        if editor.dirty {
            editor.status = SettingsStatus::LivePreview;
        } else {
            editor.status = SettingsStatus::None;
        }
        editor.config = draft;
        let draft = editor.config.clone();
        self.apply_live_preview(&draft);
    }

    /// Reads the settings that the worker read again, and applies them.
    ///
    /// While the Settings window holds unsaved edits in a section that the store
    /// changed, the draft keeps them, the footer offers the two choices, and every
    /// other section applies at once. Without such an edit the whole store applies,
    /// as a change of another window always did.
    fn adopt_reloaded_settings(&mut self, config: &Config) {
        let Some(editor) = self.settings.as_mut() else {
            self.apply_runtime_config(config);
            self.board.sync_workspace_metadata(config);
            return;
        };
        let mut sections: Vec<String> = editor
            .conflict
            .take()
            .map(|conflict| conflict.sections)
            .unwrap_or_default();
        let changed = horizon_core::settings_sync::changed_sections(&editor.original, config);
        let mut draft = editor.config.clone();
        for section in changed {
            let edited = horizon_core::settings_sync::body_of(&editor.config, &section).ok()
                != horizon_core::settings_sync::body_of(&editor.original, &section).ok();
            if edited || sections.contains(&section) {
                if !sections.contains(&section) {
                    sections.push(section);
                }
                continue;
            }
            let Some(value) = horizon_core::settings_sync::body_of(config, &section).ok().flatten() else {
                continue;
            };
            if let Ok(next) = horizon_core::settings_sync::with_section(&draft, &section, &value) {
                draft = next;
            }
        }
        // A section that the person took, or that the draft already holds the value
        // of, is no conflict any more.
        sections.retain(|section| {
            horizon_core::settings_sync::body_of(&draft, section).ok()
                != horizon_core::settings_sync::body_of(config, section).ok()
        });
        editor.original = config.clone();
        editor.config = draft;
        editor.dirty = draft_dirty(&editor.config, &editor.original);
        editor.conflict = None;
        if !sections.is_empty() {
            editor.conflict = Some(SettingsConflict {
                sections,
                stored: Box::new(config.clone()),
            });
            editor.status = SettingsStatus::LivePreview;
        } else if editor.dirty {
            editor.status = SettingsStatus::LivePreview;
        } else {
            editor.status = SettingsStatus::None;
        }
        let draft = editor.config.clone();
        self.apply_live_preview(&draft);
    }

    fn save_settings(&mut self) {
        let Some(config) = self.settings.as_ref().map(|editor| editor.config.clone()) else {
            return;
        };
        // The sections that this save changes, so the worker queues one intent for
        // each of them after the store is written. The store of the window is the
        // baseline, not the live draft: a value that arrived from another computer
        // is already in the store and is no change of this window.
        let stored = self
            .settings
            .as_ref()
            .map_or_else(|| self.template_config.clone(), |editor| editor.original.clone());
        let shared = horizon_core::settings_sync::changed_sections(&stored, &config);
        // The draft is checked here, in memory; the write waits on the settings
        // thread, and a reply applies the saved settings in a later frame.
        match config.validate() {
            Ok(()) => {
                self.apply_runtime_config(&config);
                self.settings_io.save_config(self.config_path.clone(), config, shared);
                if let Some(editor) = self.settings.as_mut() {
                    editor.status = SettingsStatus::Saving;
                }
            }
            Err(error) => {
                if let Some(editor) = self.settings.as_mut() {
                    editor.status = SettingsStatus::Error(error.to_string());
                }
                tracing::error!("failed to validate the settings before the save: {error}");
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
                SettingsReply::Saved { path, config } => {
                    if let Some(editor) = self.settings.as_mut() {
                        editor.adopt_saved((*config).clone());
                    }
                    self.template_config = *config;
                    // Another window may have reloaded the presets meanwhile.
                    self.board.sync_workspace_metadata(&self.template_config);
                    tracing::info!(path = %path.display(), "settings saved");
                }
                SettingsReply::Probed { mtime } => {
                    self.config_probe_pending = false;
                    if !self.config_mtime_known {
                        // The first report of this run is the baseline: nothing
                        // changed while the window opened.
                        self.config_mtime_known = true;
                        self.config_last_mtime = mtime;
                    } else if mtime != self.config_last_mtime && !self.settings_reload_pending {
                        self.config_last_mtime = mtime;
                        self.settings_reload_pending = true;
                        self.settings_io.reload(self.config_path.clone());
                    }
                }
                SettingsReply::Reloaded { path, config } => {
                    self.settings_reload_pending = false;
                    if let Some(config) = config {
                        tracing::info!(path = %path.display(), "settings changed, applying them");
                        self.adopt_reloaded_settings(&config);
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
        self.apply_speech_config(&config.speech);
        if self.template_config.browser.remote != config.browser.remote {
            self.remote_browser_credentials
                .load_environment_bindings(&config.browser.remote);
            if let Some(sync) = self.server_sync.as_ref() {
                sync.refresh_credentials(&config.browser.remote);
            }
        }
        self.template_config = config.clone();
        self.shortcuts = resolve_shortcuts(config);
        self.action_commands_cache =
            crate::command_registry::action_commands(&self.shortcuts, util::primary_shortcut_label());
        self.presets = config.ssh_presets(self.ssh_hosts.hosts());
        if self.appearance_theme != config.appearance.theme {
            self.appearance_theme = config.appearance.theme;
            self.theme_applied = false;
        }
    }
}

pub(super) fn settings_panel_default_width(viewport_width: f32) -> f32 {
    (viewport_width * 0.3).clamp(340.0, 900.0)
}

/// The rectangle that the Settings panel takes from `ui`, before any tab draws.
/// The panel sizes itself from this rectangle and the canvas takes only what is
/// left of `ui`, so one place decides the edge between them: no tab can paint
/// over the canvas, and the canvas cannot paint over the panel, whatever the
/// content of the tab asks for.
///
/// A right panel takes the width of its widest child, and egui moves the left
/// edge of a panel that overflows instead of growing it, so a rectangle that a
/// tab could widen would move the canvas into the panel.
pub(super) fn panel_rect(ui: &egui::Ui) -> egui::Rect {
    let available = ui.available_rect_before_wrap();
    let width = settings_panel_default_width(util::viewport_local_rect(ui).width()).min(available.width());
    egui::Rect::from_min_max(egui::Pos2::new(available.max.x - width, available.min.y), available.max)
}

/// The line of the footer while a value of another computer waits for the person.
pub(super) const CONFLICT_LINE: &str = "This setting changed on another computer.";

fn settings_status(status: &SettingsStatus) -> (String, Color32) {
    match status {
        SettingsStatus::None => (String::new(), theme::FG_DIM()),
        SettingsStatus::LivePreview => ("Live preview".to_string(), theme::FG_DIM()),
        SettingsStatus::Saving => ("Saving".to_string(), theme::FG_DIM()),
        SettingsStatus::Saved => ("Saved".to_string(), theme::PALETTE_GREEN()),
        SettingsStatus::Error(message) => (message.clone(), theme::PALETTE_RED()),
    }
}

/// What the Settings panel shows besides its tabs: the status bar and whether
/// the draft differs from the store.
#[derive(Clone)]
struct PanelState<'a> {
    status_text: &'a str,
    status_color: Color32,
    has_changes: bool,
    /// The sections of this window that a value of another computer reached while
    /// the person edits them.
    conflict: Option<String>,
}

fn render_settings_panel(
    ui: &mut egui::Ui,
    panel_state: &PanelState<'_>,
    editor: &mut SettingsEditor,
    workbenches: &mut TabWorkbenches<'_>,
    view: &server::ServerView,
    general_view: general::GeneralView<'_>,
) -> (SettingsAction, bool, speech::SpeechRequest) {
    let rect = panel_rect(ui);
    // The pane is its own area on the top layer: every panel of the canvas is an area
    // of its own, and the area of a panel covers the pane for the pointer, so a wheel
    // over the pane would reach the terminal behind it and not the settings.
    let mut pane = None;
    egui::Area::new(egui::Id::unique("settings_pane"))
        .fixed_pos(rect.min)
        .default_size(rect.size())
        .order(egui::Order::Tooltip)
        // The pane is no overlay that appears over the work: it takes its edge of the
        // window at once, as the panel did before it had a layer of its own.
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                pane = Some(
                    egui::Panel::right(SETTINGS_PANEL_ID)
                        // One width for every tab, and the same width that the panel
                        // opens with. The rectangle comes from `panel_rect`, which the
                        // canvas reads as well, so a tab with a long path or a long
                        // secret name cannot move the edge between the panel and the
                        // canvas. The content wraps, scrolls or is cut inside this
                        // width instead.
                        .exact_size(rect.width())
                        .frame(
                            egui::Frame::default()
                                .fill(theme::BG_ELEVATED())
                                .inner_margin(Margin::symmetric(SETTINGS_PANEL_MARGIN_X, 16))
                                .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE())),
                        )
                        .show(ui, |ui| render_settings_content(ui, panel_state, editor, workbenches, view, general_view))
                        .inner,
                );
            });
        });
    pane.unwrap_or_else(|| (SettingsAction::None, false, speech::SpeechRequest::default()))
}

/// The content of the pane: its bar, its title, its tabs and the active tab.
fn render_settings_content(
    ui: &mut egui::Ui,
    panel_state: &PanelState<'_>,
    editor: &mut SettingsEditor,
    workbenches: &mut TabWorkbenches<'_>,
    view: &server::ServerView,
    general_view: general::GeneralView<'_>,
) -> (SettingsAction, bool, speech::SpeechRequest) {
    // The panel is a fraction of the viewport, so its content draws inside its own
    // rect: a wide child scrolls or clips instead of painting over the canvas beside it.
    ui.shrink_clip_rect(ui.max_rect());
    // The bar belongs to this panel, so the status line and its buttons sit with the
    // settings instead of under the canvas.
    let action = bar::render(
        ui,
        panel_state.status_text,
        panel_state.status_color,
        panel_state.has_changes,
        panel_state.conflict.as_deref(),
    );
    ui.label(crate::typeface::rich(
        "Settings",
        18.0,
        theme::FG(),
        crate::typeface::Weight::Semibold,
    ));
    ui.add_space(16.0);

    render_tab_bar(ui, editor);
    ui.add_space(16.0);

    let available = ui.available_size() - Vec2::new(0.0, 8.0);
    let (changed, speech_request) = render_tab(ui, editor, workbenches, available, view, general_view);
    (action, changed, speech_request)
}

/// Draw the active tab. Returns whether it changed the draft.
/// The workers of the tabs, which the app holds beside the editor.
struct TabWorkbenches<'a> {
    credentials: &'a mut CredentialWorkbench,
    allocations: &'a mut horizon_core::browser::remote_recovery::RemoteAllocations,
    named_secrets: &'a mut NamedSecretWorkbench,
    keyring: &'a mut KeyringImport,
    speech: &'a speech::SpeechView<'a>,
}

fn render_tab(
    ui: &mut egui::Ui,
    editor: &mut SettingsEditor,
    workbenches: &mut TabWorkbenches<'_>,
    available: Vec2,
    view: &server::ServerView,
    general_view: general::GeneralView<'_>,
) -> (bool, speech::SpeechRequest) {
    let TabWorkbenches {
        credentials,
        allocations,
        named_secrets,
        keyring,
        speech: speech_view,
    } = workbenches;
    let SettingsEditor {
        config,
        credential_inputs,
        portable_profile,
        provider_usage,
        secret_inputs,
        server,
        active_tab,
        ..
    } = editor;

    egui::ScrollArea::vertical()
        .max_height(available.y)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // The width that the scroll area gives is the width of the panel, with
            // the scroll bar taken out. A tab that sized itself from the width
            // before the bar would be wider than the panel, and the panel would
            // take the width of its widest tab.
            ui.set_width(ui.available_width());
            match active_tab {
                SettingsTab::General => (
                    general::render(ui, config, general_view),
                    speech::SpeechRequest::default(),
                ),
                SettingsTab::Shortcuts => (shortcuts::render(ui, config), speech::SpeechRequest::default()),
                SettingsTab::Presets => (presets::render(ui, config), speech::SpeechRequest::default()),
                SettingsTab::Browser => (browser::render(ui, config), speech::SpeechRequest::default()),
                SettingsTab::RemoteBrowsers => {
                    remote_recovery::render(ui, allocations);
                    (
                        remote_browsers::render(
                            ui,
                            config,
                            credentials,
                            credential_inputs,
                            portable_profile,
                            provider_usage,
                            keyring,
                            named_secrets,
                        ),
                        speech::SpeechRequest::default(),
                    )
                }
                SettingsTab::Secrets => (
                    secrets::render(ui, config, named_secrets, secret_inputs),
                    speech::SpeechRequest::default(),
                ),
                SettingsTab::Server => (
                    server::render(ui, config, server, view),
                    speech::SpeechRequest::default(),
                ),
                SettingsTab::Speech => {
                    let outcome = speech::render(ui, config, named_secrets, speech_view);
                    (
                        outcome.changed,
                        speech::SpeechRequest {
                            try_take: outcome.try_take,
                            retry_take: outcome.retry_take,
                        },
                    )
                }
            }
        })
        .inner
}

/// The tab row. It is compact on purpose: one row of short labels at the default
/// width of the panel of the smallest window that this build opens (1480 points
/// gives 396 points of content), so the person never reads a tab on a second line.
fn render_tab_bar(ui: &mut egui::Ui, editor: &mut SettingsEditor) {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = Vec2::new(TAB_BUTTON_PADDING, 5.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(TAB_ITEM_GAP, 6.0);
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

                let btn = egui::Button::new(egui::RichText::new(tab.label()).size(TAB_LABEL_SIZE).color(text_color))
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(TAB_CORNER_RADIUS);

                if ui.add(btn).clicked() {
                    editor.active_tab = tab;
                }
            }
        });
    });
}

/// The width that the tab row needs: every label, the padding of its button and
/// the gaps between the buttons. The tab row must fit the content of the default
/// panel, so this is what the test of the layout measures.
#[cfg(test)]
fn tab_row_width(mut measure: impl FnMut(&str) -> f32, labels: &[&str]) -> f32 {
    let buttons: f32 = labels
        .iter()
        .map(|label| measure(label) + 2.0 * TAB_BUTTON_PADDING)
        .sum();
    let gaps = u16::try_from(labels.len().saturating_sub(1)).unwrap_or(u16::MAX);
    buttons + TAB_ITEM_GAP * f32::from(gaps)
}

// -- Shared section helpers used by tab modules --------------------------

// -- Fields and their rules, shared by the tabs --------------------------

/// One field that a person may leave blank, and writes back only when the text
/// changed, so no frame writes a value that a person did not type.
fn text_field(ui: &mut egui::Ui, value: &mut String, hint: &str, id: &str) -> bool {
    let mut text = value.clone();
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text(hint)
            .desired_width((ui.available_width() - ui.spacing().item_spacing.x).max(120.0))
            .id_salt(id),
    );
    if !response.changed() {
        return false;
    }
    *value = text;
    true
}

/// One field of an entry that a person may leave blank. The box edits a copy,
/// so a blank box means the entry is not set; an entry that is not set gives an
/// empty copy, so the common case allocates nothing.
///
/// The box keeps the text as the person typed it, spaces included. A trim of the
/// draft of every frame would end a word at each space: the space is the last
/// character of the draft until the next key of the word arrives, so a feed that
/// writes the trimmed draft back takes that space away. The value is trimmed when
/// the field is left, and a box of nothing but spaces is an entry that is not set.
fn optional_text_field(ui: &mut egui::Ui, value: &mut Option<String>, hint: &str, id: &str) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text(hint)
            .desired_width((ui.available_width() - ui.spacing().item_spacing.x).max(120.0))
            .id_salt(id),
    );
    let typed = if response.lost_focus() {
        text.trim().to_owned()
    } else {
        text
    };
    let next = (!typed.trim().is_empty()).then_some(typed);
    if next == *value {
        return false;
    }
    *value = next;
    true
}

/// The refusal of a value, in the words of its rule. A value that the engine
/// refuses is shown under its section, and Save reports it as well.
fn validation_notice(ui: &mut egui::Ui, result: Result<(), impl std::fmt::Display>) {
    if let Err(error) = result {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(error.to_string())
                .color(theme::PALETTE_RED())
                .size(11.0),
        );
    }
}

/// One row whose actions sit at its right end. The actions are placed from the
/// right first and the content takes the width that they leave, so a field of
/// the row never pushes an action past the panel beside the canvas, whatever the
/// text of a button is and whatever the width of the panel.
fn action_row(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui), actions: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_wrapped(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            actions(ui);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), content);
        });
    });
}

fn section_heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(crate::typeface::rich(
        title,
        13.0,
        theme::FG_SOFT(),
        crate::typeface::Weight::Semibold,
    ));
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

/// Whether the draft of the Settings window differs from the store in one section.
fn draft_dirty(draft: &Config, stored: &Config) -> bool {
    !horizon_core::settings_sync::changed_sections(stored, draft).is_empty()
        || horizon_core::settings_sync::SECTIONS.iter().any(|section| {
            horizon_core::settings_sync::body_of(draft, section.name).ok()
                != horizon_core::settings_sync::body_of(stored, section.name).ok()
        })
}
