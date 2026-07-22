use std::collections::HashMap;

use egui::{Context, CursorIcon, Id, Order, PopupAnchor, Pos2, Rect, Sense, Tooltip, Ui, Vec2};
use horizon_core::{Board, Panel, PanelId, WorkspaceId};

use super::{
    HorizonApp, MINIMAP_MARGIN, MINIMAP_PAD, MinimapHitTarget, MinimapModel, MinimapScope, minimap_model,
    paint_minimap_contents, panel_minimap_screen_rect, workspace_minimap_screen_rect,
};

pub(super) fn render_scoped_minimap(
    app: &mut HorizonApp,
    ctx: &Context,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    canvas_rect: Rect,
    scope: MinimapScope,
    overlay_id: Id,
) -> f32 {
    if !app.fixed_overlays_visible() || !app.minimap_visible || !scope_has_content(app, scope) {
        return 0.0;
    }

    let Some(model) = minimap_model(app, canvas_rect, workspace_bounds, scope) else {
        return 0.0;
    };
    let minimap_height = model.outer_size.y;

    let response = egui::Area::new(overlay_id)
        .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-MINIMAP_MARGIN, -MINIMAP_MARGIN))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            let (response, painter) = ui.allocate_painter(model.outer_size, Sense::click_and_drag());
            let hovered = if response.dragged() {
                None
            } else {
                response.hover_pos().and_then(|pointer| {
                    minimap_hit_target(app, response.rect.min, &model, workspace_bounds, scope, pointer)
                })
            };
            paint_minimap_contents(app, &painter, response.rect, &model, workspace_bounds, scope, hovered);
            if hovered.is_some() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            show_minimap_hover_tooltip(app, ui, overlay_id, hovered);
            (response, hovered)
        });

    let (inner, hovered) = response.inner;
    if inner.dragged() {
        if let Some(pointer) = ctx.input(|input| input.pointer.interact_pos()) {
            center_minimap_point(app, &model, canvas_rect, inner.rect.min, pointer);
        }
    } else if inner.double_clicked() {
        if let Some(target) = hovered {
            fit_minimap_target(app, ctx, canvas_rect, scope, target);
        }
    } else if inner.clicked() {
        match hovered {
            Some(MinimapHitTarget::Panel { panel_id, .. }) => match scope {
                MinimapScope::Attached => app.focus_panel_visible(ctx, panel_id, false),
                MinimapScope::Workspace(_) => app.focus_panel_in_rect(panel_id, canvas_rect),
            },
            Some(MinimapHitTarget::Workspace(workspace_id)) if scope == MinimapScope::Attached => {
                let _ = app.focus_workspace_visible(ctx, workspace_id, false);
            }
            _ => {
                if let Some(pointer) = ctx.input(|input| input.pointer.interact_pos()) {
                    center_minimap_point(app, &model, canvas_rect, inner.rect.min, pointer);
                }
            }
        }
    }

    minimap_height
}

fn center_minimap_point(app: &mut HorizonApp, model: &MinimapModel, canvas_rect: Rect, origin: Pos2, pointer: Pos2) {
    let local = pointer - origin;
    let canvas_x = model.content_min[0] + (local.x - MINIMAP_PAD) / model.scale_x;
    let canvas_y = model.content_min[1] + (local.y - MINIMAP_PAD) / model.scale_y;

    app.pan_target = None;
    app.canvas_view.align_canvas_point_to_screen(
        [canvas_rect.min.x, canvas_rect.min.y],
        [canvas_x, canvas_y],
        [canvas_rect.center().x, canvas_rect.center().y],
    );
    app.mark_runtime_dirty();
}

fn fit_minimap_target(
    app: &mut HorizonApp,
    ctx: &Context,
    canvas_rect: Rect,
    scope: MinimapScope,
    target: MinimapHitTarget,
) {
    let fitted = match scope {
        MinimapScope::Attached => app.fit_workspace_visible(ctx, target.workspace_id()),
        MinimapScope::Workspace(_) => app.fit_workspace_in_rect(target.workspace_id(), canvas_rect),
    };
    restore_minimap_panel_focus(&mut app.board, target, fitted);
}

