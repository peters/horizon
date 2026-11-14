//! One item of a board: its fill and outline, the mark of its state, its title
//! with its tag, its value at the right edge and its evidence under them.

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::BoardState;

use crate::theme;
use crate::typeface::Weight;

use super::super::metrics::{
    ITEM_BORDER, ITEM_DONE_NOTE_TINT, ITEM_HALO, ITEM_HALO_ALPHA, ITEM_LINE_GAP, ITEM_MARK, ITEM_MARK_GAP,
    ITEM_MARK_RING, ITEM_PAD_SIDE, ITEM_PAD_Y, ITEM_TAG_FONT, ITEM_TAG_GAP, ITEM_TAG_PAD, ITEM_TITLE_FONT,
    ITEM_VALUE_FONT, ITEM_VALUE_GAP, RADIUS_CONTROL, RADIUS_TAG,
};
use super::super::text;

/// How much of the accent or the warn colour the fill of an item carries.
const TINT_ALPHA: u8 = 20;
/// How much of the accent or the warn colour the outline of an item carries.
const OUTLINE_ALPHA: u8 = 175;
/// Width of the mark of a state, as a stroke.
const MARK_STROKE: f32 = 2.0;

/// One item of a board, as the parts of its two lines.
pub(super) struct Chip<'a> {
    pub(super) title: &'a str,
    pub(super) value: Option<&'a str>,
    pub(super) tag: Option<&'a str>,
    pub(super) note: &'a str,
    pub(super) state: BoardState,
    pub(super) place: Option<&'a str>,
}

impl Chip<'_> {
    /// The whole text of the item, for the tooltip of a text that was cut.
    fn whole(&self) -> String {
        let mut text = String::from(self.title);
        if let Some(tag) = self.tag {
            text.push_str(" · ");
            text.push_str(tag);
        }
        if let Some(value) = self.value {
            text.push_str("  ");
            text.push_str(value);
        }
        if !self.note.is_empty() {
            text.push('\n');
            if let Some(place) = self.place {
                text.push_str(place);
                text.push_str(" · ");
            }
            text.push_str(self.note);
        }
        text
    }
}

/// The colours of one item, from the state that it states.
pub(super) struct ChipColours {
    fill: Color32,
    outline: Color32,
    title: Color32,
    value: Color32,
    pub(super) note: Color32,
}

pub(super) fn colours(state: BoardState) -> ChipColours {
    let (tint, fill) = match state {
        BoardState::Now => (theme::ACCENT(), theme::alpha(theme::ACCENT(), TINT_ALPHA)),
        BoardState::NeedsYou => (
            theme::PALETTE_YELLOW(),
            theme::alpha(theme::PALETTE_YELLOW(), TINT_ALPHA),
        ),
        _ => (theme::ACCENT(), theme::PANEL_BG()),
    };
    let outline = match state {
        BoardState::Now | BoardState::NeedsYou => theme::alpha(tint, OUTLINE_ALPHA),
        BoardState::Failed => theme::PALETTE_RED(),
        _ => theme::BORDER_SUBTLE(),
    };
    let value = match state {
        BoardState::NeedsYou => theme::PALETTE_YELLOW(),
        _ => theme::FG_SOFT(),
    };
    let note = match state {
        BoardState::Done => theme::blend(theme::FG_DIM(), theme::PALETTE_GREEN(), ITEM_DONE_NOTE_TINT),
        _ => theme::FG_DIM(),
    };
    let title = if state == BoardState::Waiting {
        theme::FG_SOFT()
    } else {
        theme::FG()
    };
    ChipColours {
        fill,
        outline,
        title,
        note,
        value,
    }
}

/// Paint one item: its fill, its outline, the mark of its state, its title with
/// its tag, its value at the right edge and its evidence under them.
pub(super) fn paint_chip(painter: &egui::Painter, rect: Rect, chip: &Chip<'_>, note: &std::sync::Arc<egui::Galley>) {
    let colours = colours(chip.state);
    let radius = CornerRadius::same(RADIUS_CONTROL);
    painter.rect_filled(rect, radius, colours.fill);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(ITEM_BORDER, colours.outline),
        StrokeKind::Inside,
    );
    let top = rect.min.y + ITEM_BORDER + ITEM_PAD_Y;
    let left = rect.min.x + ITEM_BORDER + ITEM_PAD_SIDE;
    let right = rect.max.x - ITEM_BORDER - ITEM_PAD_SIDE;
    paint_mark(
        painter,
        Pos2::new(left + ITEM_MARK * 0.5, top + ITEM_MARK * 0.5),
        chip.state,
    );
    let title_left = left + ITEM_MARK + ITEM_MARK_GAP;
    let value = chip.value.map(|value| {
        text::line(
            painter,
            value,
            &FontId::monospace(ITEM_VALUE_FONT),
            colours.value,
            f32::INFINITY,
        )
    });
    let value_width = value.as_ref().map_or(0.0, |galley| galley.size().x);
    let edge = if value.is_some() {
        ITEM_VALUE_GAP + value_width
    } else {
        0.0
    };
    let tag_width = chip.tag.map_or(0.0, |tag| {
        text::line_weight(
            painter,
            tag,
            &FontId::monospace(ITEM_TAG_FONT),
            colours.value,
            f32::INFINITY,
            Weight::Medium,
        )
        .size()
        .x + 2.0 * ITEM_TAG_PAD
            + ITEM_TAG_GAP
    });
    let title_room = (right - title_left - edge - tag_width).max(0.0);
    let title = text::line_weight(
        painter,
        chip.title,
        &FontId::proportional(ITEM_TITLE_FONT),
        colours.title,
        title_room,
        Weight::Semibold,
    );
    let title_y = top + (ITEM_MARK - title.size().y) * 0.5;
    painter.galley(Pos2::new(title_left, title_y), title.clone(), colours.title);
    if let Some(tag) = chip.tag {
        paint_tag(painter, Pos2::new(title_left + title.size().x + ITEM_TAG_GAP, top), tag);
    }
    if let Some(galley) = value {
        let height = galley.size().y;
        painter.galley(
            Pos2::new(right - galley.size().x, top + (ITEM_MARK - height) * 0.5),
            galley,
            colours.value,
        );
    }
    painter.galley(
        Pos2::new(title_left, top + ITEM_MARK + ITEM_LINE_GAP),
        note.clone(),
        colours.note,
    );
}

