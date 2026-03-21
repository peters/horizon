use std::collections::BTreeSet;

use egui::{
    Align, Button, Color32, Context, CornerRadius, Layout, Pos2, Rect, Stroke, TopBottomPanel, Vec2, ViewportBuilder,
    ViewportCommand, ViewportId,
};
use horizon_core::{CanvasViewState, WindowConfig, WorkspaceId};

use crate::{branding, theme};

use super::util::{chrome_button, primary_shortcut_label, viewport_local_rect};
use super::{DetachedWorkspaceViewportState, HorizonApp, TOOLBAR_HEIGHT, WS_BG_PAD, WS_TITLE_HEIGHT};

const DETACHED_WINDOW_OFFSET: f32 = 48.0;

impl HorizonApp {
    pub(super) fn workspace_is_detached(&self, workspace_id: WorkspaceId) -> bool {
        self.board
            .workspace(workspace_id)
            .is_some_and(|workspace| self.detached_workspaces.contains_key(&workspace.local_id))
    }

    pub(super) fn detach_workspace(&mut self, workspace_id: WorkspaceId) {
        let Some(workspace) = self.board.workspace(workspace_id) else {
            return;
        };
        if self.detached_workspaces.contains_key(&workspace.local_id) {
            return;
        }

        self.detached_workspaces.insert(
            workspace.local_id.clone(),
            DetachedWorkspaceViewportState::new(self.initial_detached_window_config(workspace_id)),
        );
        self.pending_detached_window_position_restore
            .insert(workspace.local_id.clone());
        self.mark_runtime_dirty();
    }

    pub(super) fn reattach_workspace(&mut self, workspace_id: WorkspaceId) {
        let Some(workspace) = self.board.workspace(workspace_id) else {
            return;
        };
        if self.detached_workspaces.remove(&workspace.local_id).is_some() {
            self.pending_detached_window_position_restore
                .remove(&workspace.local_id);
            self.mark_runtime_dirty();
        }
    }

    pub(super) fn focus_workspace_window(&self, ctx: &Context, workspace_id: WorkspaceId) -> bool {
        let Some(workspace) = self.board.workspace(workspace_id) else {
            return false;
        };
        if !self.detached_workspaces.contains_key(&workspace.local_id) {
            return false;
        }

        ctx.send_viewport_cmd_to(detached_viewport_id(&workspace.local_id), ViewportCommand::Focus);
        true
    }

    pub(super) fn render_detached_viewports(&mut self, ctx: &Context) {
        self.process_pending_detached_reattach();

        let local_ids: Vec<_> = self.detached_workspaces.keys().cloned().collect();
        let mut stale_local_ids = Vec::new();

        for local_id in local_ids {
            let Some(workspace_id) = self.board.workspace_id_by_local_id(&local_id) else {
                stale_local_ids.push(local_id);
                continue;
            };
            let Some(workspace) = self.board.workspace(workspace_id) else {
                stale_local_ids.push(local_id);
                continue;
            };
            let Some(window_config) = self
                .detached_workspaces
                .get(&local_id)
                .map(|state| state.window.clone())
            else {
                continue;
            };

            let viewport_id = detached_viewport_id(&local_id);
            // Consume the restore hint before rebuilding the child viewport so
            // native drags do not fight a stale saved outer position.
            let restore_window_position =
                consume_detached_position_restore(&mut self.pending_detached_window_position_restore, &local_id);
            let builder = detached_viewport_builder(&window_config, &workspace.name, restore_window_position);
            let local_id_for_viewport = local_id.clone();

            ctx.show_viewport_immediate(viewport_id, builder, |viewport_ctx, _class| {
                self.render_detached_workspace_window(viewport_ctx, workspace_id, &local_id_for_viewport);
            });
        }

        if !stale_local_ids.is_empty() {
            for local_id in stale_local_ids {
                self.detached_workspaces.remove(&local_id);
                self.pending_detached_window_position_restore.remove(&local_id);
            }
            self.mark_runtime_dirty();
        }
    }

