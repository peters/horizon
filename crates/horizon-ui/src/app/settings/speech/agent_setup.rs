//! Agent-assisted Speech Input setup card.
//!
//! The card deliberately owns only transient UI/probe state. Speech settings
//! remain part of the existing configuration, and launching the requested
//! panel is left to `HorizonApp` after the settings UI borrow has ended.

mod detection;
mod prompt;
mod saved_config;

use std::path::Path;

use egui::{Margin, Stroke, Ui};
use horizon_core::{Config, PanelKind, PresetConfig, SpeechConfig, agent_definition};

use detection::SpeechSetupAgentAvailability;
#[cfg(test)]
use detection::SpeechSetupProbeFailure;
pub(in crate::app) use prompt::speech_setup_prompt;
pub(in crate::app) use saved_config::validate_setup_saved_config;

use detection::AgentProbeCache;

use crate::theme;

const SAVE_OR_REVERT_MESSAGE: &str = "Save or revert your changes first.";

/// The two locally launched agents supported by Speech Input setup.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::app) enum SpeechSetupAgent {
    Codex,
    Claude,
}

impl SpeechSetupAgent {
    const ALL: [Self; 2] = [Self::Codex, Self::Claude];

    pub(in crate::app) const fn panel_kind(self) -> PanelKind {
        match self {
            Self::Codex => PanelKind::Codex,
            Self::Claude => PanelKind::Claude,
        }
    }

    pub(in crate::app) const fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
        }
    }

    pub(in crate::app) fn default_command(self) -> &'static str {
        agent_definition(self.panel_kind()).map_or("", |definition| definition.default_command)
    }

    pub(in crate::app) fn panel_title(self) -> String {
        format!("Speech Input Setup — {}", self.display_name())
    }
}

/// A launch request returned by the settings renderer. It carries the exact
/// preset whose command was probed, avoiding a detector/launcher mismatch
/// when a configuration contains more than one preset for an agent kind.
#[derive(Clone, Debug)]
pub(in crate::app) struct SpeechSetupRequest {
    pub(in crate::app) agent: SpeechSetupAgent,
    pub(in crate::app) preset: PresetConfig,
}

impl SpeechSetupRequest {
    pub(in crate::app) fn verify_command(&self, workspace_cwd: Option<&Path>) -> Result<(), String> {
        match detection::verify_preset_command(&self.preset, workspace_cwd) {
            SpeechSetupAgentAvailability::Available { .. } => Ok(()),
            SpeechSetupAgentAvailability::Missing => Err(format!(
                "{} is no longer available. Restore the executable, then rescan.",
                self.agent.display_name()
            )),
            SpeechSetupAgentAvailability::Checking => Err(format!(
                "{} availability is still being checked. Wait for detection or rescan.",
                self.agent.display_name()
            )),
            SpeechSetupAgentAvailability::Unknown(reason) => Err(format!(
                "Could not verify {} before launch: {} Rescan and try again.",
                self.agent.display_name(),
                reason.user_message()
            )),
        }
    }
}

/// Whether the current Speech Input configuration should show the prominent
/// setup card or the compact help row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SpeechSetupReadiness {
    BuildMissing,
    Disabled,
    ModelMissing,
    Ready,
}

impl SpeechSetupReadiness {
    pub(super) fn classify(built_with_speech: bool, speech: &SpeechConfig) -> Self {
        if !built_with_speech {
            return Self::BuildMissing;
        }
        if !speech.enabled {
            return Self::Disabled;
        }
        if !has_configured_models(speech) {
            return Self::ModelMissing;
        }
        Self::Ready
    }

    const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

fn has_configured_models(speech: &SpeechConfig) -> bool {
    if speech.profiles.is_empty() {
        !speech.model.trim().is_empty()
    } else {
        speech.profiles.iter().all(|profile| !profile.model.trim().is_empty())
    }
}

/// Save-state information supplied by the settings editor. The launch path
/// must re-check this gate after rendering, because a GUI edit can update the
/// YAML buffer in the same frame that produced a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) struct SpeechSetupLaunchGate {
    config_valid: bool,
    has_unsaved_changes: bool,
}