/// Paint the small outlined word after the title of an item.
fn paint_tag(painter: &egui::Painter, at: Pos2, tag: &str) {
    let galley = text::line_weight(
        painter,
        tag,
        &FontId::monospace(ITEM_TAG_FONT),
        theme::ACCENT(),
        f32::INFINITY,
        Weight::Medium,
    );
    let rect = Rect::from_min_size(
        Pos2::new(at.x, at.y + (ITEM_MARK - galley.size().y) * 0.5 - ITEM_TAG_GAP * 0.5),
        Vec2::new(galley.size().x + 2.0 * ITEM_TAG_PAD, galley.size().y + ITEM_TAG_GAP),
    );
    let radius = CornerRadius::same(RADIUS_TAG);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, theme::alpha(theme::ACCENT(), 115)),
        StrokeKind::Inside,
    );
    painter.galley(
        Pos2::new(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        theme::ACCENT(),
    );
}

/// Paint the mark of one state: never a ring that a person could read as a
/// choice of theirs.
pub(super) fn paint_mark(painter: &egui::Painter, centre: Pos2, state: BoardState) {
    let radius = ITEM_MARK * 0.5;
    let ring = ITEM_MARK_RING;
    match state {
        BoardState::Done => {
            painter.circle_filled(centre, radius, theme::PALETTE_GREEN());
            ink(painter, centre, radius, true);
        }
        BoardState::Failed => {
            painter.circle_filled(centre, radius, theme::PALETTE_RED());
            ink(painter, centre, radius, false);
        }
        BoardState::Now | BoardState::NeedsYou => {
            let colour = if state == BoardState::Now {
                theme::ACCENT()
            } else {
                theme::PALETTE_YELLOW()
            };
            painter.circle_filled(centre, radius + ITEM_HALO, theme::alpha(colour, ITEM_HALO_ALPHA));
            painter.circle_stroke(centre, radius - ring * 0.5, Stroke::new(ring, colour));
            painter.circle_filled(centre, radius * 0.55, colour);
        }
        BoardState::Waiting => {
            painter.circle_stroke(centre, radius - ring * 0.5, Stroke::new(ring, theme::FG_DIM()));
        }
    }
}

/// The dark check of a finished item, or the dark cross of a failed one.
fn ink(painter: &egui::Painter, centre: Pos2, radius: f32, check: bool) {
    let stroke = Stroke::new(MARK_STROKE, theme::BG());
    let (a, b, c) = if check {
        (
            Pos2::new(centre.x - radius * 0.45, centre.y),
            Pos2::new(centre.x - radius * 0.1, centre.y + radius * 0.35),
            Pos2::new(centre.x + radius * 0.5, centre.y - radius * 0.35),
        )
    } else {
        (
            Pos2::new(centre.x - radius * 0.4, centre.y - radius * 0.4),
            Pos2::new(centre.x + radius * 0.4, centre.y + radius * 0.4),
            Pos2::new(centre.x - radius * 0.4, centre.y + radius * 0.4),
        )
    };
    painter.line_segment([a, b], stroke);
    painter.line_segment([b, c], stroke);
    if !check {
        painter.line_segment([a, c], stroke);
    }
}

/// Registers one item, so a reader hears its title and a resting pointer shows
/// the whole text of a text that the edge of the card cut.
pub(super) fn chip_node(ui: &Ui, index: usize, at: usize, rect: Rect, chip: &Chip<'_>) {
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("block_board_item", index, at)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, chip.title));
    if response.hovered() {
        response.on_hover_text(chip.whole());
    }
}

/// The room that the evidence of one item takes: the width of the item without
/// its padding, its mark and the gap to its title.
pub(super) fn note_room(rect: Rect) -> f32 {
    (rect.width() - 2.0 * ITEM_BORDER - 2.0 * ITEM_PAD_SIDE - ITEM_MARK - ITEM_MARK_GAP).max(0.0)
}
