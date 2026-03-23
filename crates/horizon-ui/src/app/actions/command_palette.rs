use egui::Context;

use crate::app::HorizonApp;
use crate::app::shortcuts::shortcut_pressed;
use crate::command_palette::{CommandPalette, PaletteAction};
use crate::command_registry::CommandId;
use crate::search_overlay::SearchOverlay;

use super::align_attached_workspaces;
use super::support::{
    command_palette_panel_entries, command_palette_preset_entries, command_palette_workspace_entries,
    detached_workspace_ids,
};

impl HorizonApp {
    pub(in crate::app) fn open_command_palette(&mut self) {
        self.command_palette = Some(CommandPalette::new());
    }

    fn toggle_command_palette(&mut self) {
        self.command_palette = if self.command_palette.is_some() {
            None
        } else {
            Some(CommandPalette::new())
        };
    }

    pub(in crate::app) fn render_command_palette(&mut self, ctx: &Context) {
        let Some(palette) = self.command_palette.as_mut() else {
            return;
        };

        let detached_workspace_ids = detached_workspace_ids(&self.board, &self.detached_workspaces);
        let workspace_entries =
            command_palette_workspace_entries(&self.board, &detached_workspace_ids, self.board.active_workspace);
        let panel_entries = command_palette_panel_entries(&self.board, &detached_workspace_ids);
        let preset_entries = command_palette_preset_entries(&self.presets);

        let action = palette.show(
            ctx,
            &workspace_entries,
            &panel_entries,
            &preset_entries,
            &self.action_commands_cache,
        );
        match action {
            PaletteAction::None => {}
            PaletteAction::Cancelled => self.command_palette = None,
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
            CommandId::FocusPanel(panel_id) => {
                self.board.focus(panel_id);
                if let Some(workspace_id) = self.board.panel(panel_id).map(|panel| panel.workspace_id)
                    && let Some((min, max)) = self.board.workspace_bounds(workspace_id)
                {
                    self.focus_workspace_bounds(ctx, min, max, true);
                }
            }
            CommandId::FocusActiveWorkspace => {
                let _ = self.focus_active_workspace(ctx, false);
            }
            CommandId::FitActiveWorkspace => {
                let _ = self.fit_active_workspace(ctx);
            }
            CommandId::ToggleSidebar => self.sidebar_visible = !self.sidebar_visible,
            CommandId::ToggleHud => self.hud_visible = !self.hud_visible,
            CommandId::ToggleMinimap => self.minimap_visible = !self.minimap_visible,
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
                let canvas_rect = self.canvas_rect(ctx);
                let _ = self.zoom_reset(canvas_rect, canvas_rect.center());
            }
            CommandId::ZoomIn => {
                let canvas_rect = self.canvas_rect(ctx);
                let _ = self.zoom_canvas_at(canvas_rect, canvas_rect.center(), self.canvas_view.zoom * 1.1);
            }
            CommandId::ZoomOut => {
                let canvas_rect = self.canvas_rect(ctx);
                let _ = self.zoom_canvas_at(canvas_rect, canvas_rect.center(), self.canvas_view.zoom / 1.1);
            }
            CommandId::AlignWorkspacesHorizontally => {
                if let Some(workspace_id) = align_attached_workspaces(&mut self.board, &self.detached_workspaces)
                    && let Some((min, max)) = self.board.workspace_bounds(workspace_id)
                {
                    self.focus_workspace_bounds(ctx, min, max, true);
                    self.mark_runtime_dirty();
                }
            }
            CommandId::NewPanel => {
                let workspace_id = self.ensure_workspace_visible(ctx);
                if let Some(preset) = self.presets.first().cloned() {
                    self.add_panel_to_workspace(workspace_id, preset, None);
                } else {
                    self.create_panel(ctx);
                }
            }
            CommandId::OpenRemoteHosts => self.toggle_remote_hosts_overlay(ctx),
            CommandId::CreatePanelFromPreset(index) => {
                if let Some(preset) = self.presets.get(index).cloned() {
                    let workspace_id = self
                        .board
                        .active_workspace
                        .unwrap_or_else(|| self.ensure_workspace_visible(ctx));
                    self.add_panel_to_workspace(workspace_id, preset, None);
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
        let shortcut_bindings: &[(_, CommandId)] = &[
            (self.shortcuts.zoom_reset, CommandId::ZoomReset),
            (self.shortcuts.zoom_in, CommandId::ZoomIn),
            (self.shortcuts.zoom_out, CommandId::ZoomOut),
            (self.shortcuts.focus_active_workspace, CommandId::FocusActiveWorkspace),
            (self.shortcuts.fit_active_workspace, CommandId::FitActiveWorkspace),
            (
                self.shortcuts.align_workspaces_horizontally,
                CommandId::AlignWorkspacesHorizontally,
            ),
            (self.shortcuts.toggle_settings, CommandId::ToggleSettings),
            (self.shortcuts.toggle_sidebar, CommandId::ToggleSidebar),
            (self.shortcuts.toggle_hud, CommandId::ToggleHud),
            (self.shortcuts.toggle_minimap, CommandId::ToggleMinimap),
            (self.shortcuts.open_remote_hosts, CommandId::OpenRemoteHosts),
            (self.shortcuts.new_terminal, CommandId::NewPanel),
            (self.shortcuts.search, CommandId::ToggleSearch),
        ];

        let (toggle_palette, triggered_command) = ctx.input(|input| {
            let palette = shortcut_pressed(input, self.shortcuts.command_palette);
            let command = shortcut_bindings
                .iter()
                .find(|(binding, _)| shortcut_pressed(input, *binding))
                .map(|(_, id)| id.clone());
            (palette, command)
        });

        if toggle_palette {
            self.toggle_command_palette();
        }
        if let Some(command_id) = triggered_command {
            self.execute_command(ctx, &command_id);
        }
    }
}
