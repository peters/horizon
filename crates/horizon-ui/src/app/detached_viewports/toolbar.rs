use egui::{Align, Button, Context, CornerRadius, FontId, Layout, Pos2, Stroke, TopBottomPanel, Vec2, ViewportId};
use horizon_core::{WorkspaceId, flatten_line_separators};

use crate::text::{painter_text_galley, stable_hover_text};
use crate::theme;

use super::{HorizonApp, TOOLBAR_HEIGHT, detached_canvas_rect};
use crate::app::util::{chrome_button, primary_shortcut_label};

const STATUS_LABEL: &str = "Detached Workspace";
const SHOW_MINIMAP_LABEL: &str = "Show Minimap";
const HIDE_MINIMAP_LABEL: &str = "Hide Minimap";

#[derive(Clone, Copy)]
struct ControlWidths {
    attach: f32,
    fit: f32,
    minimap: f32,
    status: f32,
}

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
        HIDE_MINIMAP_LABEL
    } else {
        SHOW_MINIMAP_LABEL
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
            let widths = control_widths(ui);
            let (name_width, status_width) = label_widths(ui.available_width(), spacing, widths);
            let workspace_name = flatten_line_separators(workspace_name);
            let _ = stable_hover_text(
                ui.add_sized(
                    [name_width, 30.0],
                    egui::Label::new(
                        egui::RichText::new(workspace_name.as_ref())
                            .color(theme::FG())
                            .size(13.5)
                            .strong(),
                    )
                    .truncate()
                    .show_tooltip_when_elided(false),
                ),
                workspace_name.as_ref(),
            );
            let _ = stable_hover_text(
                ui.add_sized(
                    [status_width, 30.0],
                    egui::Label::new(egui::RichText::new(STATUS_LABEL).color(theme::FG_DIM()).size(10.5))
                        .truncate()
                        .show_tooltip_when_elided(false),
                ),
                STATUS_LABEL,
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
                        .min_size(Vec2::new(widths.attach, 30.0)),
                    )
                    .clicked()
                {
                    app.schedule_detached_workspace_reattach(workspace_local_id);
                    ctx.request_repaint_of(ViewportId::ROOT);
                }

                if stable_hover_text(
                    ui.add(chrome_button("Fit Workspace").min_size(Vec2::new(widths.fit, 30.0))),
                    fit_shortcut.as_str(),
                )
                .clicked()
                {
                    let _ = app.fit_workspace_in_rect(workspace_id, detached_canvas_rect(ctx));
                }

                if stable_hover_text(
                    ui.add(chrome_button(minimap_label).min_size(Vec2::new(widths.minimap, 30.0))),
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

fn control_widths(ui: &egui::Ui) -> ControlWidths {
    let button_padding = ui.spacing().button_padding.x * 2.0;
    let minimum = ui.spacing().interact_size.x;
    let measure = |text: &str, font_size: f32| {
        (painter_text_galley(
            ui.painter(),
            text,
            &FontId::proportional(font_size),
            theme::FG_SOFT(),
            f32::INFINITY,
        )
        .size()
        .x + button_padding)
            .max(minimum)
    };
    ControlWidths {
        attach: measure("Attach to Main Window", 11.5),
        fit: measure("Fit Workspace", 11.0),
        minimap: measure(SHOW_MINIMAP_LABEL, 11.0).max(measure(HIDE_MINIMAP_LABEL, 11.0)),
        status: painter_text_galley(
            ui.painter(),
            STATUS_LABEL,
            &FontId::proportional(10.5),
            theme::FG_DIM(),
            f32::INFINITY,
        )
        .size()
        .x,
    }
}

fn label_widths(available_width: f32, spacing: f32, widths: ControlWidths) -> (f32, f32) {
    let controls_width = widths.attach + widths.fit + widths.minimap + spacing * 2.0;
    let labels_width = (available_width - controls_width - spacing * 2.0).max(0.0);
    let status_width = widths.status.min(labels_width * 0.5);
    ((labels_width - status_width).max(0.0), status_width)
}

#[cfg(test)]
mod tests {
    use super::{ControlWidths, label_widths};

    #[test]
    fn reattach_controls_are_reserved_before_the_workspace_name() {
        let widths = ControlWidths {
            attach: 154.0,
            fit: 126.0,
            minimap: 124.0,
            status: 116.0,
        };
        let (name_width, status_width) = label_widths(760.0, 8.0, widths);

        assert!(name_width > 0.0);
        assert!((status_width - 116.0).abs() < f32::EPSILON);

        let (narrow_name_width, narrow_status_width) = label_widths(460.0, 8.0, widths);
        assert!(narrow_name_width > 0.0);
        assert!(narrow_name_width >= narrow_status_width);
        assert!(narrow_status_width <= 420.0);
    }
}