fn restore_minimap_panel_focus(board: &mut Board, target: MinimapHitTarget, fitted: bool) {
    if fitted && let MinimapHitTarget::Panel { panel_id, .. } = target {
        board.focus(panel_id);
    }
}

fn show_minimap_hover_tooltip(app: &HorizonApp, ui: &Ui, overlay_id: Id, hovered: Option<MinimapHitTarget>) {
    let Some(target) = hovered else {
        return;
    };

    let text = match target {
        MinimapHitTarget::Panel { panel_id, .. } => app
            .board
            .panel(panel_id)
            .map(|panel| panel.display_title().into_owned()),
        MinimapHitTarget::Workspace(workspace_id) => app.board.workspace(workspace_id).map(|workspace| {
            let panel_count = workspace.panels.len();
            format!(
                "{} — {} panel{}",
                workspace.name,
                panel_count,
                if panel_count == 1 { "" } else { "s" }
            )
        }),
    };
    let Some(text) = text else {
        return;
    };

    Tooltip::always_open(
        ui.ctx().clone(),
        ui.layer_id(),
        overlay_id.with("minimap_hover_tooltip"),
        PopupAnchor::Pointer,
    )
    .gap(12.0)
    .show(|ui| {
        ui.label(text);
    });
}

enum MinimapPanelSource<'a> {
    Board(std::slice::Iter<'a, Panel>),
    Workspace {
        board: &'a Board,
        panel_ids: std::slice::Iter<'a, PanelId>,
    },
}

impl<'a> Iterator for MinimapPanelSource<'a> {
    type Item = &'a Panel;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Board(panels) => panels.next(),
            Self::Workspace { board, panel_ids } => loop {
                let panel_id = *panel_ids.next()?;
                if let Some(panel) = board.panel(panel_id) {
                    return Some(panel);
                }
            },
        }
    }
}

fn minimap_panel_source(board: &Board, scope: MinimapScope) -> MinimapPanelSource<'_> {
    match scope {
        MinimapScope::Attached => MinimapPanelSource::Board(board.panels.iter()),
        MinimapScope::Workspace(workspace_id) => {
            let panel_ids = board
                .workspace(workspace_id)
                .map(|workspace| workspace.panels.as_slice())
                .unwrap_or_default();
            MinimapPanelSource::Workspace {
                board,
                panel_ids: panel_ids.iter(),
            }
        }
    }
}

pub(super) struct MinimapPanelPaintOrder<'a> {
    app: &'a HorizonApp,
    scope: MinimapScope,
    source: MinimapPanelSource<'a>,
    focused: Option<PanelId>,
    deferred_focused: Option<&'a Panel>,
    source_exhausted: bool,
}

impl<'a> Iterator for MinimapPanelPaintOrder<'a> {
    type Item = &'a Panel;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.source_exhausted {
            let Some(panel) = self.source.next() else {
                self.source_exhausted = true;
                break;
            };
            if !scope_includes_workspace(self.app, self.scope, panel.workspace_id) {
                continue;
            }
            if Some(panel.id) == self.focused {
                self.deferred_focused = Some(panel);
                continue;
            }
            return Some(panel);
        }

        self.deferred_focused.take()
    }
}

/// Match each canvas's panel source order, with the focused panel last, so the
/// focus outline stays visible and hit-testing prefers the visual topmost panel.
pub(super) fn minimap_panels_in_paint_order(app: &HorizonApp, scope: MinimapScope) -> MinimapPanelPaintOrder<'_> {
    MinimapPanelPaintOrder {
        app,
        scope,
        source: minimap_panel_source(&app.board, scope),
        focused: app.board.focused,
        deferred_focused: None,
        source_exhausted: false,
    }
}

/// Returns the target whose rect contains `pos`, preferring the one drawn last
/// (topmost). Pure so the precedence rules stay unit-testable.
fn last_hit<T>(pos: Pos2, items: impl Iterator<Item = (T, Rect)>) -> Option<T> {
    let mut hit = None;
    for (target, rect) in items {
        if rect.contains(pos) {
            hit = Some(target);
        }
    }
    hit
}

