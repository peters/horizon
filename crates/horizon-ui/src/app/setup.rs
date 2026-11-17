//! Set up: the one action that starts the agent which does the work of the
//! wizard, and the notice that Horizon draws itself when it cannot.
//!
//! Set up starts the agent preset that the settings name first among the agent
//! programs that are installed on this computer, in the order of Settings,
//! Presets; the rule and the first message live in [`horizon_core::setup`]. The
//! panel opens in the active workspace, or in a new one when there is none, it
//! takes the keyboard, and a second press reveals that panel instead of starting
//! a second one.
//!
//! The search for the agent programs reads folders, so it runs on the
//! coordination worker and no frame waits for it.

pub(crate) mod notice;

use egui::Context;
use horizon_browser_control::manifest::now_millis;
use horizon_core::setup::{FolderPanel, SetupAgent, folder_panel, setup_agents};
use horizon_core::{BlockPanelState, Panel, PanelId, PanelKind, PanelOptions, PresetConfig, SetupGoal};

use super::HorizonApp;
use crate::agent_integrations;

use self::notice::ProgramRow;

/// What Set up remembers between presses in one window.
#[derive(Default)]
pub(in crate::app) struct SetupState {
    /// The goal of the last press, which a press on Look again repeats.
    pub(in crate::app) goal: SetupGoal,
    /// The panel of the agent that Set up started, while it lives.
    panel: Option<PanelId>,
    /// The notice card that Horizon drew, while it lives.
    pub(in crate::app) notice_panel: Option<PanelId>,
    /// A search of the agent programs runs on the worker, so a press starts no
    /// second one.
    searching: bool,
    /// The programs of the last search, in the order of the settings.
    programs: Vec<ProgramRow>,
    /// Whether this run installs its agent integrations, so the agent that Set
    /// up starts has the Horizon skills.
    pub(in crate::app) integrations: bool,
}

impl SetupState {
    /// The state of a window of this process, with the integrations switch that
    /// its start read.
    pub(in crate::app) fn new() -> Self {
        Self {
            integrations: agent_integrations::installed_now(),
            ..Self::default()
        }
    }
}

impl HorizonApp {
    /// Set up: open the agent of this person, or reveal the one that Set up
    /// already opened, with `goal` in its first message.
    pub(in crate::app) fn press_set_up(&mut self, ctx: &Context, goal: SetupGoal) {
        self.setup.goal = goal;
        if let Some(panel) = self.setup_panel_to_reveal() {
            self.reveal_set_up_panel(ctx, panel);
            return;
        }
        if self.setup.searching {
            return;
        }
        // A card that cannot say why would leave the person with a press that
        // did nothing, and a search of programs would change nothing about it.
        if !self.setup.integrations {
            self.show_setup_notice(ctx, &[]);
            return;
        }
        let candidates: Vec<(SetupAgent, PresetConfig)> = setup_agents(&self.presets)
            .into_iter()
            .filter_map(|agent| {
                let preset = self.presets.get(agent.preset)?.clone();
                Some((agent, preset))
            })
            .collect();
        let programs: Vec<String> = candidates.iter().map(|(agent, _)| agent.program.clone()).collect();
        self.setup.searching = true;
        let opening = ctx.clone();
        self.browser_create_host.io.then(
            move || {
                programs
                    .iter()
                    .map(|program| horizon_core::setup::find_program(program, None).is_some())
                    .collect::<Vec<bool>>()
            },
            move |app, found| {
                app.setup.searching = false;
                let found = found.unwrap_or_else(|| vec![false; candidates.len()]);
                app.apply_setup_search(&opening, &candidates, &found);
            },
        );
    }

    /// The answer of the search: start the first agent program that this
    /// computer has, or draw the notice that says what is missing.
    fn apply_setup_search(&mut self, ctx: &Context, candidates: &[(SetupAgent, PresetConfig)], found: &[bool]) {
        let rows: Vec<ProgramRow> = candidates
            .iter()
            .zip(found)
            .map(|((agent, _), found)| ProgramRow {
                name: agent.display_name().to_owned(),
                program: agent.program.clone(),
                found: *found,
            })
            .collect();
        self.setup.programs.clone_from(&rows);
        // The candidates keep the order of the settings, so the first program
        // that this computer has is the agent of Set up.
        let chosen = candidates
            .iter()
            .zip(found)
            .find(|(_, found)| **found)
            .map(|((_, preset), _)| preset.clone());
        match chosen {
            Some(preset) => {
                self.start_set_up_agent(ctx, preset);
            }
            None => self.show_setup_notice(ctx, &rows),
        }
    }

