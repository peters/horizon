use egui::emath::TSTransform;
use egui::{Button, Context, Id, Margin, Pos2, Rect, Stroke, Vec2};
use horizon_core::WorkspaceLayout;

use crate::theme;

use super::render::apply_canvas_transform;
use super::{
    WORKSPACE_LAYOUT_BUTTON_HEIGHT, WORKSPACE_LAYOUT_BUTTON_SPACING, WORKSPACE_LAYOUT_DEFAULT_BUTTON_WIDTH,
    WORKSPACE_LAYOUT_TOOLBAR_MARGIN_X, WORKSPACE_LAYOUT_TOOLBAR_MARGIN_Y, WORKSPACE_LAYOUT_TOOLBAR_OFFSET_X,
    WorkspaceAction, WorkspaceInteraction, WorkspaceVisual,
};

pub(super) fn should_show_workspace_layout_toolbar(workspace: &WorkspaceVisual) -> bool {
    workspace.panel_count > 0
}

pub(super) fn render_workspace_layout_toolbar(
    ctx: &Context,
    workspace: &WorkspaceVisual,
    canvas_transform: TSTransform,
    canvas_clip_rect: Rect,
) -> Option<WorkspaceAction> {
    let mut action = None;

    egui::Area::new(Id::new(("workspace_layout_toolbar", workspace.id.0)))
        .fixed_pos(workspace.toolbar_canvas_rect.min)
        .constrain(false)
        .interactable(false)
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            apply_canvas_transform(ui, canvas_transform, canvas_clip_rect);
            egui::Frame::new()
                .fill(theme::alpha(
                    theme::blend(theme::PANEL_BG_ALT, workspace.color, 0.08),
                    228,
                ))
                .stroke(Stroke::new(1.0, theme::alpha(workspace.color, 112)))
                .corner_radius(10.0)
                .inner_margin(Margin::symmetric(
                    WORKSPACE_LAYOUT_TOOLBAR_MARGIN_X,
                    WORKSPACE_LAYOUT_TOOLBAR_MARGIN_Y,
                ))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = WORKSPACE_LAYOUT_BUTTON_SPACING;
                        let is_default = workspace.layout.is_none();
                        let response = ui
                            .add(
                                Button::new(egui::RichText::new("Default").size(10.5).color(if is_default {
                                    theme::FG
                                } else {
                                    theme::FG_SOFT
                                }))
                                .min_size(Vec2::new(
                                    WORKSPACE_LAYOUT_DEFAULT_BUTTON_WIDTH,
                                    WORKSPACE_LAYOUT_BUTTON_HEIGHT,
                                ))
                                .fill(if is_default {
                                    theme::alpha(theme::blend(theme::PANEL_BG_ALT, workspace.color, 0.22), 236)
                                } else {
                                    theme::alpha(theme::blend(theme::PANEL_BG_ALT, workspace.color, 0.05), 220)
                                })
                                .stroke(Stroke::new(
                                    1.0,
                                    if is_default {
                                        theme::alpha(workspace.color, 224)
                                    } else {
                                        theme::alpha(theme::blend(theme::BORDER_SUBTLE, workspace.color, 0.24), 216)
                                    },
                                ))
                                .corner_radius(8),
                            )
                            .on_hover_text("Manual placement");
                        if response.clicked() {
                            action = Some(WorkspaceAction::ClearLayout);
                        }

                        for layout in WorkspaceLayout::ALL {
                            let is_selected = workspace.layout == Some(layout);
                            let response = ui
                                .add(
                                    Button::new(
                                        egui::RichText::new(workspace_layout_label(layout))
                                            .size(10.5)
                                            .color(if is_selected { theme::FG } else { theme::FG_SOFT }),
                                    )
                                    .min_size(Vec2::new(
                                        workspace_layout_button_width(layout),
                                        WORKSPACE_LAYOUT_BUTTON_HEIGHT,
                                    ))
                                    .fill(if is_selected {
                                        theme::alpha(theme::blend(theme::PANEL_BG_ALT, workspace.color, 0.22), 236)
                                    } else {
                                        theme::alpha(theme::blend(theme::PANEL_BG_ALT, workspace.color, 0.05), 220)
                                    })
                                    .stroke(Stroke::new(
                                        1.0,
                                        if is_selected {
                                            theme::alpha(workspace.color, 224)
                                        } else {
                                            theme::alpha(theme::blend(theme::BORDER_SUBTLE, workspace.color, 0.24), 216)
                                        },
                                    ))
                                    .corner_radius(8),
                                )
                                .on_hover_text(layout.label());
                            if response.clicked() {
                                action = Some(WorkspaceAction::ArrangeLayout(layout));
                            }
                        }

                        if render_detach_button(ui, workspace) {
                            action = Some(WorkspaceAction::Detach);
                        }
                    });
                });
        });

    action
}