impl SpeechSetupLaunchGate {
    pub(in crate::app) const fn new(config_valid: bool, has_unsaved_changes: bool) -> Self {
        Self {
            config_valid,
            has_unsaved_changes,
        }
    }

    pub(in crate::app) const fn can_launch(self) -> bool {
        self.config_valid && !self.has_unsaved_changes
    }

    pub(in crate::app) const fn blocked_reason(self) -> Option<&'static str> {
        if self.can_launch() {
            None
        } else {
            Some(SAVE_OR_REVERT_MESSAGE)
        }
    }
}

/// Result of rendering the setup surface. Existing speech controls should be
/// rendered only when `show_manual_controls` is true.
#[derive(Debug)]
pub(in crate::app) struct SpeechSetupRenderResult {
    pub(in crate::app) request: Option<SpeechSetupRequest>,
    pub(in crate::app) show_manual_controls: bool,
}

/// Settings-session state for background agent detection and disclosure UI.
/// Nothing here is serialized or copied into the Horizon configuration.
pub(in crate::app) struct SpeechAgentSetupState {
    probes: AgentProbeCache,
    manual_expanded: bool,
    launch_error: Option<SpeechSetupLaunchError>,
}

enum SpeechSetupLaunchError {
    Transient(String),
    SavedConfig(String),
}

impl SpeechSetupLaunchError {
    fn message(&self) -> &str {
        match self {
            Self::Transient(message) | Self::SavedConfig(message) => message,
        }
    }
}

impl SpeechAgentSetupState {
    pub(in crate::app) fn new() -> Self {
        Self {
            probes: AgentProbeCache::new(),
            manual_expanded: false,
            launch_error: None,
        }
    }

    pub(in crate::app) fn render(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        built_with_speech: bool,
        launch_gate: SpeechSetupLaunchGate,
        workspace_cwd: Option<&Path>,
    ) -> SpeechSetupRenderResult {
        self.probes.sync(ui.ctx(), config, workspace_cwd);
        let readiness = SpeechSetupReadiness::classify(built_with_speech, &config.features.speech);
        let request = if readiness.is_ready() {
            self.render_compact(ui, config, launch_gate)
        } else {
            self.render_full(ui, config, readiness, launch_gate)
        };

        SpeechSetupRenderResult {
            request,
            show_manual_controls: readiness.is_ready() || self.manual_expanded,
        }
    }

    pub(in crate::app) fn rescan(&mut self) {
        self.probes.invalidate();
        self.clear_transient_launch_error();
    }

    pub(in crate::app) fn set_launch_error(&mut self, error: impl Into<String>) {
        self.launch_error = Some(SpeechSetupLaunchError::Transient(error.into()));
    }

    pub(in crate::app) fn set_saved_config_error(&mut self, error: impl Into<String>) {
        self.launch_error = Some(SpeechSetupLaunchError::SavedConfig(error.into()));
    }

    pub(in crate::app) fn clear_launch_error(&mut self) {
        self.launch_error = None;
    }

    pub(in crate::app) fn clear_transient_launch_error(&mut self) {
        if matches!(self.launch_error, Some(SpeechSetupLaunchError::Transient(_))) {
            self.launch_error = None;
        }
    }

    #[cfg(test)]
    pub(in crate::app) fn expand_manual_for_test(&mut self) {
        self.manual_expanded = true;
    }

    #[cfg(test)]
    pub(in crate::app) fn launch_error_message_for_test(&self) -> Option<&str> {
        self.launch_error.as_ref().map(SpeechSetupLaunchError::message)
    }

    #[cfg(test)]
    fn availability(&self, agent: SpeechSetupAgent) -> SpeechSetupAgentAvailability {
        self.probes.availability(agent)
    }

