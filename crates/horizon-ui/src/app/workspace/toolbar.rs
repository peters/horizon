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

#[profiling::function]
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
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            apply_canvas_transform(ui, canvas_transform, canvas_clip_rect);
            egui::Frame::new()
                .fill(theme::alpha(
                    theme::blend(theme::PANEL_BG_ALT(), workspace.color, 0.08),
                    228,
                ))
                .stroke(Stroke::new(1.0_f32, theme::alpha(workspace.color, 112)))
                .corner_radius(10.0)
                .inner_margin(Margin::symmetric(
                    WORKSPACE_LAYOUT_TOOLBAR_MARGIN_X,
                    WORKSPACE_LAYOUT_TOOLBAR_MARGIN_Y,
                ))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = WORKSPACE_LAYOUT_BUTTON_SPACING;
                        let mut layout = workspace.layout;
                        if workspace_layout_buttons(ui, &mut layout, workspace.color) {
                            action = Some(layout.map_or(WorkspaceAction::ClearLayout, WorkspaceAction::ArrangeLayout));
                        }

                        if render_detach_button(ui, workspace) {
                            action = Some(WorkspaceAction::Detach);
                        }
                        #[cfg(target_os = "linux")]
                        if render_cast_button(ui, workspace) {
                            action = Some(WorkspaceAction::Cast);
                        }
                    });
                });
        });

    action
}

/// Shared segmented layout controls for workspaces and cloud groups.
pub(in crate::app) fn workspace_layout_buttons(
    ui: &mut egui::Ui,
    selected: &mut Option<WorkspaceLayout>,
    color: egui::Color32,
) -> bool {
    let mut changed = false;
    ui.spacing_mut().item_spacing.x = WORKSPACE_LAYOUT_BUTTON_SPACING;
    for layout in std::iter::once(None).chain(WorkspaceLayout::ALL.into_iter().map(Some)) {
        let active = *selected == layout;
        let label = layout.map_or("Default", workspace_layout_label);
        let width = layout.map_or(WORKSPACE_LAYOUT_DEFAULT_BUTTON_WIDTH, workspace_layout_button_width);
        let response = ui
            .add(
                Button::new(egui::RichText::new(label).size(10.5).color(if active {
                    theme::FG()
                } else {
                    theme::FG_SOFT()
                }))
                .min_size(Vec2::new(width, WORKSPACE_LAYOUT_BUTTON_HEIGHT))
                .fill(theme::alpha(
                    theme::blend(theme::PANEL_BG_ALT(), color, if active { 0.22 } else { 0.05 }),
                    if active { 236 } else { 220 },
                ))
                .stroke(Stroke::new(
                    1.0,
                    if active {
                        theme::alpha(color, 224)
                    } else {
                        theme::alpha(theme::blend(theme::BORDER_SUBTLE(), color, 0.24), 216)
                    },
                ))
                .corner_radius(8),
            )
            .on_hover_text(layout.map_or("Manual placement", WorkspaceLayout::label));
        if response.clicked() {
            *selected = layout;
            changed = true;
        }
    }
    changed
}