fn minimap_hit_target(
    app: &HorizonApp,
    origin: Pos2,
    model: &MinimapModel,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
    pos: Pos2,
) -> Option<MinimapHitTarget> {
    let panel_hit = last_hit(
        pos,
        minimap_panels_in_paint_order(app, scope).map(|panel| {
            (
                MinimapHitTarget::Panel {
                    panel_id: panel.id,
                    workspace_id: panel.workspace_id,
                },
                panel_minimap_screen_rect(origin, model, panel.layout.position, panel.layout.size),
            )
        }),
    );
    if panel_hit.is_some() {
        return panel_hit;
    }

    last_hit(
        pos,
        app.board
            .workspaces
            .iter()
            .filter(|workspace| scope_includes_workspace(app, scope, workspace.id))
            .map(|workspace| {
                (
                    MinimapHitTarget::Workspace(workspace.id),
                    workspace_minimap_screen_rect(origin, model, workspace.id, workspace.position, workspace_bounds),
                )
            }),
    )
}

fn scope_has_content(app: &HorizonApp, scope: MinimapScope) -> bool {
    match scope {
        MinimapScope::Attached => app
            .board
            .workspaces
            .iter()
            .any(|workspace| !app.workspace_is_detached(workspace.id)),
        MinimapScope::Workspace(workspace_id) => app.board.workspace(workspace_id).is_some(),
    }
}

pub(super) fn scope_includes_workspace(app: &HorizonApp, scope: MinimapScope, workspace_id: WorkspaceId) -> bool {
    match scope {
        MinimapScope::Attached => !app.workspace_is_detached(workspace_id),
        MinimapScope::Workspace(target_workspace_id) => target_workspace_id == workspace_id,
    }
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect, Vec2};
    use horizon_core::{Board, PanelKind, PanelOptions};

    use super::{MinimapHitTarget, MinimapScope, last_hit, minimap_panel_source, restore_minimap_panel_focus};

    fn editor_panel_options(name: &str) -> PanelOptions {
        PanelOptions {
            name: Some(name.to_string()),
            kind: PanelKind::Editor,
            ..PanelOptions::default()
        }
    }

    #[test]
    fn last_hit_prefers_topmost_of_overlapping_rects() {
        let bottom = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        let top = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(40.0, 40.0));
        let items = [(1_u8, bottom), (2_u8, top)];

        assert_eq!(last_hit(Pos2::new(20.0, 20.0), items.iter().copied()), Some(2));
        assert_eq!(last_hit(Pos2::new(5.0, 5.0), items.iter().copied()), Some(1));
        assert_eq!(last_hit(Pos2::new(90.0, 90.0), items.iter().copied()), None);
    }

    #[test]
    fn panel_source_matches_each_canvas_order_after_reassignment() {
        let mut board = Board::new();
        let source_workspace = board.create_workspace("Source");
        let target_workspace = board.create_workspace("Target");
        let moved_panel = board
            .create_panel(editor_panel_options("Moved"), source_workspace)
            .expect("moved panel");
        let existing_panel = board
            .create_panel(editor_panel_options("Existing"), target_workspace)
            .expect("existing panel");

        board.assign_panel_to_workspace(moved_panel, target_workspace);

        let attached_order = minimap_panel_source(&board, MinimapScope::Attached)
            .map(|panel| panel.id)
            .collect::<Vec<_>>();
        let detached_order = minimap_panel_source(&board, MinimapScope::Workspace(target_workspace))
            .map(|panel| panel.id)
            .collect::<Vec<_>>();

        assert_eq!(attached_order, vec![moved_panel, existing_panel]);
        assert_eq!(detached_order, vec![existing_panel, moved_panel]);
    }

    #[test]
    fn panel_double_click_keeps_clicked_panel_focused_after_workspace_fit() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("Target");
        let clicked_panel = board
            .create_panel(editor_panel_options("Clicked"), workspace_id)
            .expect("clicked panel");
        let last_panel = board
            .create_panel(editor_panel_options("Last"), workspace_id)
            .expect("last panel");
        let target = MinimapHitTarget::Panel {
            panel_id: clicked_panel,
            workspace_id,
        };

        board.focus_workspace(workspace_id);
        assert_eq!(board.focused, Some(last_panel));

        restore_minimap_panel_focus(&mut board, target, true);

        assert_eq!(board.focused, Some(clicked_panel));
        assert_eq!(board.active_workspace, Some(workspace_id));
    }
}
