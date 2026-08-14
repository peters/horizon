use egui::{Align, Button, Context, CornerRadius, Layout, Pos2, Stroke, TopBottomPanel, Vec2, ViewportId};
use horizon_core::{WorkspaceId, flatten_line_separators};

use crate::text::stable_hover_text;
use crate::theme;

use super::{HorizonApp, TOOLBAR_HEIGHT, detached_canvas_rect};
use crate::app::util::{chrome_button, primary_shortcut_label};

const ATTACH_BUTTON_WIDTH: f32 = 154.0;
const FIT_BUTTON_WIDTH: f32 = 126.0;
const MINIMAP_BUTTON_WIDTH: f32 = 124.0;
const STATUS_LABEL_WIDTH: f32 = 116.0;

pub(super) fn render(
    app: &mut HorizonApp,
    ctx: &Context,
    workspace_id: WorkspaceId,
    workspace_local_id: &str,
    workspace_name: &str,
) {
    let fit_shortcut = app
        .shortcuts
        .fit_active_workspace
        .display_label(primary_shortcut_label());
    let minimap_shortcut = app.shortcuts.toggle_minimap.display_label(primary_shortcut_label());
    let minimap_label = if app.minimap_visible {
        "Hide Minimap"
    } else {
        "Show Minimap"
    };

    TopBottomPanel::top(egui::Id::new(("detached_workspace_toolbar", workspace_local_id))).show(ctx, |ui| {
        ui.set_height(TOOLBAR_HEIGHT);
        ui.painter()
            .rect_filled(ui.max_rect(), CornerRadius::ZERO, theme::TITLEBAR_BG());
        ui.painter().line_segment(
            [
                Pos2::new(ui.max_rect().min.x, ui.max_rect().max.y),
                Pos2::new(ui.max_rect().max.x, ui.max_rect().max.y),
            ],
            Stroke::new(1.0_f32, theme::alpha(theme::BORDER_SUBTLE(), 170)),
        );

        ui.horizontal(|ui| {
            ui.add_space(12.0);
            let spacing = ui.spacing().item_spacing.x;
            let (name_width, status_width) = label_widths(ui.available_width(), spacing);
            let workspace_name = flatten_line_separators(workspace_name);
            ui.add_sized(
                [name_width, 30.0],
                egui::Label::new(
                    egui::RichText::new(workspace_name.as_ref())
                        .color(theme::FG())
                        .size(13.5)
                        .strong(),
                )
                .truncate(),
            );
            ui.add_sized(
                [status_width, 30.0],
                egui::Label::new(
                    egui::RichText::new("Detached Workspace")
                        .color(theme::FG_DIM())
                        .size(10.5),
                )
                .truncate(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(
                        Button::new(
                            egui::RichText::new("Attach to Main Window")
                                .size(11.5)
                                .color(theme::FG_SOFT()),
                        )
                        .frame(false)
                        .min_size(Vec2::new(ATTACH_BUTTON_WIDTH, 30.0)),
                    )
                    .clicked()
                {
                    app.schedule_detached_workspace_reattach(workspace_local_id);
                    ctx.request_repaint_of(ViewportId::ROOT);
                }

                if stable_hover_text(
                    ui.add(chrome_button("Fit Workspace").min_size(Vec2::new(FIT_BUTTON_WIDTH, 30.0))),
                    fit_shortcut.as_str(),
                )
                .clicked()
                {
                    let _ = app.fit_workspace_in_rect(workspace_id, detached_canvas_rect(ctx));
                }

                if stable_hover_text(
                    ui.add(chrome_button(minimap_label).min_size(Vec2::new(MINIMAP_BUTTON_WIDTH, 30.0))),
                    minimap_shortcut.as_str(),
                )
                .clicked()
                {
                    app.minimap_visible = !app.minimap_visible;
                }
            });
        });
    });
}

fn label_widths(available_width: f32, spacing: f32) -> (f32, f32) {
    let controls_width = ATTACH_BUTTON_WIDTH + FIT_BUTTON_WIDTH + MINIMAP_BUTTON_WIDTH + spacing * 2.0;
    let labels_width = (available_width - controls_width - spacing * 2.0).max(0.0);
    let status_width = STATUS_LABEL_WIDTH.min(labels_width);
    ((labels_width - status_width).max(0.0), status_width)
}

#[cfg(test)]
mod tests {
    use super::label_widths;

    #[test]
    fn reattach_controls_are_reserved_before_the_workspace_name() {
        let (name_width, status_width) = label_widths(760.0, 8.0);

        assert!(name_width > 0.0);
        assert!((status_width - 116.0).abs() < f32::EPSILON);

        let (narrow_name_width, narrow_status_width) = label_widths(420.0, 8.0);
        assert!(narrow_name_width.abs() < f32::EPSILON);
        assert!(narrow_status_width <= 420.0);
    }
}