pub(super) fn show_workspace_context_menu(
    response: &egui::Response,
    workspace: &WorkspaceVisual,
    interaction: &mut WorkspaceInteraction,
) {
    response.context_menu(|ui| {
        ui.set_min_width(160.0);
        ui.label(egui::RichText::new("Workspace").size(11.0).color(theme::FG_DIM()));
        if ui
            .add(
                Button::new(
                    egui::RichText::new("Focus Workspace")
                        .size(12.0)
                        .color(theme::FG_SOFT()),
                )
                .frame(false),
            )
            .clicked()
        {
            interaction.action = Some(WorkspaceAction::Focus);
            ui.close();
        }
        if ui
            .add(Button::new(egui::RichText::new("Fit Workspace").size(12.0).color(theme::FG_SOFT())).frame(false))
            .clicked()
        {
            interaction.action = Some(WorkspaceAction::Fit);
            ui.close();
        }

        ui.separator();
        ui.label(egui::RichText::new("Arrange Panels").size(11.0).color(theme::FG_DIM()));
        if ui
            .add(Button::new(egui::RichText::new("Default").size(12.0).color(theme::FG_SOFT())).frame(false))
            .clicked()
        {
            interaction.action = Some(WorkspaceAction::ClearLayout);
            ui.close();
        }
        for layout in WorkspaceLayout::ALL {
            let text = egui::RichText::new(layout.label()).size(12.0).color(theme::FG_SOFT());
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
                    .color(theme::PALETTE_RED()),
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
                + workspace_cast_button_room()
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

/// Width of the cast button and the gap before it; casting is Linux-only.
fn workspace_cast_button_room() -> f32 {
    if cfg!(target_os = "linux") {
        WORKSPACE_LAYOUT_BUTTON_SPACING + WORKSPACE_CAST_BUTTON_WIDTH
    } else {
        0.0
    }
}

const WORKSPACE_CAST_BUTTON_WIDTH: f32 = 30.0;

/// Casts the whole workspace, its panels and clouds, to an Apple TV.
#[cfg(target_os = "linux")]
fn render_cast_button(ui: &mut egui::Ui, workspace: &WorkspaceVisual) -> bool {
    let on = workspace.casting;
    let response = ui.add(
        Button::new("")
            .min_size(Vec2::new(WORKSPACE_CAST_BUTTON_WIDTH, WORKSPACE_LAYOUT_BUTTON_HEIGHT))
            .fill(theme::alpha(
                theme::blend(theme::PANEL_BG_ALT(), workspace.color, if on { 0.22 } else { 0.05 }),
                220,
            ))
            .stroke(Stroke::new(
                1.0_f32,
                theme::alpha(theme::blend(theme::BORDER_SUBTLE(), workspace.color, 0.24), 216),
            ))
            .corner_radius(8),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Cast workspace"));
    let color = if on {
        ui.visuals().selection.stroke.color
    } else if response.hovered() {
        ui.visuals().strong_text_color()
    } else {
        theme::FG_SOFT()
    };
    let icon = Rect::from_center_size(response.rect.center(), Vec2::splat(16.0));
    crate::app::casting::paint_cast_icon(ui.painter(), icon, color);
    response.on_hover_text("Cast this workspace").clicked()
}

fn render_detach_button(ui: &mut egui::Ui, workspace: &WorkspaceVisual) -> bool {
    let response = ui
        .add_enabled(
            workspace.capabilities.can_detach,
            Button::new(egui::RichText::new("Detach").size(10.5).color(theme::FG_SOFT()))
                .min_size(Vec2::new(54.0, WORKSPACE_LAYOUT_BUTTON_HEIGHT))
                .fill(theme::alpha(
                    theme::blend(theme::PANEL_BG_ALT(), workspace.color, 0.05),
                    220,
                ))
                .stroke(Stroke::new(
                    1.0_f32,
                    theme::alpha(theme::blend(theme::BORDER_SUBTLE(), workspace.color, 0.24), 216),
                ))
                .corner_radius(8),
        )
        .on_hover_text("Open in a separate window")
        .on_disabled_hover_text("Cloud workspaces stay in the main window. Use the cloud's Full screen action.");
    response.clicked()
}

#[cfg(test)]
mod tests {
    use egui::{Event, Modifiers, PointerButton, RawInput, ViewportId};
    use horizon_core::AppearanceTheme;
    use horizon_core::WorkspaceId;

    use super::super::WorkspaceAction;
    use super::*;
    use crate::test_egui::DiscardTextures;
    use crate::theme;

    fn test_workspace_visual(toolbar_canvas_rect: Rect) -> WorkspaceVisual {
        WorkspaceVisual {
            id: WorkspaceId(1),
            name: "manual".to_string(),
            color: egui::Color32::LIGHT_BLUE,
            canvas_rect: Rect::from_min_size(Pos2::new(40.0, 100.0), Vec2::new(900.0, 600.0)),
            screen_rect: Rect::from_min_size(Pos2::new(40.0, 100.0), Vec2::new(900.0, 600.0)),
            label_canvas_rect: Rect::from_min_size(Pos2::new(60.0, 110.0), Vec2::new(120.0, 26.0)),
            toolbar_canvas_rect,
            toolbar_screen_rect: toolbar_canvas_rect,
            is_active: true,
            is_empty: false,
            label_hidden: false,
            panel_count: 3,
            layout: None,
            capabilities: super::super::WorkspaceLayoutCapabilities {
                can_arrange: true,
                can_detach: true,
            },
            #[cfg(target_os = "linux")]
            casting: false,
        }
    }

    /// Moves to `pos`, presses and releases there over the toolbar of `visual`, and
    /// returns the action it emitted.
    fn click_toolbar(visual: &WorkspaceVisual, pos: Pos2) -> Option<WorkspaceAction> {
        let ctx = egui::Context::default();
        theme::apply(&ctx, AppearanceTheme::Dark);
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0));
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let frames: [Vec<Event>; 4] = [
            Vec::new(),
            vec![Event::PointerMoved(pos)],
            vec![button(true)],
            vec![button(false)],
        ];
        let mut action = None;
        for events in frames {
            let mut input = RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            };
            input.viewport_id = ViewportId::ROOT;
            let _ = ctx
                .run_ui(input, |ctx| {
                    if let Some(emitted) = render_workspace_layout_toolbar(ctx, visual, TSTransform::IDENTITY, screen) {
                        action = Some(emitted);
                    }
                })
                .discard_textures();
        }
        action
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_cast_button_after_detach_emits_cast() {
        let toolbar_rect = Rect::from_min_size(Pos2::new(400.0, 140.0), Vec2::new(260.0, 34.0));
        let visual = test_workspace_visual(toolbar_rect);
        let y =
            toolbar_rect.min.y + f32::from(WORKSPACE_LAYOUT_TOOLBAR_MARGIN_Y) + WORKSPACE_LAYOUT_BUTTON_HEIGHT / 2.0;
        // Buttons may grow past their minimum width with their labels, so scan the row:
        // the last button inside the toolbar's reserved width casts, the one before
        // it detaches.
        let room = workspace_layout_toolbar_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(120.0, 26.0)));
        let emitted: Vec<_> = (0..)
            .map(|step| toolbar_rect.min.x + 2.0 * step as f32)
            .take_while(|x| *x <= toolbar_rect.min.x + room.width())
            .filter_map(|x| click_toolbar(&visual, Pos2::new(x, y)))
            .collect();
        let first_cast = emitted
            .iter()
            .position(|action| matches!(action, WorkspaceAction::Cast));
        assert!(first_cast.is_some(), "the cast button lies inside the reserved width");
        assert!(
            first_cast.is_some_and(|index| index > 0 && matches!(emitted[index - 1], WorkspaceAction::Detach)),
            "the cast button follows Detach"
        );
    }

    /// Regression test for the egui 0.36 upgrade: layers marked
    /// `interactable(false)` became click-through in hit-testing, which made
    /// every toolbar button dead. Clicking the Grid button must emit an
    /// `ArrangeLayout` action.
    #[test]
    fn grid_button_click_emits_arrange_action() {
        let toolbar_rect = Rect::from_min_size(Pos2::new(400.0, 140.0), Vec2::new(260.0, 34.0));
        let visual = test_workspace_visual(toolbar_rect);

        let grid_center = Pos2::new(
            toolbar_rect.min.x
                + f32::from(WORKSPACE_LAYOUT_TOOLBAR_MARGIN_X)
                + WORKSPACE_LAYOUT_DEFAULT_BUTTON_WIDTH
                + 2.0 * (WORKSPACE_LAYOUT_BUTTON_SPACING + workspace_layout_button_width(WorkspaceLayout::Rows))
                + WORKSPACE_LAYOUT_BUTTON_SPACING
                + workspace_layout_button_width(WorkspaceLayout::Grid) / 2.0,
            toolbar_rect.min.y + f32::from(WORKSPACE_LAYOUT_TOOLBAR_MARGIN_Y) + WORKSPACE_LAYOUT_BUTTON_HEIGHT / 2.0,
        );

        let action = click_toolbar(&visual, grid_center);

        assert!(
            matches!(action, Some(WorkspaceAction::ArrangeLayout(WorkspaceLayout::Grid))),
            "clicking the Grid button should emit ArrangeLayout(Grid), got {:?}",
            action.map(|a| match a {
                WorkspaceAction::Focus => "Focus",
                WorkspaceAction::Fit => "Fit",
                WorkspaceAction::ClearLayout => "ClearLayout",
                WorkspaceAction::ArrangeLayout(_) => "ArrangeLayout(non-Grid)",
                WorkspaceAction::CloseAllPanels => "CloseAllPanels",
                WorkspaceAction::Detach => "Detach",
                #[cfg(target_os = "linux")]
                WorkspaceAction::Cast => "Cast",
            })
        );
    }
}
