mod paint;
mod render;
mod toolbar;

use std::collections::HashMap;

use egui::{Color32, Context, Pos2, Rect, Vec2};
use horizon_core::{WorkspaceId, WorkspaceLayout};

use super::util::{OverlayExclusion, workspace_label_width};
use super::{HorizonApp, RenameEditAction, WS_BG_PAD, WS_EMPTY_SIZE, WS_LABEL_HEIGHT, WS_TITLE_HEIGHT};

use self::render::render_workspace_visual;
use self::toolbar::workspace_layout_toolbar_rect;

struct WorkspaceVisual {
    id: WorkspaceId,
    name: String,
    color: Color32,
    canvas_rect: Rect,
    screen_rect: Rect,
    label_canvas_rect: Rect,
    toolbar_canvas_rect: Rect,
    toolbar_screen_rect: Rect,
    is_active: bool,
    is_empty: bool,
    label_hidden: bool,
    panel_count: usize,
    layout: Option<WorkspaceLayout>,
}

struct WorkspaceInteraction {
    activate_workspace: bool,
    drag_delta: Vec2,
    start_rename: bool,
    rename_action: RenameEditAction,
    action: Option<WorkspaceAction>,
}

enum WorkspaceAction {
    Focus,
    Fit,
    ClearLayout,
    ArrangeLayout(WorkspaceLayout),
    CloseAllPanels,
    Detach,
}

const WORKSPACE_LAYOUT_BUTTON_HEIGHT: f32 = 24.0;
const WORKSPACE_LAYOUT_BUTTON_SPACING: f32 = 4.0;
const WORKSPACE_LAYOUT_DEFAULT_BUTTON_WIDTH: f32 = 60.0;
const WORKSPACE_LAYOUT_TOOLBAR_MARGIN_X: i8 = 6;
const WORKSPACE_LAYOUT_TOOLBAR_MARGIN_Y: i8 = 5;
const WORKSPACE_LAYOUT_TOOLBAR_OFFSET_X: f32 = 10.0;

impl HorizonApp {
    #[profiling::function]
    pub(super) fn render_workspace_backgrounds(
        &mut self,
        ctx: &Context,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
        overlay_zones: &OverlayExclusion,
    ) {
        let canvas_rect = self.canvas_rect(ctx);
        let canvas_transform = super::view::canvas_scene_transform(canvas_rect, self.canvas_view);
        let canvas_clip_rect = canvas_transform.inverse() * canvas_rect;
        let visuals = self.workspace_visuals(canvas_rect, workspace_bounds, overlay_zones);

        self.workspace_screen_rects.clear();
        let mut pending_workspace_moves = Vec::new();
        let mut focus_workspace = None;
        let mut start_rename_workspace = None;
        let mut rename_action = RenameEditAction::None;
        let mut clear_workspace_layout = None;
        let mut arrange_workspace = None;
        let mut close_workspace_panels = None;
        let mut focus_workspace_view = None;
        let mut fit_workspace_view = None;

        for workspace in &visuals {
            self.workspace_screen_rects.push((workspace.id, workspace.screen_rect));

            let is_renaming = self.renaming_workspace == Some(workspace.id);
            let interaction = if is_renaming {
                render_workspace_visual(
                    ctx,
                    workspace,
                    Some(&mut self.rename_buffer),
                    overlay_zones,
                    canvas_transform,
                    canvas_clip_rect,
                )
            } else {
                render_workspace_visual(ctx, workspace, None, overlay_zones, canvas_transform, canvas_clip_rect)
            };

            if interaction.activate_workspace {
                focus_workspace = Some(workspace.id);
            }
            if interaction.drag_delta != Vec2::ZERO {
                pending_workspace_moves.push((workspace.id, interaction.drag_delta));
            }
            if interaction.start_rename {
                start_rename_workspace = Some((workspace.id, workspace.name.clone()));
            }
            if interaction.rename_action != RenameEditAction::None {
                rename_action = interaction.rename_action;
            }
            match interaction.action {
                Some(WorkspaceAction::Focus) => {
                    focus_workspace_view = Some(workspace.id);
                }
                Some(WorkspaceAction::Fit) => {
                    fit_workspace_view = Some(workspace.id);
                }
                Some(WorkspaceAction::ClearLayout) => {
                    focus_workspace = Some(workspace.id);
                    clear_workspace_layout = Some(workspace.id);
                }
                Some(WorkspaceAction::ArrangeLayout(layout)) => {
                    focus_workspace = Some(workspace.id);
                    arrange_workspace = Some((workspace.id, layout));
                }
                Some(WorkspaceAction::Detach) => {
                    focus_workspace = Some(workspace.id);
                    self.detach_workspace(workspace.id);
                }
                Some(WorkspaceAction::CloseAllPanels) => {
                    focus_workspace = Some(workspace.id);
                    close_workspace_panels = Some(workspace.id);
                }
                None => {}
            }
        }

        if let Some((workspace_id, current_name)) = start_rename_workspace {
            self.clear_panel_rename();
            self.renaming_workspace = Some(workspace_id);
            self.rename_buffer = current_name;
        }

        match rename_action {
            RenameEditAction::Commit => {
                if let Some(workspace_id) = self.renaming_workspace {
                    let name = self.rename_buffer.trim().to_string();
                    if !name.is_empty() && self.board.rename_workspace(workspace_id, &name) {
                        self.mark_runtime_dirty();
                    }
                    self.clear_workspace_rename();
                }
            }
            RenameEditAction::Cancel => self.clear_workspace_rename(),
            RenameEditAction::None => {}
        }

        if let Some(workspace_id) = focus_workspace {
            self.board.focus_workspace(workspace_id);
        }
        if let Some(workspace_id) = focus_workspace_view {
            let _ = self.focus_workspace_visible(ctx, workspace_id, false);
        }
        if let Some(workspace_id) = fit_workspace_view {
            let _ = self.fit_workspace_visible(ctx, workspace_id);
        }
        if let Some(workspace_id) = clear_workspace_layout
            && self.board.clear_workspace_layout(workspace_id)
        {
            self.mark_runtime_dirty();
        }
        if let Some((workspace_id, layout)) = arrange_workspace {
            self.board.arrange_workspace(workspace_id, layout);
            self.mark_runtime_dirty();
        }
        if let Some(workspace_id) = close_workspace_panels {
            self.close_workspace_panels(workspace_id);
        }

        if !self.is_panning {
            for (workspace_id, delta) in pending_workspace_moves {
                let _ = self
                    .board
                    .translate_workspace_with_push(workspace_id, [delta.x, delta.y]);
                self.mark_runtime_dirty();
            }
        }
    }