    fn render_full(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        readiness: SpeechSetupReadiness,
        launch_gate: SpeechSetupLaunchGate,
    ) -> Option<SpeechSetupRequest> {
        let mut request = None;
        egui::Frame::default()
            .fill(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.06))
            .stroke(Stroke::new(
                1.0_f32,
                theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), 0.38),
            ))
            .corner_radius(10)
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    egui::RichText::new("Set up Speech Input")
                        .color(theme::FG())
                        .size(14.0)
                        .strong(),
                );
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(full_card_description(readiness))
                            .color(theme::FG_SOFT())
                            .size(11.5),
                    )
                    .wrap(),
                );
                ui.add_space(8.0);

                request = self.render_agent_actions(ui, config, launch_gate, false);

                ui.add_space(6.0);
                let manual_label = if self.manual_expanded {
                    "Hide manual configuration"
                } else {
                    "Configure manually"
                };
                if ui
                    .add(egui::Button::new(
                        egui::RichText::new(manual_label).color(theme::FG_SOFT()).size(11.0),
                    ))
                    .clicked()
                {
                    self.manual_expanded = !self.manual_expanded;
                }
                self.render_launch_error(ui);
            });
        ui.add_space(8.0);
        request
    }

    fn render_compact(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        launch_gate: SpeechSetupLaunchGate,
    ) -> Option<SpeechSetupRequest> {
        let mut request = None;
        egui::Frame::default()
            .fill(theme::PANEL_BG_ALT())
            .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE()))
            .corner_radius(8)
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("Get setup help")
                            .color(theme::FG_SOFT())
                            .size(11.5)
                            .strong(),
                    );
                    request = self.render_agent_actions(ui, config, launch_gate, true);
                });
                self.render_launch_error(ui);
            });
        ui.add_space(8.0);
        request
    }

    fn render_agent_actions(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        launch_gate: SpeechSetupLaunchGate,
        compact: bool,
    ) -> Option<SpeechSetupRequest> {
        let available = available_agents(&self.probes);
        let mut requested_agent = None;

        if available.is_empty() {
            self.render_empty_agent_state(ui);
        } else {
            ui.horizontal_wrapped(|ui| {
                for agent in available.iter().copied() {
                    let label = if compact {
                        agent.display_name().to_string()
                    } else {
                        format!("Set up with {}", agent.display_name())
                    };
                    let response = ui.add_enabled(
                        launch_gate.can_launch(),
                        egui::Button::new(
                            egui::RichText::new(label)
                                .color(if launch_gate.can_launch() {
                                    theme::FG()
                                } else {
                                    theme::FG_DIM()
                                })
                                .size(11.0),
                        )
                        .fill(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.16))
                        .stroke(Stroke::new(
                            1.0_f32,
                            theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), 0.5),
                        )),
                    );
                    let clicked = response.clicked();
                    if let Some(reason) = launch_gate.blocked_reason() {
                        response.on_hover_text(reason);
                    }
                    if clicked {
                        requested_agent = Some(agent);
                    }
                }
                if ui
                    .small_button("Rescan")
                    .on_hover_text("Look again for locally installed setup agents")
                    .clicked()
                {
                    self.rescan();
                    ui.ctx().request_repaint();
                }
            });
            if let Some(reason) = launch_gate.blocked_reason() {
                ui.add_space(3.0);
                dim_label(ui, reason);
            }
        }

        let agent = requested_agent?;
        self.clear_transient_launch_error();
        match (
            selected_setup_preset(config, agent),
            self.probes.resolved_command(agent),
        ) {
            (Some(mut preset), Some(command)) => {
                preset.command = Some(command);
                Some(SpeechSetupRequest { agent, preset })
            }
            (Some(_), None) => {
                self.set_launch_error(format!(
                    "{} detection completed without a resolved executable. Rescan and try again.",
                    agent.display_name()
                ));
                None
            }
            (None, _) => {
                self.set_launch_error(format!(
                    "No safe {} panel preset is available. Restore the default preset and rescan.",
                    agent.display_name()
                ));
                None
            }
        }
    }

    fn render_empty_agent_state(&mut self, ui: &mut Ui) {
        match availability_summary(&self.probes) {
            AvailabilitySummary::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    dim_label(ui, "Looking for Codex and Claude…");
                });
            }
            AvailabilitySummary::NoneFound => {
                dim_label(ui, "No supported setup agent found.");
            }
            AvailabilitySummary::Unknown => {
                dim_label(ui, "Horizon could not verify a supported setup agent.");
                for agent in SpeechSetupAgent::ALL {
                    if let SpeechSetupAgentAvailability::Unknown(reason) = self.probes.availability(agent) {
                        dim_label(ui, &format!("{}: {}", agent.display_name(), reason.user_message()));
                    }
                }
            }
            AvailabilitySummary::Available => {}
        }

        if ui.small_button("Rescan").clicked() {
            self.rescan();
            ui.ctx().request_repaint();
        }
    }

    fn render_launch_error(&self, ui: &mut Ui) {
        if let Some(error) = &self.launch_error {
            ui.add_space(5.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(error.message())
                        .color(theme::PALETTE_RED())
                        .size(11.0),
                )
                .wrap(),
            );
        }
    }
}