    /// Opens the panel of `preset` with the first message of Set up and gives it
    /// the keyboard. Returns whether Horizon started one.
    fn start_set_up_agent(&mut self, ctx: &Context, mut preset: PresetConfig) -> bool {
        let goal = self.setup.goal;
        let Some(args) = horizon_core::first_message_args(preset.kind, &goal.message()) else {
            return false;
        };
        preset.args.extend(args);
        self.close_setup_notice();
        let workspace_id = self.ensure_workspace_visible(ctx);
        let before: Vec<PanelId> = self.board.panels.iter().map(|panel| panel.id).collect();
        self.add_panel_to_workspace(ctx, workspace_id, preset, None);
        let started = self
            .board
            .panels
            .iter()
            .find(|panel| !before.contains(&panel.id))
            .map(|panel| panel.id);
        // A workspace without a directory asks where to run first, and the
        // person's answer starts the panel of this same preset, with the same
        // first message.
        self.setup.panel = started;
        self.mark_runtime_dirty();
        started.is_some()
    }

    /// Draws the notice of Set up in the active workspace, or replaces the
    /// notice that is already there.
    fn show_setup_notice(&mut self, ctx: &Context, rows: &[ProgramRow]) {
        let document = notice::document(rows, self.setup.integrations);
        if let Some(existing) = self
            .setup
            .notice_panel
            .filter(|panel| self.board.panel(*panel).is_some())
        {
            if let Some(state) = self.board.panel_mut(existing).and_then(Panel::block_mut) {
                state.replace_document(document);
                state.updated_at_millis = now_millis();
            }
            self.reveal_panel_visible(ctx, existing);
            self.mark_runtime_dirty();
            return;
        }
        let workspace_id = self.ensure_workspace_visible(ctx);
        let size = crate::block_widget::default_card_size(&document, self.canvas_rect(ctx).width());
        let state = BlockPanelState {
            title: notice::TITLE.to_owned(),
            updated_at_millis: now_millis(),
            ..BlockPanelState::new(document)
        };
        let options = PanelOptions {
            kind: PanelKind::Block,
            block: Some(state),
            name: Some(notice::TITLE.to_owned()),
            size: Some(size),
            ..PanelOptions::default()
        };
        match self.create_panel_with_options(options, workspace_id) {
            Ok(panel) => {
                self.setup.notice_panel = Some(panel);
                self.reveal_panel_visible(ctx, panel);
                self.mark_runtime_dirty();
            }
            Err(error) => {
                tracing::error!("Set up could not draw its notice: {error}");
                self.set_sidebar_notice(format!("Set up could not open a card: {error}"));
            }
        }
    }

    /// Removes the notice of Set up, because an agent now does its work.
    fn close_setup_notice(&mut self) {
        if let Some(panel) = self.setup.notice_panel.take()
            && self.board.panel(panel).is_some()
        {
            self.close_panel(panel);
        }
    }

    /// A press on a button of a card that Horizon owns. The notice of Set up and
    /// the card that asks about a setting of this computer are such cards, so
    /// another card of an agent never reaches this.
    pub(in crate::app) fn press_host_card_button(&mut self, ctx: &Context, panel: PanelId, id: &str) {
        if self.press_setting_request(ctx, panel, id) {
            return;
        }
        if self.setup.notice_panel != Some(panel) {
            return;
        }
        match id {
            notice::LOOK_AGAIN => self.press_set_up(ctx, self.setup.goal),
            notice::OPEN_PRESETS => self.open_settings_presets(),
            _ => {}
        }
    }

    /// Reveals the panel that Set up opened, and forgets it when the person
    /// closed it.
    fn reveal_set_up_panel(&mut self, ctx: &Context, panel: PanelId) {
        if let Some(workspace_id) = self.board.panel(panel).map(|panel| panel.workspace_id) {
            self.reveal_new_panel(ctx, workspace_id, panel);
        }
    }

    /// The panel of Set up that still lives in this window, if any.
    fn setup_panel_to_reveal(&mut self) -> Option<PanelId> {
        let panel = self.setup.panel?;
        if self.board.panel(panel).is_some() {
            return Some(panel);
        }
        self.setup.panel = None;
        None
    }
}

/// The preset of the settings that a press which starts an agent runs: the first
/// agent preset, in the order of the settings, whose program this computer has.
///
/// `found` is the answer of the search of the programs that [`setup_agents`]
/// names, in its order, so the search runs on a worker and no frame reads a
/// folder. [`FolderPanel::Terminal`] says that this computer has no agent program.
pub(in crate::app) fn installed_agent(presets: &[PresetConfig], found: &[bool]) -> FolderPanel {
    let agents = setup_agents(presets);
    folder_panel(presets, |program| {
        agents
            .iter()
            .position(|agent| agent.program == program)
            .and_then(|place| found.get(place).copied())
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests;
