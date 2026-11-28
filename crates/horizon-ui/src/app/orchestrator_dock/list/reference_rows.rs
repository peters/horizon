//! The reference rows of the list: the row of a card that the program drew, and the
//! row of a work item that the front desk started, which a press brings into view.

use egui::{Color32, CursorIcon, Rangef, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::app::sidebar::rows::paint_workspace_tile;
use crate::theme;
use crate::typeface::Weight;

use super::super::text;
use super::{
    CARD_FILL_ALPHA, CARD_GAP_AFTER_GLYPH, CARD_GLYPH, CARD_GLYPH_LINE_DROP, CARD_GLYPH_LINE_INSET, CARD_GLYPH_RADIUS,
    CARD_GLYPH_STROKE, CARD_HEIGHT, CARD_INDENT, CARD_OUTLINE_ALPHA, CARD_PADDING_X, CARD_RADIUS, CARD_SHARE,
    CARD_STATE_SIZE, CARD_TITLE_SIZE, WORK_ITEM_STATE_SIZE, WORK_ITEM_TILE, WorkItemRow,
};

/// One reference row of a work item that the person started from a proposal: the
/// mark of its workspace, its name and the state word of that work item. A work
/// item that the board no longer holds reads dim and takes no press, as the row of
/// a card that is gone does.
pub(super) fn work_item_row(ui: &mut Ui, item: &WorkItemRow, width: f32) -> bool {
    let WorkItemRow {
        work_item,
        name,
        held,
        word,
        accent,
    } = item;
    let row_width = (width * CARD_SHARE).min(width - CARD_INDENT).max(1.0);
    let (row, _) = ui.allocate_exact_size(vec2(width, CARD_HEIGHT), Sense::hover());
    let rect = Rect::from_min_size(pos2(row.min.x + CARD_INDENT, row.min.y), vec2(row_width, row.height()));
    let present = *held;
    let sense = if present { Sense::click() } else { Sense::hover() };
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("orchestrator_dock_work_item", work_item)),
        sense,
    );
    let hovered = present && response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let word_galley = word.map(|word| {
        text::line(
            ui,
            word,
            WORK_ITEM_STATE_SIZE,
            Weight::Regular,
            theme::FG_DIM(),
            row_width,
        )
    });
    let tile_left = rect.min.x + CARD_PADDING_X;
    let name_left = tile_left + WORK_ITEM_TILE + CARD_GAP_AFTER_GLYPH;
    let word_width = word_galley
        .as_ref()
        .map_or(0.0, |galley| galley.size().x + CARD_GAP_AFTER_GLYPH);
    let name_width = (rect.max.x - CARD_PADDING_X - word_width - name_left).max(1.0);
    let name_color = if present { theme::FG() } else { theme::FG_DIM() };
    let name_galley = text::line(ui, name, CARD_TITLE_SIZE, Weight::Medium, name_color, name_width);
    let (fill, outline) = if present {
        let outline = if hovered {
            theme::ACCENT()
        } else {
            theme::alpha(theme::ACCENT(), CARD_OUTLINE_ALPHA)
        };
        (theme::alpha(theme::ACCENT(), CARD_FILL_ALPHA), outline)
    } else {
        (Color32::TRANSPARENT, theme::BORDER_SUBTLE())
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        CARD_RADIUS,
        fill,
        Stroke::new(1.0_f32, outline),
        StrokeKind::Inside,
    );
    let tile = Rect::from_min_size(
        pos2(tile_left, rect.center().y - WORK_ITEM_TILE / 2.0),
        vec2(WORK_ITEM_TILE, WORK_ITEM_TILE),
    );
    paint_workspace_tile(ui, tile, name, *accent, present);
    painter.galley(
        pos2(name_left, rect.center().y - name_galley.size().y / 2.0),
        name_galley,
        name_color,
    );
    if let Some(word_galley) = word_galley {
        painter.galley(
            pos2(
                rect.max.x - CARD_PADDING_X - word_galley.size().x,
                rect.center().y - word_galley.size().y / 2.0,
            ),
            word_galley,
            theme::FG_DIM(),
        );
    }
    present && response.clicked()
}