    fn render_detached_workspace_window(&mut self, ctx: &Context, workspace_id: WorkspaceId, workspace_local_id: &str) {
        if ctx.input(|input| input.viewport().close_requested()) {
            // Keep the native window alive for the remainder of this pass.
            // Dropping the viewport immediately can make winit query a dead X11
            // handle before the backend prunes the child viewport on the next pass.
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.schedule_detached_workspace_reattach(workspace_local_id);
            ctx.request_repaint_of(ViewportId::ROOT);
            return;
        }
        self.sync_detached_window_config(ctx, workspace_local_id);
        self.fit_detached_canvas_view_once(ctx, workspace_id, workspace_local_id);

        let Some(workspace_name) = self
            .board
            .workspace(workspace_id)
            .map(|workspace| workspace.name.clone())
        else {
            self.detached_workspaces.remove(workspace_local_id);
            self.mark_runtime_dirty();
            return;
        };

        let saved_canvas_view = self.canvas_view;
        let saved_pan_target = self.pan_target;
        let saved_is_panning = self.is_panning;
        // Detached rendering must not overwrite root-window hit-testing or
        // close requests that were collected earlier in the frame.
        let saved_panel_screen_rects = std::mem::take(&mut self.panel_screen_rects);
        let saved_panel_screen_order = std::mem::take(&mut self.panel_screen_order);
        let saved_panels_to_close = std::mem::take(&mut self.panels_to_close);
        let saved_workspace_screen_rects = std::mem::take(&mut self.workspace_screen_rects);
        if !self.restore_detached_viewport_state(workspace_local_id) {
            self.panel_screen_rects = saved_panel_screen_rects;
            self.panel_screen_order = saved_panel_screen_order;
            self.panels_to_close = saved_panels_to_close;
            self.workspace_screen_rects = saved_workspace_screen_rects;
            return;
        }

        self.render_detached_toolbar(ctx, workspace_id, workspace_local_id, &workspace_name);

        let canvas_rect = detached_canvas_rect(ctx);
        let workspace_bounds = self.board.workspace_bounds_map();
        self.handle_canvas_pan_in_rect(ctx, canvas_rect);
        self.render_canvas(ctx);
        self.render_detached_workspace_backgrounds(ctx, &workspace_bounds, canvas_rect, workspace_id);
        self.render_panels_for_workspace(ctx, workspace_id);
        let _ = self.render_workspace_minimap(
            ctx,
            &workspace_bounds,
            workspace_id,
            canvas_rect,
            egui::Id::new(("detached_workspace_minimap", workspace_local_id)),
        );
        self.handle_workspace_file_drop(ctx, workspace_id, canvas_rect);
        if self.pan_target.is_some() {
            ctx.request_repaint();
        }

        self.persist_detached_viewport_state(workspace_local_id);

        self.canvas_view = saved_canvas_view;
        self.pan_target = saved_pan_target;
        self.is_panning = saved_is_panning;
        self.panels_to_close = saved_panels_to_close;
        self.panel_screen_rects = saved_panel_screen_rects;
        self.panel_screen_order = saved_panel_screen_order;
        self.workspace_screen_rects = saved_workspace_screen_rects;
    }

    fn fit_detached_canvas_view_once(&mut self, ctx: &Context, workspace_id: WorkspaceId, workspace_local_id: &str) {
        let fitted_canvas_view = self.detached_canvas_view(ctx, workspace_id);
        let Some(detached_state) = self.detached_workspaces.get_mut(workspace_local_id) else {
            return;
        };
        if !detached_state.initial_fit_pending {
            return;
        }

        detached_state.canvas_view = fitted_canvas_view;
        detached_state.pan_target = None;
        detached_state.is_panning = false;
        detached_state.initial_fit_pending = false;
    }