    #[profiling::function]
    fn workspace_visuals(
        &self,
        canvas_rect: Rect,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
        overlay_zones: &OverlayExclusion,
    ) -> Vec<WorkspaceVisual> {
        self.board
            .workspaces
            .iter()
            .filter_map(|workspace| {
                if self.workspace_is_detached(workspace.id) {
                    return None;
                }

                let (r, g, b) = workspace.accent();
                let color = Color32::from_rgb(r, g, b);
                let is_active = self.board.active_workspace == Some(workspace.id);
                let (workspace_canvas_rect, screen_rect, is_empty) =
                    if let Some((min, max)) = workspace_bounds.get(&workspace.id).copied() {
                        let top_left = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
                        let bottom_right = Pos2::new(max[0] + WS_BG_PAD, max[1] + WS_BG_PAD);
                        let canvas_rect_local = Rect::from_min_max(top_left, bottom_right);
                        let screen_rect = Rect::from_min_size(
                            self.canvas_to_screen(canvas_rect, canvas_rect_local.min),
                            self.canvas_size_to_screen(canvas_rect_local.size()),
                        )
                        .intersect(canvas_rect);
                        (canvas_rect_local, screen_rect, false)
                    } else {
                        let canvas_rect_local = Rect::from_min_size(
                            Pos2::new(workspace.position[0], workspace.position[1]),
                            Vec2::new(WS_EMPTY_SIZE[0], WS_EMPTY_SIZE[1]),
                        );
                        let screen_rect = Rect::from_min_size(
                            self.canvas_to_screen(canvas_rect, canvas_rect_local.min),
                            self.canvas_size_to_screen(canvas_rect_local.size()),
                        )
                        .intersect(canvas_rect);
                        (canvas_rect_local, screen_rect, true)
                    };

                // Cull off-screen workspaces to avoid painting backgrounds and
                // labels for workspaces the user cannot see.
                if !screen_rect.is_positive() || !canvas_rect.intersects(screen_rect) {
                    return None;
                }

                let label_canvas_rect = Rect::from_min_size(
                    workspace_canvas_rect.min + Vec2::new(14.0, 12.0),
                    Vec2::new(workspace_label_width(&workspace.name), WS_LABEL_HEIGHT),
                );
                let label_screen_rect = Rect::from_min_size(
                    self.canvas_to_screen(canvas_rect, label_canvas_rect.min),
                    self.canvas_size_to_screen(label_canvas_rect.size()),
                );
                let toolbar_canvas_rect = workspace_layout_toolbar_rect(label_canvas_rect);
                let toolbar_screen_rect = Rect::from_min_size(
                    self.canvas_to_screen(canvas_rect, toolbar_canvas_rect.min),
                    self.canvas_size_to_screen(toolbar_canvas_rect.size()),
                );
                Some(WorkspaceVisual {
                    id: workspace.id,
                    name: workspace.name.clone(),
                    color,
                    canvas_rect: workspace_canvas_rect,
                    screen_rect,
                    label_canvas_rect,
                    toolbar_canvas_rect,
                    toolbar_screen_rect,
                    is_active,
                    is_empty,
                    label_hidden: overlay_zones.intersects(label_screen_rect),
                    panel_count: workspace.panels.len(),
                    layout: workspace.layout,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkspaceVisual, toolbar::should_show_workspace_layout_toolbar};
    use egui::{Color32, Pos2, Rect, Vec2};
    use horizon_core::WorkspaceId;

    fn workspace_visual(panel_count: usize) -> WorkspaceVisual {
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(120.0, 64.0));
        WorkspaceVisual {
            id: WorkspaceId(1),
            name: "Alpha".to_string(),
            color: Color32::WHITE,
            canvas_rect: rect,
            screen_rect: rect,
            label_canvas_rect: rect,
            toolbar_canvas_rect: rect,
            toolbar_screen_rect: rect,
            is_active: false,
            is_empty: panel_count == 0,
            label_hidden: false,
            panel_count,
            layout: None,
        }
    }

    #[test]
    fn layout_toolbar_stays_hidden_for_empty_workspaces() {
        assert!(!should_show_workspace_layout_toolbar(&workspace_visual(0)));
    }

    #[test]
    fn layout_toolbar_stays_visible_for_single_panel_workspaces() {
        assert!(should_show_workspace_layout_toolbar(&workspace_visual(1)));
    }
}
