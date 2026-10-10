//! The workspace and panel rows of the sidebar.
use egui::{Align, Color32, CornerRadius, Layout, Pos2, Rect, Sense, Stroke, Vec2};

use crate::theme;

use super::{SidebarWorkspaceInsert, SidebarWorkspaceRowInteraction, WorkspaceSidebarEntry};

/// Width left for the workspace name after the badges that follow it: the
/// panel count (accordion rows only) and the `NEW WINDOW` badge (detached
/// rows). Sizing the label to this width lets it truncate instead of running
/// past the sidebar edge.
pub(super) fn sidebar_workspace_name_width(available_width: f32, detached: bool, accordion: bool) -> f32 {
    let count_reserve = if accordion { 28.0 } else { 0.0 };
    // Badge text plus the explicit 4px gap and egui's item spacing before it.
    let detached_reserve = if detached { 76.0 } else { 0.0 };
    (available_width - count_reserve - detached_reserve - 10.0).max(0.0)
}

pub(super) fn render_sidebar_workspace_row_contents(
    ui: &mut egui::Ui,
    workspace: &WorkspaceSidebarEntry,
    accordion: bool,
) -> SidebarWorkspaceRowInteraction {
    let mut hovered = false;
    let mut clicked = false;

    ui.add_space(14.0);

    let bar_color = theme::alpha(workspace.color, if workspace.is_active { 240 } else { 110 });
    let bar_rect = ui.allocate_space(Vec2::new(3.0, 22.0)).1;
    ui.painter().rect_filled(bar_rect, CornerRadius::same(2), bar_color);

    ui.add_space(8.0);

    let name = egui::RichText::new(&workspace.name)
        .color(if workspace.is_active {
            theme::FG()
        } else {
            theme::FG_SOFT()
        })
        .size(13.0)
        .strong();
    // A sized, left-to-right scope (rather than `add_sized`, which centers)
    // keeps the name flush left while letting long names truncate.
    let name_width = sidebar_workspace_name_width(ui.available_width(), workspace.detached, accordion);
    let name_response = ui
        .allocate_ui_with_layout(
            Vec2::new(name_width, 18.0),
            Layout::left_to_right(Align::Center),
            |ui| ui.add(egui::Label::new(name).truncate().sense(Sense::click())),
        )
        .inner;
    hovered |= name_response.hovered();
    clicked |= name_response.clicked();

    if workspace.detached {
        ui.add_space(4.0);
        let detached_response = ui.add(
            egui::Label::new(
                egui::RichText::new("NEW WINDOW")
                    .color(theme::FG_DIM())
                    .size(8.5)
                    .strong(),
            )
            .sense(Sense::click()),
        );
        hovered |= detached_response.hovered();
        clicked |= detached_response.clicked();
    }

    if accordion {
        let count_response = ui.add(
            egui::Label::new(
                egui::RichText::new(workspace.panels.len().to_string())
                    .color(theme::FG_DIM())
                    .size(11.0),
            )
            .sense(Sense::click()),
        );
        hovered |= count_response.hovered();
        clicked |= count_response.clicked();
    }

    SidebarWorkspaceRowInteraction { hovered, clicked }
}

pub(super) fn paint_workspace_row_bg(
    ui: &mut egui::Ui,
    workspace_rect: Rect,
    workspace_color: Color32,
    is_active: bool,
    hovered: bool,
    dragging: bool,
) {
    let workspace_bg = Rect::from_min_max(
        Pos2::new(workspace_rect.min.x + 6.0, workspace_rect.min.y),
        Pos2::new(workspace_rect.max.x - 6.0, workspace_rect.max.y),
    );
    if dragging {
        ui.painter_at(workspace_bg).rect_filled(
            workspace_bg,
            CornerRadius::same(10),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT(), workspace_color, 0.18), 180),
        );
    } else if is_active {
        ui.painter_at(workspace_bg).rect_filled(
            workspace_bg,
            CornerRadius::same(10),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT(), workspace_color, 0.12), 140),
        );
    } else if hovered {
        ui.painter_at(workspace_bg).rect_filled(
            workspace_bg,
            CornerRadius::same(10),
            theme::alpha(theme::PANEL_BG_ALT(), 160),
        );
    }
}

pub(super) fn paint_workspace_drop_indicator(
    ui: &egui::Ui,
    workspace_rect: Rect,
    insert: SidebarWorkspaceInsert,
    workspace_color: Color32,
) {
    let y = match insert {
        SidebarWorkspaceInsert::Before => workspace_rect.min.y + 1.0,
        SidebarWorkspaceInsert::After => workspace_rect.max.y - 1.0,
    };
    let left = workspace_rect.min.x + 12.0;
    let right = workspace_rect.max.x - 12.0;
    ui.painter().line_segment(
        [Pos2::new(left, y), Pos2::new(right, y)],
        Stroke::new(2.0_f32, theme::alpha(workspace_color, 220)),
    );
}

pub(super) fn paint_panel_row_bg(
    ui: &mut egui::Ui,
    item_rect: Rect,
    workspace_color: Color32,
    is_focused: bool,
    hovered: bool,
) {
    let bg_rect = Rect::from_min_max(
        Pos2::new(item_rect.min.x + 6.0, item_rect.min.y),
        Pos2::new(item_rect.max.x - 6.0, item_rect.max.y),
    );
    if is_focused {
        ui.painter_at(bg_rect).rect_filled(
            bg_rect,
            CornerRadius::same(10),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT(), workspace_color, 0.22), 200),
        );
        let edge = Rect::from_min_size(
            Pos2::new(bg_rect.min.x, bg_rect.min.y + 4.0),
            Vec2::new(2.0, bg_rect.height() - 8.0),
        );
        ui.painter().rect_filled(edge, CornerRadius::same(1), workspace_color);
    } else if hovered {
        ui.painter_at(bg_rect).rect_filled(
            bg_rect,
            CornerRadius::same(10),
            theme::alpha(theme::PANEL_BG_ALT(), 180),
        );
    }
}
