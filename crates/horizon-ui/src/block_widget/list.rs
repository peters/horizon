//! A list of rows a person can select.
//!
//! A row is drawn one under another with the mark of its list selection, or as
//! one small pill of a line when its list draws pills. A row carries a short
//! value or a badge at its right edge, and a row the document disables says what
//! is missing and takes no press.

use egui::{
    Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect, Role, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, WidgetInfo,
};
use horizon_browser_control::block::{ListLayout, ListRow, Selection};

use crate::theme;
use crate::typeface::Weight;

use super::badge;
use super::metrics::{
    LIST_LINE_GAP, LIST_MARK_GAP, LIST_MARK_RING, LIST_MARK_RING_SELECTED, LIST_MARK_SIZE, LIST_ROW_GAP,
    LIST_ROW_OUTLINE, LIST_ROW_OUTLINE_SELECTED, LIST_ROW_PAD_SIDE, LIST_ROW_PAD_Y, LIST_SUBTITLE_FONT,
    LIST_TITLE_FONT, LIST_VALUE_FONT, LIST_VALUE_GAP, PILL_FONT, PILL_GAP, PILL_HEIGHT, PILL_LINE_GAP, PILL_PAD_SIDE,
    RADIUS_CONTROL,
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
/// The length of one dash of the outline of a row that cannot be chosen.
const DASH: f32 = 4.0;
/// The length of the gap between two dashes of that outline.
const DASH_GAP: f32 = 3.0;
/// How many segments one corner of a rounded rectangle takes when its outline
/// is dashed.
const CORNER_SEGMENTS: usize = 4;

/// Draw every row of a `list` component, in document order.
pub(super) fn show(
    ui: &mut Ui,
    index: usize,
    selection: Selection,
    rows: &[ListRow],
    layout: ListLayout,
    view: &mut CardView<'_>,
    enabled: bool,
) {
    match layout {
        ListLayout::Rows => show_rows(ui, index, selection, rows, view, enabled),
        ListLayout::Pills => show_pills(ui, index, selection, rows, view, enabled),
    }
}

fn show_rows(
    ui: &mut Ui,
    index: usize,
    selection: Selection,
    rows: &[ListRow],
    view: &mut CardView<'_>,
    enabled: bool,
) {
    let selectable = selection == Selection::Single && enabled;
    // A list that offers no selection draws no ring, so its text takes the room
    // that the ring keeps.
    let marked = selection == Selection::Single;
    let text_x = text_left(marked);
    let tallest = rows
        .iter()
        .map(|row| {
            let edge = row_edge(ui, row, row.disabled);
            let room = (ui.available_width() - text_x - LIST_ROW_PAD_SIDE - LIST_ROW_OUTLINE - edge.width).max(0.0);
            2.0 * LIST_ROW_OUTLINE + 2.0 * LIST_ROW_PAD_Y + row_text_height(ui, row, room)
        })
        .fold(0.0, f32::max);
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
            RowState {
                selectable: selectable && !row.disabled,
                selected,
                enabled,
                marked,
            },
            view,
            tallest,
        );
    }
}

/// The left edge of the text of a row: the outline, the padding and, in a list
/// that offers a selection, the ring and the gap after it.
fn text_left(marked: bool) -> f32 {
    LIST_ROW_OUTLINE + LIST_ROW_PAD_SIDE + if marked { LIST_MARK_SIZE + LIST_MARK_GAP } else { 0.0 }
}

/// The height that the two lines of one row take in `room` points, so every row
/// of one list can take the height of its tallest row.
fn row_text_height(ui: &Ui, row: &ListRow, room: f32) -> f32 {
    let title = text::wrapped_weight(
        ui.painter(),
        &row.title,
        &FontId::proportional(LIST_TITLE_FONT),
        theme::FG(),
        room,
        None,
        Weight::Semibold,
    );
    let mut height = title.size().y;
    if let Some(subtitle) = &row.subtitle {
        let subtitle = text::wrapped(
            ui.painter(),
            subtitle,
            &FontId::proportional(LIST_SUBTITLE_FONT),
            theme::FG_SOFT(),
            room,
            None,
        );
        height += LIST_LINE_GAP + subtitle.size().y;
    }
    height
}