    fn render_detached_toolbar(
        &mut self,
        ctx: &Context,
        workspace_id: WorkspaceId,
        workspace_local_id: &str,
        workspace_name: &str,
    ) {
        let fit_shortcut = self
            .shortcuts
            .fit_active_workspace
            .display_label(primary_shortcut_label());
        let minimap_shortcut = self.shortcuts.toggle_minimap.display_label(primary_shortcut_label());
        let minimap_label = if self.minimap_visible {
            "Hide Minimap"
        } else {
            "Show Minimap"
        };

        TopBottomPanel::top(egui::Id::new(("detached_workspace_toolbar", workspace_local_id))).show(ctx, |ui| {
            ui.set_height(TOOLBAR_HEIGHT);
            ui.painter()
                .rect_filled(ui.max_rect(), CornerRadius::ZERO, theme::TITLEBAR_BG);
            ui.painter().line_segment(
                [
                    Pos2::new(ui.max_rect().min.x, ui.max_rect().max.y),
                    Pos2::new(ui.max_rect().max.x, ui.max_rect().max.y),
                ],
                Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 170)),
            );

            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(egui::RichText::new(workspace_name).color(theme::FG).size(13.5).strong());
                ui.label(
                    egui::RichText::new("Detached Workspace")
                        .color(theme::FG_DIM)
                        .size(10.5),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(
                            Button::new(
                                egui::RichText::new("Attach to Main Window")
                                    .size(11.5)
                                    .color(theme::FG_SOFT),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        self.schedule_detached_workspace_reattach(workspace_local_id);
                        ctx.request_repaint_of(ViewportId::ROOT);
                    }

                    if ui
                        .add(chrome_button("Fit Workspace").min_size(Vec2::new(126.0, 30.0)))
                        .on_hover_text(fit_shortcut.as_str())
                        .clicked()
                    {
                        let _ = self.fit_workspace_in_rect(workspace_id, detached_canvas_rect(ctx));
                    }

                    if ui
                        .add(chrome_button(minimap_label).min_size(Vec2::new(124.0, 30.0)))
                        .on_hover_text(minimap_shortcut.as_str())
                        .clicked()
                    {
                        self.minimap_visible = !self.minimap_visible;
                    }
                });
            });
        });
    }

    fn restore_detached_viewport_state(&mut self, workspace_local_id: &str) -> bool {
        let Some(detached_state) = self.detached_workspaces.get_mut(workspace_local_id) else {
            return false;
        };

        self.canvas_view = detached_state.canvas_view;
        self.pan_target = detached_state.pan_target;
        self.is_panning = detached_state.is_panning;
        self.panel_screen_rects = std::mem::take(&mut detached_state.panel_screen_rects);
        self.panel_screen_order = std::mem::take(&mut detached_state.panel_screen_order);
        true
    }

    fn persist_detached_viewport_state(&mut self, workspace_local_id: &str) {
        let Some(detached_state) = self.detached_workspaces.get_mut(workspace_local_id) else {
            return;
        };

        detached_state.canvas_view = self.canvas_view;
        detached_state.pan_target = self.pan_target;
        detached_state.is_panning = self.is_panning;
        detached_state.panel_screen_rects = std::mem::take(&mut self.panel_screen_rects);
        detached_state.panel_screen_order = std::mem::take(&mut self.panel_screen_order);
    }

    fn render_panels_for_workspace(&mut self, ctx: &Context, workspace_id: WorkspaceId) {
        self.panel_screen_rects.clear();
        self.panel_screen_order.clear();

        let workspaces: Vec<_> = self
            .board
            .workspaces
            .iter()
            .map(|workspace| {
                let (r, g, b) = workspace.accent();
                (workspace.id, workspace.name.clone(), Color32::from_rgb(r, g, b))
            })
            .collect();

        let mut panel_ids = self
            .board
            .workspace(workspace_id)
            .map(|workspace| workspace.panels.clone())
            .unwrap_or_default();
        let focused = self.board.focused;
        panel_ids.sort_by_key(|panel_id| Some(*panel_id) == focused);

        let canvas_rect = detached_canvas_rect(ctx);
        self.panels_to_close.clear();
        for (fallback_index, panel_id) in panel_ids.into_iter().enumerate() {
            if self.render_panel(ctx, canvas_rect, panel_id, fallback_index, &workspaces) {
                self.panels_to_close.push(panel_id);
            }
        }

        self.apply_panel_transitions();
        self.apply_pending_workspace_changes();
    }

    fn detached_canvas_view(&self, ctx: &Context, workspace_id: WorkspaceId) -> CanvasViewState {
        let canvas_rect = detached_canvas_rect(ctx);
        if let Some((min, max)) = self.board.workspace_bounds(workspace_id) {
            let pos = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
            let size = Vec2::new(
                max[0] - min[0] + 2.0 * WS_BG_PAD,
                max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
            );
            return CanvasViewState::new([40.0 - pos.x, canvas_rect.height() * 0.5 - (pos.y + size.y * 0.5)], 1.0);
        }

        self.board
            .workspace(workspace_id)
            .map_or(CanvasViewState::default(), |workspace| {
                CanvasViewState::new(
                    [
                        40.0 - workspace.position[0],
                        canvas_rect.height() * 0.5 - workspace.position[1],
                    ],
                    1.0,
                )
            })
    }

    fn initial_detached_window_config(&self, workspace_id: WorkspaceId) -> WindowConfig {
        let (width, height) = if let Some((min, max)) = self.board.workspace_bounds(workspace_id) {
            (
                (max[0] - min[0] + 2.0 * WS_BG_PAD + 80.0).clamp(800.0, 7680.0),
                (max[1] - min[1] + 2.0 * WS_BG_PAD + TOOLBAR_HEIGHT + 48.0).clamp(600.0, 4320.0),
            )
        } else {
            (960.0, 720.0)
        };

        WindowConfig {
            width,
            height,
            x: self.window_config.x.map(|x| x + DETACHED_WINDOW_OFFSET),
            y: self.window_config.y.map(|y| y + DETACHED_WINDOW_OFFSET),
        }
    }

    fn sync_detached_window_config(&mut self, ctx: &Context, workspace_local_id: &str) {
        let (inner_rect, outer_rect) = ctx.input(|input| (input.viewport().inner_rect, input.viewport().outer_rect));
        let Some(detached_state) = self.detached_workspaces.get_mut(workspace_local_id) else {
            return;
        };
        let window_config = &mut detached_state.window;

        let mut changed = false;
        if let Some(rect) = inner_rect {
            let new_w = rect.width();
            let new_h = rect.height();
            if (new_w - window_config.width).abs() > 1.0 || (new_h - window_config.height).abs() > 1.0 {
                window_config.width = new_w;
                window_config.height = new_h;
                changed = true;
            }
        }

        if let Some(pos) = outer_rect {
            let new_x = pos.min.x;
            let new_y = pos.min.y;
            let moved = window_config.x.is_none()
                || window_config.x.is_some_and(|x| (x - new_x).abs() > 1.0)
                || window_config.y.is_none()
                || window_config.y.is_some_and(|y| (y - new_y).abs() > 1.0);
            if moved {
                window_config.x = Some(new_x);
                window_config.y = Some(new_y);
                changed = true;
            }
        }

        if changed {
            self.mark_runtime_dirty();
        }
    }

    fn schedule_detached_workspace_reattach(&mut self, workspace_local_id: &str) {
        if self.pending_detached_reattach.insert(workspace_local_id.to_string()) {
            self.mark_runtime_dirty();
        }
    }

    fn process_pending_detached_reattach(&mut self) {
        if self.pending_detached_reattach.is_empty() {
            return;
        }

        // Remove pending viewports at the start of the root pass so egui
        // simply stops emitting them this frame.
        let pending = std::mem::take(&mut self.pending_detached_reattach);
        let mut changed = false;
        for workspace_local_id in pending {
            changed |= self.detached_workspaces.remove(&workspace_local_id).is_some();
            self.pending_detached_window_position_restore
                .remove(&workspace_local_id);
        }

        if changed {
            self.mark_runtime_dirty();
        }
    }
}