pub(super) fn show_workspace_context_menu(
    response: &egui::Response,
    workspace: &WorkspaceVisual,
    interaction: &mut WorkspaceInteraction,
) {
    response.context_menu(|ui| {
        ui.set_min_width(160.0);
        ui.label(egui::RichText::new("Arrange Panels").size(11.0).color(theme::FG_DIM));
        if ui
            .add(Button::new(egui::RichText::new("Default").size(12.0).color(theme::FG_SOFT)).frame(false))
            .clicked()
        {
            interaction.action = Some(WorkspaceAction::ClearLayout);
            ui.close();
        }
        for layout in WorkspaceLayout::ALL {
            let text = egui::RichText::new(layout.label()).size(12.0).color(theme::FG_SOFT);
            if ui.add(Button::new(text).frame(false)).clicked() {
                interaction.action = Some(WorkspaceAction::ArrangeLayout(layout));
                ui.close();
            }
        }

        ui.separator();
        let close_all = ui.add_enabled(
            workspace.panel_count > 0,
            Button::new(
                egui::RichText::new("Close All Panels")
                    .size(12.0)
                    .color(theme::PALETTE_RED),
            )
            .frame(false),
        );
        if close_all.clicked() {
            interaction.action = Some(WorkspaceAction::CloseAllPanels);
            ui.close();
        }
    });
}

pub(super) fn workspace_layout_toolbar_rect(label_rect: Rect) -> Rect {
    Rect::from_min_size(
        Pos2::new(label_rect.max.x + WORKSPACE_LAYOUT_TOOLBAR_OFFSET_X, label_rect.min.y),
        Vec2::new(
            WORKSPACE_LAYOUT_DEFAULT_BUTTON_WIDTH
                + workspace_layout_preset_row_width()
                + 4.0 * WORKSPACE_LAYOUT_BUTTON_SPACING
                + 54.0
                + 2.0 * f32::from(WORKSPACE_LAYOUT_TOOLBAR_MARGIN_X),
            WORKSPACE_LAYOUT_BUTTON_HEIGHT + 2.0 * f32::from(WORKSPACE_LAYOUT_TOOLBAR_MARGIN_Y),
        ),
    )
}

fn workspace_layout_label(layout: WorkspaceLayout) -> &'static str {
    match layout {
        WorkspaceLayout::Rows => "Rows",
        WorkspaceLayout::Columns => "Cols",
        WorkspaceLayout::Grid => "Grid",
    }
}

fn workspace_layout_button_width(layout: WorkspaceLayout) -> f32 {
    match layout {
        WorkspaceLayout::Rows | WorkspaceLayout::Columns | WorkspaceLayout::Grid => 44.0,
    }
}

fn workspace_layout_preset_row_width() -> f32 {
    workspace_layout_button_width(WorkspaceLayout::Rows)
        + workspace_layout_button_width(WorkspaceLayout::Columns)
        + workspace_layout_button_width(WorkspaceLayout::Grid)
}

fn render_detach_button(ui: &mut egui::Ui, workspace: &WorkspaceVisual) -> bool {
    ui.add(
        Button::new(egui::RichText::new("Detach").size(10.5).color(theme::FG_SOFT))
            .min_size(Vec2::new(54.0, WORKSPACE_LAYOUT_BUTTON_HEIGHT))
            .fill(theme::alpha(
                theme::blend(theme::PANEL_BG_ALT, workspace.color, 0.05),
                220,
            ))
            .stroke(Stroke::new(
                1.0,
                theme::alpha(theme::blend(theme::BORDER_SUBTLE, workspace.color, 0.24), 216),
            ))
            .corner_radius(8),
    )
    .on_hover_text("Open in a separate window")
    .clicked()
}