impl Default for SpeechAgentSetupState {
    fn default() -> Self {
        Self::new()
    }
}

/// Select the first configured preset of the requested kind. When the user
/// has no such preset, fall back to Horizon's centralized default config.
pub(super) fn selected_setup_preset(config: &Config, agent: SpeechSetupAgent) -> Option<PresetConfig> {
    config
        .presets
        .iter()
        .find(|preset| preset.kind == agent.panel_kind())
        .cloned()
        .or_else(|| {
            Config::default()
                .presets
                .into_iter()
                .find(|preset| preset.kind == agent.panel_kind())
        })
}

fn full_card_description(readiness: SpeechSetupReadiness) -> &'static str {
    match readiness {
        SpeechSetupReadiness::BuildMissing => {
            "This Horizon build has no speech support. A setup agent can inspect this machine, prepare a speech-enabled build, choose a local GGUF model, select a microphone, and configure Horizon. After download, audio processing stays local."
        }
        SpeechSetupReadiness::Disabled => {
            "Speech Input is currently disabled. A setup agent can inspect this machine, verify build support, choose a local GGUF model and microphone, and configure a working starter profile. After download, audio processing stays local."
        }
        SpeechSetupReadiness::ModelMissing => {
            "Speech Input still needs a local GGUF model. A setup agent can inspect this machine, recommend a suitable model, select a microphone, and finish the configuration. After download, audio processing stays local."
        }
        SpeechSetupReadiness::Ready => "Speech Input is configured.",
    }
}

fn available_agents(probes: &AgentProbeCache) -> Vec<SpeechSetupAgent> {
    SpeechSetupAgent::ALL
        .into_iter()
        .filter(|agent| {
            matches!(
                probes.availability(*agent),
                SpeechSetupAgentAvailability::Available { .. }
            )
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AvailabilitySummary {
    Checking,
    Available,
    NoneFound,
    Unknown,
}

fn availability_summary(probes: &AgentProbeCache) -> AvailabilitySummary {
    let states = SpeechSetupAgent::ALL.map(|agent| probes.availability(agent));
    if states
        .iter()
        .any(|state| matches!(state, SpeechSetupAgentAvailability::Available { .. }))
    {
        AvailabilitySummary::Available
    } else if states
        .iter()
        .any(|state| matches!(state, SpeechSetupAgentAvailability::Checking))
    {
        AvailabilitySummary::Checking
    } else if states
        .iter()
        .all(|state| matches!(state, SpeechSetupAgentAvailability::Missing))
    {
        AvailabilitySummary::NoneFound
    } else {
        AvailabilitySummary::Unknown
    }
}

fn dim_label(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).color(theme::FG_DIM()).size(10.5)).wrap());
}

#[cfg(test)]
mod tests;