/// How one row of a list is drawn: whether it takes a press, whether it carries
/// the selection of its list, and whether the card that holds it is live.
#[derive(Clone, Copy)]
struct RowState {
    selectable: bool,
    selected: bool,
    enabled: bool,
    marked: bool,
}

fn show_row(
    ui: &mut Ui,
    index: usize,
    row_index: usize,
    row: &ListRow,
    state: RowState,
    view: &mut CardView<'_>,
    tallest: f32,
) {
    let RowState {
        selectable,
        selected,
        enabled,
        marked,
    } = state;
    let width = ui.available_width();
    // A selected row keeps its outer size and its text position: the wider
    // outline takes the space of the padding that the row gives up.
    let text_x = text_left(marked);
    let edge = row_edge(ui, row, row.disabled);
    let text_width = (width - text_x - LIST_ROW_PAD_SIDE - LIST_ROW_OUTLINE - edge.width).max(0.0);

    let title_colour = if row.disabled { theme::FG_SOFT() } else { theme::FG() };
    // The text of a row keeps the lines that its words need: a row is a choice
    // and a name with its reason, and neither loses a word to the edge of the
    // card. The row grows with them, and its value or badge keeps the right edge.
    let title = text::wrapped_weight(
        ui.painter(),
        &row.title,
        &FontId::proportional(LIST_TITLE_FONT),
        title_colour,
        text_width,
        None,
        Weight::Semibold,
    );
    let subtitle_colour = row.tone.map_or_else(theme::FG_SOFT, tones::text);
    let subtitle = row.subtitle.as_ref().map(|subtitle| {
        text::wrapped(
            ui.painter(),
            subtitle,
            &FontId::proportional(LIST_SUBTITLE_FONT),
            subtitle_colour,
            text_width,
            None,
        )
    });

    let title_height = title.size().y;
    let subtitle_height = subtitle.as_ref().map_or(0.0, |galley| galley.size().y);
    let mut height = 2.0 * LIST_ROW_OUTLINE + 2.0 * LIST_ROW_PAD_Y + title_height;
    if subtitle.is_some() {
        height += LIST_LINE_GAP + subtitle_height;
    }
    height = height.max(edge.height).max(tallest);

    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let sense = if selectable { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, ui.make_persistent_id(("block_row", index, row_index)), sense);
    row_info(&response, row, marked, selectable, selected);
    let hovered = selectable && response.hovered();

    let (fill, outline_stroke) = if row.disabled && !selected {
        (Color32::TRANSPARENT, Stroke::NONE)
    } else if selected {
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
    let radius = CornerRadius::same(RADIUS_CONTROL);
    if fill.a() > 0 {
        painter.rect_filled(rect, radius, fill);
    }
    if row.disabled && !selected {
        // A row that cannot be chosen is drawn as an empty frame: the dashes say
        // that nothing here takes a press.
        let dashes = dashed_outline(
            rect,
            f32::from(RADIUS_CONTROL),
            Stroke::new(LIST_ROW_OUTLINE, theme::BORDER_STRONG()),
        );
        painter.extend(dashes);
    } else {
        painter.rect_stroke(rect, radius, outline_stroke, StrokeKind::Inside);
    }

    if marked && !row.disabled {
        paint_mark(painter, rect, selected, selected_accent(enabled));
    }
    let text_left = rect.min.x + text_x;
    let text_height = title_height
        + if subtitle.is_some() {
            LIST_LINE_GAP + subtitle_height
        } else {
            0.0
        };
    let mut y = rect.center().y - text_height * 0.5;
    painter.galley(Pos2::new(text_left, y), title, title_colour);
    if let Some(galley) = subtitle {
        y += title_height + LIST_LINE_GAP;
        painter.galley(Pos2::new(text_left, y), galley, subtitle_colour);
        subtitle_node(ui, index, row_index, rect, y, row);
    }
    edge.paint(ui, index, row_index, rect, row);

    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if selectable && response.clicked() && press_row(index, row, view) {
        ui.ctx().request_repaint();
    }
}

/// The right edge of a row: its value in the monospace family and its badge,
/// right aligned in that order.
struct RowEdge {
    value: Option<std::sync::Arc<egui::Galley>>,
    badge: Option<(f32, horizon_browser_control::block::Tone)>,
    width: f32,
    height: f32,
}

impl RowEdge {
    fn paint(&self, ui: &Ui, index: usize, row_index: usize, rect: Rect, row: &ListRow) {
        let mut right = rect.max.x - LIST_ROW_PAD_SIDE - LIST_ROW_OUTLINE;
        if let Some((pill, tone)) = self.badge.as_ref() {
            let pill_rect = Rect::from_min_size(
                Pos2::new(right - pill, rect.center().y - super::metrics::BADGE_HEIGHT * 0.5),
                Vec2::new(*pill, super::metrics::BADGE_HEIGHT),
            );
            let text = row.badge.as_ref().map_or("", |badge| badge.text.as_str());
            badge::paint(ui.painter(), pill_rect, text, None, *tone);
            badge::node(
                ui,
                ui.make_persistent_id(("block_row_badge", index, row_index)),
                pill_rect,
                text,
                None,
            );
            right = pill_rect.min.x - LIST_VALUE_GAP;
        }
        if let Some(galley) = &self.value {
            let height = galley.size().y;
            let width = galley.size().x;
            let value = Rect::from_min_size(Pos2::new(right - width, rect.center().y - height * 0.5), galley.size());
            ui.painter().galley(value.min, galley.clone(), theme::FG_SOFT());
            let text = row.value.as_deref().unwrap_or_default();
            ui.interact(
                value,
                ui.make_persistent_id(("block_row_value", index, row_index)),
                Sense::hover(),
            )
            .widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
        }
    }
}

/// The value and the badge that the right edge of `row` takes, with the width
/// and height they need.
fn row_edge(ui: &Ui, row: &ListRow, disabled: bool) -> RowEdge {
    let value = row.value.as_ref().map(|value| {
        text::line(
            ui.painter(),
            value,
            &FontId::monospace(LIST_VALUE_FONT),
            theme::FG_SOFT(),
            (ui.available_width() * 0.5).max(0.0),
        )
    });
    let badge = row.badge.as_ref().map(|badge| {
        // A row that takes no press claims no state of its own: its pill keeps
        // its words and loses the tone that a live row would show.
        let tone = if disabled {
            horizon_browser_control::block::Tone::Neutral
        } else {
            badge.tone
        };
        (badge::width(ui, &badge.text, None, tone), tone)
    });
    let value_width = value.as_ref().map_or(0.0, |galley| galley.size().x);
    let badge_width = badge.as_ref().map_or(0.0, |(width, _)| *width);
    let gaps = match (value.is_some(), badge.is_some()) {
        (true, true) => 2.0 * LIST_VALUE_GAP,
        (false, false) => 0.0,
        _ => LIST_VALUE_GAP,
    };
    RowEdge {
        width: value_width + badge_width + gaps,
        height: if row.badge.is_some() {
            super::metrics::BADGE_HEIGHT
        } else {
            0.0
        },
        value,
        badge,
    }
}

/// Draws the rows of a list that draws pills: a line of small pills that wraps
/// when the card is narrow. Each pill carries the title of its row.
fn show_pills(
    ui: &mut Ui,
    index: usize,
    selection: Selection,
    rows: &[ListRow],
    view: &mut CardView<'_>,
    enabled: bool,
) {
    let width = ui.available_width();
    let font = FontId::proportional(PILL_FONT);
    let pills: Vec<_> = rows
        .iter()
        .map(|row| {
            let galley = text::line(ui.painter(), &row.title, &font, theme::FG(), width);
            let pill_width = (galley.size().x + 2.0 * PILL_PAD_SIDE).min(width).max(0.0);
            (galley, pill_width)
        })
        .collect();
    let widths: Vec<f32> = pills.iter().map(|(_, width)| *width).collect();
    let lines = pill_lines(&widths, width);
    let count = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let height = f32::from(count) * PILL_HEIGHT + f32::from(count.saturating_sub(1)) * PILL_LINE_GAP;
    let (block, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());

    for (line_index, line) in lines.iter().enumerate() {
        let top = block.min.y + u16::try_from(line_index).map_or(0.0, f32::from) * (PILL_HEIGHT + PILL_LINE_GAP);
        for (row_index, left) in line {
            let pill = Rect::from_min_size(
                Pos2::new(block.min.x + left, top),
                Vec2::new(pills[*row_index].1, PILL_HEIGHT),
            );
            show_pill(
                ui,
                index,
                *row_index,
                rows,
                selection,
                pill,
                &pills[*row_index].0,
                view,
                enabled,
            );
        }
    }
}

/// The pills of one line, as `(row index, left offset)` in draw order, for the
/// pill widths that fit a card `width` wide.
fn pill_lines(widths: &[f32], width: f32) -> Vec<Vec<(usize, f32)>> {
    let mut lines: Vec<Vec<(usize, f32)>> = Vec::new();
    let mut line: Vec<(usize, f32)> = Vec::new();
    let mut x = 0.0_f32;
    for (row_index, pill) in widths.iter().enumerate() {
        if !line.is_empty() && x + pill > width {
            lines.push(std::mem::take(&mut line));
            x = 0.0;
        }
        line.push((row_index, x));
        x += pill + PILL_GAP;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[allow(clippy::too_many_arguments)]
fn show_pill(
    ui: &mut Ui,
    index: usize,
    row_index: usize,
    rows: &[ListRow],
    selection: Selection,
    pill: Rect,
    title: &std::sync::Arc<egui::Galley>,
    view: &mut CardView<'_>,
    enabled: bool,
) {
    let row = &rows[row_index];
    let selectable = selection == Selection::Single && enabled && !row.disabled;
    let selected = selection == Selection::Single && view.card.selected(index) == Some(row.id.as_str());
    let sense = if selectable { Sense::click() } else { Sense::hover() };
    let response = ui.interact(pill, ui.make_persistent_id(("block_pill", index, row_index)), sense);
    row_info(&response, row, selection == Selection::Single, selectable, selected);
    let hovered = selectable && response.hovered();
    let radius = CornerRadius::same(RADIUS_CONTROL);
    let painter = ui.painter();
    if selected {
        painter.rect_filled(
            pill,
            radius,
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), selected_tint(enabled)),
        );
        painter.rect_stroke(
            pill,
            radius,
            Stroke::new(1.0, selected_accent(enabled)),
            StrokeKind::Inside,
        );
    } else {
        painter.rect_filled(pill, radius, theme::PANEL_BG_ALT());
        let outline = if hovered {
            theme::ACCENT()
        } else {
            theme::BORDER_SUBTLE()
        };
        painter.rect_stroke(pill, radius, Stroke::new(1.0, outline), StrokeKind::Inside);
    }
    let colour = if selected { theme::FG() } else { theme::FG_SOFT() };
    let cut = title.elided;
    painter.galley(
        Pos2::new(
            pill.center().x - title.size().x * 0.5,
            pill.center().y - title.size().y * 0.5,
        ),
        title.clone(),
        colour,
    );
    if cut {
        response.clone().on_hover_text(&row.title);
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

/// The outline of a rounded rectangle as dashes, which egui draws only along a
/// path: the path goes around the rectangle with a quarter circle at each
/// corner.
fn dashed_outline(rect: Rect, radius: f32, stroke: Stroke) -> Vec<Shape> {
    let radius = radius.min(rect.width() * 0.5).min(rect.height() * 0.5).max(0.0);
    let mut path = Vec::with_capacity(4 * CORNER_SEGMENTS + 5);
    let corners = [
        (
            Pos2::new(rect.max.x - radius, rect.min.y + radius),
            -0.5 * std::f32::consts::PI,
        ),
        (Pos2::new(rect.max.x - radius, rect.max.y - radius), 0.0),
        (
            Pos2::new(rect.min.x + radius, rect.max.y - radius),
            0.5 * std::f32::consts::PI,
        ),
        (
            Pos2::new(rect.min.x + radius, rect.min.y + radius),
            std::f32::consts::PI,
        ),
    ];
    for (centre, start) in corners {
        for segment in 0..=CORNER_SEGMENTS {
            #[allow(clippy::cast_precision_loss)]
            let angle = start + std::f32::consts::FRAC_PI_2 * segment as f32 / CORNER_SEGMENTS as f32;
            path.push(Pos2::new(
                centre.x + radius * angle.cos(),
                centre.y + radius * angle.sin(),
            ));
        }
    }
    path.push(path[0]);
    Shape::dashed_line(&path, stroke, DASH, DASH_GAP)
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

fn row_info(response: &egui::Response, row: &ListRow, marked: bool, selectable: bool, selected: bool) {
    if marked {
        response.widget_info(|| WidgetInfo::selected(Role::Button, selectable, selected, &row.title));
    } else {
        response.widget_info(|| WidgetInfo::labeled(Role::Label, selectable, &row.title));
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
