use egui::{
    Align, Button, Color32, Context, CornerRadius, Layout, Pos2, Rect, Stroke, StrokeKind, TopBottomPanel, Vec2,
    ViewportBuilder, ViewportCommand, ViewportId,
};
use horizon_core::{CanvasViewState, WindowConfig, WorkspaceId};

use crate::{branding, theme};

use super::{HorizonApp, TOOLBAR_HEIGHT, WS_BG_PAD, WS_EMPTY_SIZE, WS_TITLE_HEIGHT};

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
            self.initial_detached_window_config(workspace_id),
        );
        self.mark_runtime_dirty();
    }

    pub(super) fn reattach_workspace(&mut self, workspace_id: WorkspaceId) {
        let Some(workspace) = self.board.workspace(workspace_id) else {
            return;
        };
        if self.detached_workspaces.remove(&workspace.local_id).is_some() {
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
            let Some(window_config) = self.detached_workspaces.get(&local_id).cloned() else {
                continue;
            };

            let viewport_id = detached_viewport_id(&local_id);
            let builder = detached_viewport_builder(&window_config, &workspace.name);
            let local_id_for_viewport = local_id.clone();

            ctx.show_viewport_immediate(viewport_id, builder, |viewport_ctx, _class| {
                self.render_detached_workspace_window(viewport_ctx, workspace_id, &local_id_for_viewport);
            });
        }

        if !stale_local_ids.is_empty() {
            for local_id in stale_local_ids {
                self.detached_workspaces.remove(&local_id);
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

        let Some(workspace_name) = self
            .board
            .workspace(workspace_id)
            .map(|workspace| workspace.name.clone())
        else {
            self.detached_workspaces.remove(workspace_local_id);
            self.mark_runtime_dirty();
            return;
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
                ui.label(
                    egui::RichText::new(&workspace_name)
                        .color(theme::FG)
                        .size(13.5)
                        .strong(),
                );
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
                });
            });
        });

        let saved_canvas_view = self.canvas_view;
        let saved_pan_target = self.pan_target;
        let saved_is_panning = self.is_panning;
        // Detached rendering must not overwrite root-window hit-testing or
        // close requests that were collected earlier in the frame.
        let saved_panel_screen_rects = std::mem::take(&mut self.panel_screen_rects);
        let saved_panels_to_close = std::mem::take(&mut self.panels_to_close);
        self.canvas_view = self.detached_canvas_view(ctx, workspace_id);
        self.pan_target = None;
        self.is_panning = false;

        self.render_canvas(ctx);
        self.render_detached_workspace_frame(ctx, workspace_id);
        self.render_panels_for_workspace(ctx, workspace_id);

        self.canvas_view = saved_canvas_view;
        self.pan_target = saved_pan_target;
        self.is_panning = saved_is_panning;
        self.panel_screen_rects = saved_panel_screen_rects;
        self.panels_to_close = saved_panels_to_close;
    }

    fn render_detached_workspace_frame(&self, ctx: &Context, workspace_id: WorkspaceId) {
        let Some(workspace) = self.board.workspace(workspace_id) else {
            return;
        };

        let canvas_rect = Self::canvas_rect(ctx, false);
        let (r, g, b) = workspace.accent();
        let color = Color32::from_rgb(r, g, b);
        let screen_rect = if let Some((min, max)) = self.board.workspace_bounds(workspace_id) {
            let top_left = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
            let bottom_right = Pos2::new(max[0] + WS_BG_PAD, max[1] + WS_BG_PAD);
            Rect::from_min_max(
                self.canvas_to_screen(canvas_rect, top_left),
                self.canvas_to_screen(canvas_rect, bottom_right),
            )
        } else {
            let screen_min =
                self.canvas_to_screen(canvas_rect, Pos2::new(workspace.position[0], workspace.position[1]));
            Rect::from_min_size(screen_min, Vec2::new(WS_EMPTY_SIZE[0], WS_EMPTY_SIZE[1]))
        };

        egui::Area::new(egui::Id::new(("detached_workspace_bg", workspace.id.0)))
            .fixed_pos(screen_rect.min)
            .constrain(false)
            .order(egui::Order::Background)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen_rect.size(), egui::Sense::hover());
                paint_detached_workspace_frame(ui, rect, color, self.board.active_workspace == Some(workspace_id));
            });
    }

    fn render_panels_for_workspace(&mut self, ctx: &Context, workspace_id: WorkspaceId) {
        self.panel_screen_rects.clear();

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

        let canvas_rect = Self::canvas_rect(ctx, false);
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
        let canvas_rect = Self::canvas_rect(ctx, false);
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
        let Some(window_config) = self.detached_workspaces.get_mut(workspace_local_id) else {
            return;
        };

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
        }

        if changed {
            self.mark_runtime_dirty();
        }
    }
}

fn detached_viewport_id(workspace_local_id: &str) -> ViewportId {
    ViewportId(egui::Id::new(("detached_workspace", workspace_local_id)))
}

fn detached_viewport_builder(window_config: &WindowConfig, workspace_name: &str) -> ViewportBuilder {
    let mut builder = ViewportBuilder::default()
        .with_title(format!("{workspace_name} · {}", branding::APP_NAME))
        .with_icon(branding::app_icon())
        .with_decorations(true)
        .with_transparent(false)
        .with_inner_size([window_config.width, window_config.height])
        .with_min_inner_size([800.0, 600.0])
        .with_resizable(true);

    if let (Some(x), Some(y)) = (window_config.x, window_config.y) {
        builder = builder.with_position([x, y]);
    }

    if cfg!(target_os = "linux") {
        builder = builder.with_app_id(branding::APP_ID);
    }

    builder
}

fn paint_detached_workspace_frame(ui: &mut egui::Ui, rect: Rect, color: Color32, is_active: bool) {
    let fill = if is_active {
        theme::alpha(theme::blend(theme::PANEL_BG_ALT, color, 0.12), 112)
    } else {
        theme::alpha(theme::PANEL_BG_ALT, 180)
    };
    let stroke_color = if is_active {
        theme::alpha(color, 156)
    } else {
        theme::alpha(theme::blend(theme::BORDER_SUBTLE, color, 0.18), 110)
    };

    ui.painter().rect(
        rect,
        CornerRadius::same(20),
        fill,
        Stroke::new(1.0, stroke_color),
        StrokeKind::Outside,
    );
    ui.painter().rect_stroke(
        rect.shrink(1.0),
        CornerRadius::same(19),
        Stroke::new(1.0, theme::alpha(color, if is_active { 42 } else { 20 })),
        StrokeKind::Inside,
    );
}