/// The empty row of a work item that the facts of this frame do not hold, so the
/// list keeps its height. It draws nothing and takes no press.
pub(super) fn missing_work_item_row(ui: &mut Ui, width: f32) {
    let _ = ui.allocate_exact_size(vec2(width, CARD_HEIGHT), Sense::hover());
}

/// One reference row of a panel that the program drew: the glyph of a panel, the
/// heading of the row and a state word at its right end, with a fill and an
/// outline of the accent. A panel that the board no longer holds reads dim and
/// takes no press. The row of a card and the row of a worker take this one form.
pub(in super::super) fn reference_row(
    ui: &mut Ui,
    id: (&'static str, &str),
    title: &str,
    state_text: &str,
    width: f32,
    present: bool,
) -> bool {
    let row_width = (width * CARD_SHARE).min(width - CARD_INDENT).max(1.0);
    let (row, _) = ui.allocate_exact_size(vec2(width, CARD_HEIGHT), Sense::hover());
    let rect = Rect::from_min_size(pos2(row.min.x + CARD_INDENT, row.min.y), vec2(row_width, row.height()));
    let persistent_id = ui.make_persistent_id((id.0, id.1));
    let sense = if present { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, persistent_id, sense);
    let hovered = present && response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let state_galley = text::line(
        ui,
        state_text,
        CARD_STATE_SIZE,
        Weight::Regular,
        theme::FG_DIM(),
        row_width,
    );
    let glyph_left = rect.min.x + CARD_PADDING_X;
    let title_left = glyph_left + CARD_GLYPH[0] + CARD_GAP_AFTER_GLYPH;
    let title_width =
        (rect.max.x - CARD_PADDING_X - state_galley.size().x - CARD_GAP_AFTER_GLYPH - title_left).max(1.0);
    let title_color = if present { theme::FG() } else { theme::FG_DIM() };
    let title_galley = text::line(ui, title, CARD_TITLE_SIZE, Weight::Medium, title_color, title_width);
    let (fill, outline) = if present {
        let outline = if hovered {
            theme::ACCENT()
        } else {
            theme::alpha(theme::ACCENT(), CARD_OUTLINE_ALPHA)
        };
        (theme::alpha(theme::ACCENT(), CARD_FILL_ALPHA), outline)
    } else {
        (Color32::TRANSPARENT, theme::BORDER_SUBTLE())
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        CARD_RADIUS,
        fill,
        Stroke::new(1.0_f32, outline),
        StrokeKind::Inside,
    );
    let glyph = Rect::from_min_size(
        pos2(glyph_left, rect.center().y - CARD_GLYPH[1] / 2.0),
        vec2(CARD_GLYPH[0], CARD_GLYPH[1]),
    );
    let glyph_stroke = Stroke::new(CARD_GLYPH_STROKE, theme::ACCENT());
    painter.rect_stroke(glyph, CARD_GLYPH_RADIUS, glyph_stroke, StrokeKind::Inside);
    painter.hline(
        Rangef::new(glyph.min.x + CARD_GLYPH_LINE_INSET, glyph.max.x - CARD_GLYPH_LINE_INSET),
        glyph.min.y + CARD_GLYPH_LINE_DROP,
        glyph_stroke,
    );
    painter.galley(
        pos2(title_left, rect.center().y - title_galley.size().y / 2.0),
        title_galley,
        title_color,
    );
    painter.galley(
        pos2(
            rect.max.x - CARD_PADDING_X - state_galley.size().x,
            rect.center().y - state_galley.size().y / 2.0,
        ),
        state_galley,
        theme::FG_DIM(),
    );
    present && response.clicked()
}
