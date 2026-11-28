use egui::Context;

use crate::app::HorizonApp;
use crate::app::shortcuts::{consume_shortcut, shortcut_pressed};
use crate::command_palette::{CommandPalette, PaletteAction};
use crate::command_registry::CommandId;
use crate::search_overlay::SearchOverlay;

use super::support::{
    command_palette_panel_entries, command_palette_preset_entries, command_palette_workspace_entries,
    detached_workspace_ids, missing_programs,
};

impl HorizonApp {
    pub(in crate::app) fn open_command_palette(&mut self) {
        self.open_palette(CommandPalette::new());
    }

    /// The list of the presets alone, which a press that adds a panel to a
    /// workspace opens.
    pub(in crate::app) fn open_preset_palette(&mut self) {
        self.open_palette(CommandPalette::presets_only());
    }

    fn open_palette(&mut self, palette: CommandPalette) {
        if self.root_viewport_stabilization_blocks_interaction() {
            return;
        }
        self.refresh_preset_programs();
        self.command_palette = Some(palette);
    }

    /// Asks the worker which presets start a program that this computer does not
    /// have. The list draws unmarked until the answer is here.
    fn refresh_preset_programs(&mut self) {
        let programs: Vec<(usize, String)> = self
            .presets
            .iter()
            .enumerate()
            .filter(|(_, preset)| preset.kind.is_agent())
            .map(|(index, preset)| (index, horizon_core::setup::preset_program(preset)))
            .collect();
        if programs.is_empty() {
            return;
        }
        self.browser_create_host.io.then(
            move || missing_programs(&programs),
            |app, missing| app.preset_programs.missing = missing.unwrap_or_default(),
        );
    }

    /// The chord of the palette closes an open palette and opens a closed one as
    /// every other press does, with the search of the programs of its presets.
    fn toggle_command_palette(&mut self) {
        if self.command_palette.take().is_none() {
            self.open_command_palette();
        }
    }

