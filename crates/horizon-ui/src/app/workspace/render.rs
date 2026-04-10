use egui::emath::TSTransform;
use egui::{Context, CursorIcon, Id, Pos2, Rect, Sense, Vec2};

use crate::app::panels::show_inline_rename_editor;
use crate::app::view::canvas_layer_clip_rect;
use crate::app::{RenameEditAction, util::OverlayExclusion};

use super::paint::{
    paint_empty_workspace_hint, paint_workspace_frame, paint_workspace_label, paint_workspace_label_bg,
};
use super::toolbar::{
    render_workspace_layout_toolbar, should_show_workspace_layout_toolbar, show_workspace_context_menu,
};
use super::{WorkspaceInteraction, WorkspaceVisual};

#[profiling::function]
pub(super) fn render_workspace_visual(
    ctx: &Context,
    workspace: &WorkspaceVisual,
    rename_buffer: Option<&mut String>,
    overlay_zones: &OverlayExclusion,
    show_layout_toolbar: bool,
    canvas_transform: TSTransform,
    canvas_clip_rect: Rect,
) -> WorkspaceInteraction {
    let is_renaming = rename_buffer.is_some();

    egui::Area::new(Id::new(("workspace_bg", workspace.id.0)))
        .fixed_pos(workspace.canvas_rect.min)
        .constrain(false)
        .interactable(false)
        .order(egui::Order::Background)
        .show(ctx, |ui| {
            apply_canvas_transform(ui, canvas_transform, canvas_clip_rect);
            let (rect, _) = ui.allocate_exact_size(workspace.canvas_rect.size(), Sense::hover());
            paint_workspace_frame(ui, rect, workspace.color, workspace.is_active);

            if workspace.is_empty {
                paint_empty_workspace_hint(ui, rect, workspace.label_canvas_rect, workspace.color);
            }
        });

    if workspace.label_hidden {
        return WorkspaceInteraction {
            activate_workspace: false,
            drag_delta: Vec2::ZERO,
            drag_stopped: false,
            start_rename: false,
            rename_action: RenameEditAction::None,
            action: None,
        };
    }

    let mut interaction = egui::Area::new(Id::new(("workspace_label", workspace.id.0)))
        .fixed_pos(workspace.label_canvas_rect.min)
        .constrain(false)
        .interactable(false)
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            apply_canvas_transform(ui, canvas_transform, canvas_clip_rect);
            let (rect, _) = ui.allocate_exact_size(workspace.label_canvas_rect.size(), Sense::hover());
            let label_response = ui.interact(
                rect,
                ui.make_persistent_id(("workspace_drag", workspace.id.0)),
                Sense::click_and_drag(),
            );

            if let Some(buffer) = rename_buffer {
                paint_workspace_label_bg(ui, rect, workspace.color, true, false, false);
                WorkspaceInteraction {
                    activate_workspace: false,
                    drag_delta: Vec2::ZERO,
                    drag_stopped: false,
                    start_rename: false,
                    rename_action: show_inline_rename_editor(
                        ui,
                        Rect::from_min_max(
                            Pos2::new(rect.min.x + 12.0, rect.min.y + 2.0),
                            Pos2::new(rect.max.x - 8.0, rect.max.y - 2.0),
                        ),
                        buffer,
                        egui::FontId::proportional(12.5),
                    ),
                    action: None,
                }
            } else {
                if label_response.hovered() || label_response.dragged() {
                    ui.ctx().set_cursor_icon(if label_response.dragged() {
                        CursorIcon::Grabbing
                    } else {
                        CursorIcon::Grab
                    });
                }

                paint_workspace_label(
                    ui,
                    rect,
                    &workspace.name,
                    workspace.color,
                    workspace.is_active,
                    label_response.hovered(),
                    label_response.dragged(),
                );

                let mut interaction = WorkspaceInteraction {
                    activate_workspace: label_response.clicked() || label_response.drag_started(),
                    drag_delta: label_response.drag_delta(),
                    drag_stopped: label_response.drag_stopped(),
                    start_rename: label_response.double_clicked(),
                    rename_action: RenameEditAction::None,
                    action: None,
                };
                show_workspace_context_menu(&label_response, workspace, &mut interaction);
                interaction
            }
        })
        .inner;

    if show_layout_toolbar
        && !is_renaming
        && interaction.action.is_none()
        && should_show_workspace_layout_toolbar(workspace)
        && !overlay_zones.intersects(workspace.toolbar_screen_rect)
    {
        interaction.action = render_workspace_layout_toolbar(ctx, workspace, canvas_transform, canvas_clip_rect);
    }

    interaction
}

pub(super) fn apply_canvas_transform(ui: &mut egui::Ui, canvas_transform: TSTransform, canvas_clip_rect: Rect) {
    ui.ctx().set_transform_layer(ui.layer_id(), canvas_transform);
    ui.set_clip_rect(canvas_layer_clip_rect(
        ui.clip_rect(),
        canvas_transform,
        canvas_clip_rect,
    ));
}
