//! The panel rows under a workspace: a tree guide from the workspace tile, the
//! kind label, the panel's title, the one word of its attention state and, at
//! the right, a working dot or the close button while the row is hovered or
//! focused.
use egui::{Rect, Stroke, Vec2, pos2};

use crate::app::panels::panel_kind_icon;
use crate::theme;

use super::rows::{self, TILE_INSET, TILE_SIDE};
use super::text;
use super::{SidebarPanelEntry, WorkspaceSidebarEntry};

pub(super) const PANEL_ROW_HEIGHT: f32 = 30.0;
const CLOSE_SIDE: f32 = 20.0;
/// The room that the one word of a state takes at the right of a panel row.
pub(super) const STATE_WIDTH: f32 = 120.0;
/// The gap between the title and the word of its state.
pub(super) const STATE_GAP: f32 = 6.0;
/// The font of the word of a state, the size of the kind icon beside it. The
/// longest word of a state fits half of a default row at this size.
const STATE_SIZE: f32 = 9.5;
/// The largest share of a row that the word of a state takes. The name of the
/// panel keeps the other share, so the two texts never cover each other.
const STATE_SHARE: f32 = 0.5;

/// The close button of a panel row, at the right of its card.
pub(super) fn close_rect(row: Rect) -> Rect {
    let card = rows::card(row);
    Rect::from_center_size(pos2(card.max.x - 13.0, row.center().y), Vec2::splat(CLOSE_SIDE))
}

/// The pointer and list state of a panel row.
#[derive(Clone, Copy)]
pub(super) struct PanelRowState {
    pub hovered: bool,
    /// `Some(hovered)` while the close button shows.
    pub close: Option<bool>,
    /// The last row ends the tree guide.
    pub last: bool,
}

/// Where the title of a panel row ends, where the word of its state starts and
/// how wide that word may be, from the right edge that the row's text may use.
///
/// The word of a state takes at most half of the row and the title keeps the
/// rest less the gap, so the two texts never cover each other. An idle row has no
/// word, so its title keeps the whole width.
pub(super) fn title_and_state(right: f32, title_left: f32, state_width: f32) -> Option<(f32, f32, f32)> {
    if state_width <= 0.0 {
        return None;
    }
    let room = (right - title_left).max(0.0);
    let width = state_width.min(room * STATE_SHARE);
    let state_left = right - width;
    Some(((state_left - STATE_GAP).max(title_left), state_left, width))
}

/// Paints a panel row. Returns whether the title was cut to fit.
pub(super) fn paint_panel_row(
    ui: &egui::Ui,
    row: Rect,
    workspace: &WorkspaceSidebarEntry,
    panel: &SidebarPanelEntry,
    label: &str,
    state: PanelRowState,
) -> bool {
    let PanelRowState { hovered, close, last } = state;
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
    let mark = close_rect(row).center();
    let state = panel.attention.word();
    // The word of a state, when the row keeps the room for it, ends the title.
    let state_width = state.map_or(0.0, |word| {
        let galley = text::line(ui, word, STATE_SIZE, text::REGULAR, theme::FG_DIM(), STATE_WIDTH);
        galley.size().x
    });
    let state_room = title_and_state(card.max.x - 26.0, title_left, state_width);
    let title_right = state_room.map_or(card.max.x - 26.0, |room| room.0);
    let title = text::line(ui, label, 12.5, text::MEDIUM, color, title_right - title_left);
    let title_elided = title.elided;
    painter.galley(pos2(title_left, center_y - title.size().y / 2.0), title, color);

    if let (Some(word), Some((_, state_left, room))) = (state, state_room) {
        let color = match panel.attention {
            horizon_core::Attention::NeedsYou => theme::PALETTE_YELLOW(),
            horizon_core::Attention::Done => theme::ACCENT(),
            _ => theme::FG_DIM(),
        };
        let galley = text::line(ui, word, STATE_SIZE, text::REGULAR, color, room);
        painter.galley(pos2(state_left, center_y - galley.size().y / 2.0), galley, color);
    }
    match close {
        Some(close_hovered) => {
            let color = if close_hovered { theme::FG() } else { theme::FG_DIM() };
            if close_hovered {
                painter.circle_filled(mark, CLOSE_SIDE / 2.0 - 1.0, theme::alpha(theme::BORDER_STRONG(), 160));
            }
            let cross = text::line(ui, "\u{00D7}", 15.0, text::REGULAR, color, CLOSE_SIDE);
            painter.galley(mark - cross.size() / 2.0, cross, color);
        }
        None if panel.attention.rank() == horizon_core::Attention::Working.rank() => {
            painter.circle_filled(mark, 3.0, theme::PALETTE_GREEN());
        }
        None => {}
    }
    title_elided
}