    pub(in crate::app) fn render_command_palette(&mut self, ctx: &Context) {
        let Some(palette) = self.command_palette.as_mut() else {
            return;
        };

        let detached_workspace_ids = detached_workspace_ids(&self.board, &self.detached_workspaces);
        let workspace_entries =
            command_palette_workspace_entries(&self.board, &detached_workspace_ids, self.board.active_workspace);
        let panel_entries = command_palette_panel_entries(&self.board, &detached_workspace_ids);
        let preset_entries = command_palette_preset_entries(&self.presets, &self.preset_programs.missing);

        let action = palette.show(
            ctx,
            &workspace_entries,
            &panel_entries,
            &preset_entries,
            &self.action_commands_cache,
        );
        match action {
            PaletteAction::None => {}
            PaletteAction::Cancelled => {
                self.command_palette = None;
                if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                    self.consume_navigation_key(
                        ctx,
                        horizon_core::ShortcutBinding::new(
                            horizon_core::ShortcutModifiers::NONE,
                            horizon_core::ShortcutKey::Escape,
                        ),
                    );
                }
            }
            PaletteAction::Execute(cmd) => {
                self.command_palette = None;
                self.execute_command(ctx, &cmd);
            }
        }
    }

    pub(in crate::app) fn execute_command(&mut self, ctx: &Context, cmd: &CommandId) {
        match *cmd {
            CommandId::SwitchWorkspace(workspace_id) => {
                let _ = self.focus_workspace_visible(ctx, workspace_id, true);
            }
            CommandId::FocusPanel(panel_id) => self.reveal_selected_panel(ctx, panel_id),
            CommandId::FocusActiveWorkspace => {
                let _ = self.focus_active_workspace(ctx, false);
            }
            CommandId::FitActiveWorkspace => {
                let _ = self.fit_active_workspace(ctx);
            }
            CommandId::GoToNextAttention => self.go_to_next_attention(ctx),
            CommandId::ToggleSidebar => self.sidebar_visible = !self.sidebar_visible,
            CommandId::ToggleHud => self.hud_visible = !self.hud_visible,
            CommandId::ToggleMinimap => self.minimap_visible = !self.minimap_visible,
            CommandId::ToggleOrchestratorDock => self.orchestrator_dock.toggle(),
            CommandId::ShowFrontDesk => self.orchestrator_dock.front_desk_mut().show(),
            CommandId::ToggleFullscreenWindow => {
                let is_fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
            }
            CommandId::ToggleFullscreenPanel => {
                self.fullscreen_panel = if self.fullscreen_panel.is_some() {
                    None
                } else {
                    self.board.focused
                };
            }
            CommandId::ZoomReset => {
                let _ = self.zoom_by_shortcut(ctx, horizon_core::DEFAULT_CANVAS_ZOOM);
            }
            CommandId::ZoomIn => {
                let zoom = self.canvas_view.zoom * 1.1;
                let _ = self.zoom_by_shortcut(ctx, zoom);
            }
            CommandId::ZoomOut => {
                let zoom = self.canvas_view.zoom / 1.1;
                let _ = self.zoom_by_shortcut(ctx, zoom);
            }
            CommandId::AlignWorkspacesHorizontally => {
                // The shared action settles the view on the row head; the
                // interactive shortcut re-frames the workspace that was
                // active before the alignment instead.
                if self.align_attached_workspaces_horizontally(ctx).is_some() {
                    self.reframe_active_workspace_after_alignment(ctx);
                }
            }
            CommandId::NewPanel => {
                let workspace_id = self.ensure_workspace_visible(ctx);
                if let Some(preset) = self.presets.first().cloned() {
                    self.add_panel_to_workspace(ctx, workspace_id, preset, None);
                } else {
                    self.create_panel(ctx);
                }
            }
            CommandId::OpenRemoteHosts => self.toggle_remote_hosts_overlay(ctx),
            CommandId::ToggleSessions => self.toggle_session_manager(ctx),
            CommandId::CreatePanelFromPreset(index) => {
                if let Some(preset) = self.presets.get(index).cloned() {
                    let workspace_id = self
                        .board
                        .active_workspace
                        .unwrap_or_else(|| self.ensure_workspace_visible(ctx));
                    self.add_panel_to_workspace(ctx, workspace_id, preset, None);
                }
            }
            CommandId::ToggleSettings => self.toggle_settings(),
            CommandId::ToggleSearch => {
                // Focus the toolbar search input (or create it with focus
                // if it doesn't exist yet).
                if let Some(overlay) = &mut self.search_overlay {
                    overlay.focus();
                } else {
                    self.search_overlay = Some(SearchOverlay::new());
                }
            }
        }
    }

    pub(in crate::app) fn handle_shortcuts(&mut self, ctx: &Context) {
        if self.root_viewport_stabilization_blocks_interaction() {
            return;
        }
        let shortcut_bindings: &[(_, CommandId)] = &[
            (self.shortcuts.zoom_reset, CommandId::ZoomReset),
            (self.shortcuts.zoom_in, CommandId::ZoomIn),
            (self.shortcuts.zoom_out, CommandId::ZoomOut),
            (self.shortcuts.focus_active_workspace, CommandId::FocusActiveWorkspace),
            (self.shortcuts.fit_active_workspace, CommandId::FitActiveWorkspace),
            (self.shortcuts.next_attention, CommandId::GoToNextAttention),
            (
                self.shortcuts.align_workspaces_horizontally,
                CommandId::AlignWorkspacesHorizontally,
            ),
            (self.shortcuts.toggle_settings, CommandId::ToggleSettings),
            (self.shortcuts.toggle_sidebar, CommandId::ToggleSidebar),
            (self.shortcuts.toggle_hud, CommandId::ToggleHud),
            (self.shortcuts.toggle_minimap, CommandId::ToggleMinimap),
            (self.shortcuts.open_remote_hosts, CommandId::OpenRemoteHosts),
            (self.shortcuts.toggle_sessions, CommandId::ToggleSessions),
            (self.shortcuts.new_terminal, CommandId::NewPanel),
            (self.shortcuts.search, CommandId::ToggleSearch),
        ];

        let (toggle_palette, triggered_command) = ctx.input(|input| {
            let palette = shortcut_pressed(input, self.shortcuts.command_palette);
            let command = shortcut_bindings
                .iter()
                .find(|(binding, _)| shortcut_pressed(input, *binding))
                .map(|(binding, id)| (*binding, id.clone()));
            (palette, command)
        });

        if toggle_palette {
            self.toggle_command_palette();
            // The chord belongs to Horizon, also when the palette takes the focus.
            consume_shortcut(ctx, self.shortcuts.command_palette);
        }
        if let Some((binding, command_id)) = triggered_command {
            self.execute_command(ctx, &command_id);
            // The command may give the keyboard focus to a terminal, and that
            // terminal reads this frame after this point: it must not also type
            // the chord that ran the command.
            consume_shortcut(ctx, binding);
        }
    }
}
