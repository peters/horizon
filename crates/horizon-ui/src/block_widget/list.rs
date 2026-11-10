//! A list of rows a person can select.

use egui::{
    Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo,
};
use horizon_browser_control::block::{ListRow, Selection};

use crate::theme;

use super::metrics::{
    LIST_LINE_GAP, LIST_MARK_GAP, LIST_MARK_RING, LIST_MARK_RING_SELECTED, LIST_MARK_SIZE, LIST_ROW_GAP,
    LIST_ROW_OUTLINE, LIST_ROW_OUTLINE_SELECTED, LIST_ROW_PAD_SIDE, LIST_ROW_PAD_Y, LIST_ROW_RADIUS,
    LIST_SUBTITLE_FONT, LIST_TITLE_FONT,
};
use super::{CardView, text, tones};

/// How much of the accent a selected row blends into its fill.
const SELECTED_TINT: f32 = 0.18;
/// How much of the accent a selected row of a disabled card keeps: enough to
/// read the choice, too little to claim a live control.
const DISABLED_SELECTED_TINT: f32 = 0.09;
/// How much of the accent an outline or a mark of a disabled selected row
/// keeps.
const DISABLED_ACCENT_ALPHA: u8 = 120;
/// How much of the accent a hovered row blends into its fill.
const HOVER_TINT: f32 = 0.10;

/// Draw every row of a `list` component, in document order.
pub(super) fn show(
    ui: &mut Ui,
    index: usize,
    selection: Selection,
    rows: &[ListRow],
    view: &mut CardView<'_>,
    enabled: bool,
) {
    let selectable = selection == Selection::Single && enabled;
    for (row_index, row) in rows.iter().enumerate() {
        if row_index > 0 {
            ui.add_space(LIST_ROW_GAP);
        }
        // A card without its agent takes no press, but the row the person sees
        // as selected keeps that look: the card still shows the choice it
        // holds.
        let selected = selection == Selection::Single && view.card.selected(index) == Some(row.id.as_str());
        show_row(
            ui,
            index,
            row_index,
            row,
            selection,
            RowState {
                selectable,
                selected,
                enabled,
            },
            view,
        );
    }
}

/// How one row of a list is drawn: whether it takes a press, whether it carries
/// the selection of its list, and whether the card that holds it is live.
#[derive(Clone, Copy)]
struct RowState {
    selectable: bool,
    selected: bool,
    enabled: bool,
}

fn show_row(
    ui: &mut Ui,
    index: usize,
    row_index: usize,
    row: &ListRow,
    selection: Selection,
    state: RowState,
    view: &mut CardView<'_>,
) {
    let RowState {
        selectable,
        selected,
        enabled,
    } = state;
    let width = ui.available_width();
    // A selected row keeps its outer size and its text position: the wider
    // outline takes the space of the padding that the row gives up.
    let text_x = LIST_ROW_OUTLINE + LIST_ROW_PAD_SIDE + LIST_MARK_SIZE + LIST_MARK_GAP;
    let text_width = (width - text_x - LIST_ROW_PAD_SIDE - LIST_ROW_OUTLINE).max(0.0);

    let title = text::line(
        ui.painter(),
        &row.title,
        &FontId::proportional(LIST_TITLE_FONT),
        theme::FG(),
        text_width,
    );
    let subtitle_colour = row.tone.map_or_else(theme::FG_SOFT, tones::text);
    let subtitle = row.subtitle.as_ref().map(|subtitle| {
        text::line(
            ui.painter(),
            subtitle,
            &FontId::proportional(LIST_SUBTITLE_FONT),
            subtitle_colour,
            text_width,
        )
    });

    let title_height = title.size().y;
    let subtitle_height = subtitle.as_ref().map_or(0.0, |galley| galley.size().y);
    let mut height = 2.0 * LIST_ROW_OUTLINE + 2.0 * LIST_ROW_PAD_Y + title_height;
    if subtitle.is_some() {
        height += LIST_LINE_GAP + subtitle_height;
    }

    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let sense = if selectable { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, ui.make_persistent_id(("block_row", index, row_index)), sense);
    row_info(&response, &row.title, selection, selectable, selected);
    let hovered = selectable && response.hovered();

    let (fill, outline_stroke) = if selected {
        (
            theme::blend(theme::BG_ELEVATED(), theme::ACCENT(), selected_tint(enabled)),
            Stroke::new(LIST_ROW_OUTLINE_SELECTED, selected_accent(enabled)),
        )
    } else if hovered {
        (
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), HOVER_TINT),
            Stroke::new(LIST_ROW_OUTLINE, theme::BORDER_SUBTLE()),
        )
    } else {
        (
            theme::PANEL_BG_ALT(),
            Stroke::new(LIST_ROW_OUTLINE, theme::BORDER_SUBTLE()),
        )
    };

    let painter = ui.painter();
    let radius = CornerRadius::same(LIST_ROW_RADIUS);
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(rect, radius, outline_stroke, StrokeKind::Inside);

    paint_mark(painter, rect, selected, selected_accent(enabled));
    let text_left = rect.min.x + text_x;
    let text_height = title_height
        + if subtitle.is_some() {
            LIST_LINE_GAP + subtitle_height
        } else {
            0.0
        };
    let mut y = rect.center().y - text_height * 0.5;
    painter.galley(Pos2::new(text_left, y), title, theme::FG());
    if let Some(galley) = subtitle {
        y += title_height + LIST_LINE_GAP;
        painter.galley(Pos2::new(text_left, y), galley, subtitle_colour);
        subtitle_node(ui, index, row_index, rect, y, row);
    }

    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if selectable && response.clicked() && press_row(index, row, view) {
        ui.ctx().request_repaint();
    }
}

