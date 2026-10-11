//! The panel rows under a workspace: a tree guide from the workspace tile, the
//! kind label, the panel's title and, at the right, a working dot or the close
//! button while the row is hovered or focused.
use egui::{Rect, Stroke, Vec2, pos2};

use crate::app::panels::panel_kind_icon;
use crate::theme;

use super::rows::{self, TILE_INSET, TILE_SIDE};
use super::text;
use super::{SidebarPanelEntry, WorkspaceSidebarEntry};

pub(super) const PANEL_ROW_HEIGHT: f32 = 30.0;
const CLOSE_SIDE: f32 = 20.0;

/// The close button of a panel row, at the right of its card.
pub(super) fn close_rect(row: Rect) -> Rect {
    let card = rows::card(row);
    Rect::from_center_size(pos2(card.max.x - 13.0, row.center().y), Vec2::splat(CLOSE_SIDE))
}

/// Paints a panel row. `close` is `Some(hovered)` while the close button shows.
/// Returns whether the title was cut to fit.
pub(super) fn paint_panel_row(
    ui: &egui::Ui,
    row: Rect,
    workspace: &WorkspaceSidebarEntry,
    panel: &SidebarPanelEntry,
    hovered: bool,
    close: Option<bool>,
    last: bool,
) -> bool {
    let painter = ui.painter();
    let card = rows::card(row);
    let center_y = row.center().y;

    let guide_x = card.min.x + TILE_INSET + TILE_SIDE / 2.0;
    let guide = Stroke::new(1.0_f32, theme::alpha(theme::BORDER_STRONG(), 200));
    let bottom = if last { center_y } else { row.max.y };
    painter.line_segment([pos2(guide_x, row.min.y), pos2(guide_x, bottom)], guide);
    painter.line_segment([pos2(guide_x, center_y), pos2(guide_x + 9.0, center_y)], guide);
    let left = guide_x + 13.0;

    let highlight = Rect::from_min_max(pos2(left - 6.0, row.min.y + 1.0), pos2(card.max.x, row.max.y - 1.0));
    if panel.is_focused {
        painter.rect_filled(highlight, 8, theme::blend(theme::PANEL_BG_ALT(), workspace.color, 0.20));
    } else if hovered {
        painter.rect_filled(highlight, 8, theme::alpha(theme::PANEL_BG_ALT(), 170));
    }

    let (icon, icon_color) = panel_kind_icon(panel.kind, workspace.color, true);
    let icon = text::mono(ui, icon, 9.5, icon_color);
    let title_left = left + icon.size().x.max(18.0) + 6.0;
    painter.galley(pos2(left, center_y - icon.size().y / 2.0), icon, icon_color);

    let color = if panel.is_focused {
        theme::FG()
    } else {
        theme::FG_SOFT()
    };
    let title = text::line(
        ui,
        &panel.label,
        12.5,
        text::MEDIUM,
        color,
        card.max.x - 26.0 - title_left,
    );
    let title_elided = title.elided;
    painter.galley(pos2(title_left, center_y - title.size().y / 2.0), title, color);

    let mark = close_rect(row).center();
    match close {
        Some(close_hovered) => {
            let color = if close_hovered { theme::FG() } else { theme::FG_DIM() };
            if close_hovered {
                painter.circle_filled(mark, CLOSE_SIDE / 2.0 - 1.0, theme::alpha(theme::BORDER_STRONG(), 160));
            }
            let cross = text::line(ui, "\u{00D7}", 15.0, text::REGULAR, color, CLOSE_SIDE);
            painter.galley(mark - cross.size() / 2.0, cross, color);
        }
        None if panel.working => {
            painter.circle_filled(mark, 3.0, theme::PALETTE_GREEN());
        }
        None => {}
    }
    title_elided
}