fn detached_viewport_id(workspace_local_id: &str) -> ViewportId {
    ViewportId(egui::Id::new(("detached_workspace", workspace_local_id)))
}

fn consume_detached_position_restore(
    pending_detached_window_position_restore: &mut BTreeSet<String>,
    workspace_local_id: &str,
) -> bool {
    pending_detached_window_position_restore.remove(workspace_local_id)
}

fn detached_canvas_rect(ctx: &Context) -> Rect {
    let viewport = viewport_local_rect(ctx);
    Rect::from_min_max(Pos2::new(viewport.min.x, viewport.min.y + TOOLBAR_HEIGHT), viewport.max)
}

fn detached_viewport_builder(
    window_config: &WindowConfig,
    workspace_name: &str,
    restore_window_position: bool,
) -> ViewportBuilder {
    let mut builder = ViewportBuilder::default()
        .with_title(format!("{workspace_name} · {}", branding::APP_NAME))
        .with_icon(branding::app_icon())
        .with_decorations(true)
        .with_transparent(false)
        .with_min_inner_size([800.0, 600.0])
        .with_resizable(true);

    if restore_window_position {
        builder = builder.with_inner_size([window_config.width, window_config.height]);

        if let (Some(x), Some(y)) = (window_config.x, window_config.y) {
            builder = builder.with_position([x, y]);
        }
    }

    if cfg!(target_os = "linux") {
        builder = builder.with_app_id(branding::APP_ID);
    }

    builder
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{consume_detached_position_restore, detached_viewport_builder};
    use horizon_core::WindowConfig;

    #[test]
    fn detached_viewport_builder_only_restores_window_state_when_requested() {
        let window = WindowConfig {
            width: 1280.0,
            height: 720.0,
            x: Some(240.0),
            y: Some(120.0),
        };

        let restored = detached_viewport_builder(&window, "Alpha", true);
        let live = detached_viewport_builder(&window, "Alpha", false);

        assert_eq!(restored.position, Some(egui::pos2(240.0, 120.0)));
        assert_eq!(live.position, None);
        assert_eq!(restored.inner_size, Some(egui::vec2(1280.0, 720.0)));
        assert_eq!(live.inner_size, None);
    }

    #[test]
    fn detached_position_restore_is_consumed_once() {
        let workspace_local_id = "ws-alpha".to_string();
        let mut pending = BTreeSet::from([workspace_local_id.clone()]);

        assert!(consume_detached_position_restore(&mut pending, &workspace_local_id));
        assert!(!consume_detached_position_restore(&mut pending, &workspace_local_id));
        assert!(!consume_detached_position_restore(&mut pending, "ws-beta"));
    }
}
