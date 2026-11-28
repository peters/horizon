//! The form of the first start page and the start that a tile of it names.
//!
//! A window that shows the orchestrator dock draws the page of the front desk, and a
//! window without it draws the page of four tiles. This file picks the form, draws
//! the page frame around it and starts what a person presses.

use egui::Context;
use horizon_core::setup::{FolderPanel, find_program, setup_agents};

use super::super::HorizonApp;
use super::super::setup;
use super::{WelcomeTile, no_agent_program, welcome_page_shows};

impl HorizonApp {
    /// Draws the first start page, while the board holds no workspace and no
    /// panel. The dock decides the form of the page: a window that shows the dock
    /// sends the person to the front desk, and a window without it names the four
    /// starts.
    pub(in crate::app) fn render_welcome_page(&mut self, ctx: &Context) {
        if !self.welcome_page_visible() {
            return;
        }
        if self.front_desk_page_shows() {
            self.render_front_desk_page(ctx);
        } else {
            self.render_tiles_page(ctx);
        }
    }

    /// Whether the page sends the person to the front desk: the dock is on and it
    /// took its column in this frame.
    ///
    /// A window that leaves less than 320 points of canvas beside the dock draws
    /// no dock, and then the page of four tiles stands unchanged, as it does while
    /// the setting is off or the environment switch is `0`.
    pub(in crate::app) fn front_desk_page_shows(&self) -> bool {
        self.orchestrator_dock.visible() && self.orchestrator_dock.frame_rect().is_some()
    }
    /// Starts what a tile of the first page names.
    pub(super) fn start_welcome_tile(&mut self, ctx: &Context, tile: WelcomeTile) {
        match tile.goal() {
            Some(goal) => self.press_set_up(ctx, goal),
            None => self.start_agent_tile(ctx),
        }
    }

    /// The first tile starts a panel of the first agent preset, in the order of
    /// the settings, whose program this computer has.
    ///
    /// A computer with no agent program keeps the first preset of the settings,
    /// and one line says that this is a terminal. The search reads the folders of
    /// the `PATH`, so it runs on the coordination worker and the panel opens on a
    /// later frame.
    fn start_agent_tile(&mut self, ctx: &Context) {
        if self.presets.is_empty() {
            self.create_panel(ctx);
            return;
        }
        let workspace_id = self.ensure_workspace_visible(ctx);
        let presets = self.presets.clone();
        let programs: Vec<String> = setup_agents(&presets).into_iter().map(|agent| agent.program).collect();
        let searched = programs.clone();
        let opening = ctx.clone();
        self.browser_create_host.io.then(
            move || {
                searched
                    .iter()
                    .map(|program| find_program(program, None).is_some())
                    .collect::<Vec<bool>>()
            },
            move |app, found| {
                let found = found.unwrap_or_default();
                let preset = match setup::installed_agent(&presets, &found) {
                    FolderPanel::Preset(index) => presets.get(index).cloned(),
                    FolderPanel::Terminal => {
                        app.set_sidebar_plain_notice(no_agent_program(&programs));
                        presets.first().cloned()
                    }
                };
                match preset {
                    Some(preset) => app.add_panel_to_workspace(&opening, workspace_id, preset, None),
                    None => app.create_panel(&opening),
                }
            },
        );
    }

    /// Whether the first page shows: no workspace, no panel, and no dialog over
    /// the canvas.
    pub(in crate::app) fn welcome_page_visible(&self) -> bool {
        welcome_page_shows(
            self.board.workspaces.len(),
            self.board.panels.len(),
            self.fixed_overlays_visible(),
        )
    }
}