/// The accent a selected row of a live card is drawn in.
fn selected_accent(enabled: bool) -> Color32 {
    if enabled {
        theme::ACCENT()
    } else {
        theme::alpha(theme::ACCENT(), DISABLED_ACCENT_ALPHA)
    }
}

/// How much of the accent the fill of a selected row carries.
fn selected_tint(enabled: bool) -> f32 {
    if enabled { SELECTED_TINT } else { DISABLED_SELECTED_TINT }
}

/// The radio mark of a row: a filled accent ring when selected, an empty ring
/// otherwise.
fn paint_mark(painter: &egui::Painter, rect: Rect, selected: bool, accent: Color32) {
    let center = Pos2::new(
        rect.min.x + LIST_ROW_OUTLINE + LIST_ROW_PAD_SIDE + LIST_MARK_SIZE * 0.5,
        rect.center().y,
    );
    let radius = LIST_MARK_SIZE * 0.5;
    if selected {
        painter.circle_filled(center, radius, accent);
        painter.circle_filled(center, radius - LIST_MARK_RING_SELECTED, theme::BG());
    } else {
        painter.circle_stroke(
            center,
            radius - LIST_MARK_RING * 0.5,
            Stroke::new(LIST_MARK_RING, theme::BORDER_STRONG()),
        );
    }
}

fn row_info(response: &egui::Response, title: &str, selection: Selection, selectable: bool, selected: bool) {
    if selection == Selection::Single {
        response.widget_info(|| WidgetInfo::selected(Role::Button, selectable, selected, title));
    } else {
        response.widget_info(|| WidgetInfo::labeled(Role::Label, true, title));
    }
}

fn subtitle_node(ui: &Ui, index: usize, row_index: usize, row: Rect, top: f32, entry: &ListRow) {
    let Some(subtitle) = entry.subtitle.as_deref() else {
        return;
    };
    let rect = Rect::from_min_max(
        Pos2::new(row.min.x, top),
        Pos2::new(row.max.x, top + LIST_SUBTITLE_FONT),
    );
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("block_row_subtitle", index, row_index)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, subtitle));
}

/// Selects the row the person pressed and records the click that goes with it.
/// Returns whether the row took the press.
fn press_row(index: usize, row: &ListRow, view: &mut CardView<'_>) -> bool {
    let (document, revision, now_millis) = (view.document, view.revision, view.now_millis);
    if view.card.select(index, &row.id, document) {
        view.card.press(&row.id, revision, document, now_millis);
        return true;
    }
    false
}
