use egui::{Context, Vec2};

use crate::app::HorizonApp;
use crate::app::shortcuts::shortcut_pressed;

impl HorizonApp {
    pub(in crate::app) fn handle_fullscreen_toggle(&mut self, ctx: &Context) {
        let (panel_toggle, window_toggle, exit_fullscreen) = ctx.input(|input| {
            (
                shortcut_pressed(input, self.shortcuts.fullscreen_panel),
                shortcut_pressed(input, self.shortcuts.fullscreen_window),
                shortcut_pressed(input, self.shortcuts.exit_fullscreen_panel),
            )
        });

        if window_toggle {
            let is_fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
        } else if panel_toggle {
            self.fullscreen_panel = if self.fullscreen_panel.is_some() {
                None
            } else {
                self.board.focused
            };
        } else if exit_fullscreen && self.fullscreen_panel.is_some() {
            self.fullscreen_panel = None;
        }

        if let Some(panel_id) = self.fullscreen_panel
            && self.board.panel(panel_id).is_none()
        {
            self.fullscreen_panel = None;
        }
    }

    #[profiling::function]
    pub(in crate::app) fn handle_canvas_pan(&mut self, ctx: &Context) {
        let canvas_rect = self.canvas_rect(ctx);
        let (pointer_position, middle_down, primary_down, space_down, modifiers, scroll, pointer_delta, zoom_delta) =
            ctx.input(|input| {
                (
                    input.pointer.hover_pos(),
                    input.pointer.middle_down(),
                    input.pointer.primary_down(),
                    input.key_down(egui::Key::Space),
                    input.modifiers,
                    input.smooth_scroll_delta + input.raw_scroll_delta,
                    input.pointer.delta(),
                    input.zoom_delta(),
                )
            });
        let pointer_in_canvas = pointer_position.is_some_and(|position| canvas_rect.contains(position));
        if pointer_in_canvas && (zoom_delta - 1.0).abs() > f32::EPSILON {
            let anchor = pointer_position.unwrap_or_else(|| canvas_rect.center());
            if self.zoom_canvas_at(canvas_rect, anchor, self.canvas_view.zoom * zoom_delta) {
                self.clear_terminal_selections();
            }
            self.is_panning = false;
            return;
        }

        let ctrl_or_cmd = modifiers.ctrl || modifiers.command;

        // Gesture lifecycle: active once middle-click starts a pan on empty
        // canvas, cleared when the middle button is released.
        // Ctrl+middle-click overrides and pans even over terminal panels.
        if !middle_down {
            self.middle_pan_active = false;
        }
        let pointer_over_panel_body =
            pointer_position.is_some_and(|pos| self.panel_screen_rects.values().any(|rect| rect.contains(pos)));
        if middle_down && !self.middle_pan_active && pointer_in_canvas {
            if ctrl_or_cmd || !pointer_over_panel_body {
                self.middle_pan_active = true;
            }
        }
        let drag_panning = pointer_in_canvas && (self.middle_pan_active || (space_down && primary_down));
        let pointer_over_panel = pointer_position.is_some_and(|position| {
            pointer_in_canvas
                && !drag_panning
                && scroll != Vec2::ZERO
                && !ctrl_or_cmd
                && self.panel_screen_rects.values().any(|rect| rect.contains(position))
        });
        let pan_delta = if drag_panning {
            pointer_delta
        } else if pointer_in_canvas && !pointer_over_panel && (!ctrl_or_cmd || pointer_over_panel_body) {
            if modifiers.shift && scroll.x == 0.0 {
                Vec2::new(scroll.y, 0.0)
            } else {
                scroll
            }
        } else {
            Vec2::ZERO
        };

        self.is_panning = pan_delta != Vec2::ZERO;
        if self.is_panning {
            self.pan_target = None;
            let mut pan_offset = Vec2::new(self.canvas_view.pan_offset[0], self.canvas_view.pan_offset[1]);
            pan_offset += pan_delta;
            self.canvas_view.set_pan_offset([pan_offset.x, pan_offset.y]);
            self.mark_runtime_dirty();
            self.clear_terminal_selections();
        }
    }

    fn clear_terminal_selections(&self) {
        for panel in &self.board.panels {
            if let Some(terminal) = panel.terminal() {
                terminal.clear_selection();
            }
        }
    }
}
