//! The composer of the dock: the field of the draft of the active workspace,
//! the send control and the three chips that the dock reads of this part.
//!
//! The composer hangs on the bottom edge of the dock, so it works out its
//! rectangles from the bottom up before it draws: the line at its top, the field
//! and the send control on one row, and the row of the chips under them. A field
//! that grows takes the room upward and never moves the chips.
//!
//! The field writes the draft of the person. Enter sends it and Shift+Enter
//! makes a new line, so the return key of the text edit is Shift+Enter and this
//! module takes the plain Enter itself, while the field holds the keyboard. A
//! draft of white space only sends nothing.
//!
//! While the conversation of the workspace is busy, the send control becomes the
//! stop control, and Enter sends nothing: the field keeps the keys, and the
//! person reads and writes the draft of a turn that runs.

use std::sync::Arc;

use egui::{
    Align2, Event, FontId, Galley, Id, Key, KeyboardShortcut, Modifiers, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
    UiBuilder, WidgetInfo, pos2, vec2,
};

use crate::theme;
use crate::typeface::Weight;

use super::text;

/// The field of the draft, which a test names to read whether it holds the keys.
pub(super) const FIELD_ID: &str = "orchestrator_dock_draft";
/// The send control, which a test names to read whether it holds the pointer.
const SEND_ID: &str = "orchestrator_dock_send";
/// The line at the top of the composer.
const LINE_HEIGHT: f32 = 1.0;
/// The padding of the composer, which the header keeps too.
const PADDING_X: f32 = 16.0;
const PADDING_TOP: f32 = 12.0;
const PADDING_BOTTOM: f32 = 14.0;
/// The air between the row of the field and the row of the chips.
const ROW_GAP: f32 = 10.0;
const FIELD_TEXT_SIZE: f32 = 13.5;
/// The field opens with one row and grows to these rows.
const FIELD_MIN_ROWS: usize = 1;
const FIELD_MAX_ROWS: f32 = 6.0;
/// The margin of the text in the field: 14 points at each side, 12 above and
/// below it.
const FIELD_MARGIN_X: i8 = 14;
const FIELD_MARGIN_Y: i8 = 12;
const FIELD_MIN_HEIGHT: f32 = 44.0;
const FIELD_RADIUS: u8 = 12;
pub(super) const FIELD_HINT: &str = "Write to the orchestrator";
const SEND_SIDE: f32 = 32.0;
const SEND_GAP: f32 = 8.0;
const SEND_RADIUS: u8 = 9;
const SEND_GLYPH: &str = "\u{2191}";
const STOP_GLYPH: &str = "\u{25a0}";
const CONTROL_GLYPH_SIZE: f32 = 14.0;
const SEND_NAME: &str = "Send";
const STOP_NAME: &str = "Stop";
const SEND_HOVER: &str = "The orchestrator is not connected yet";
const CHIP_RADIUS: u8 = 8;
const CHIP_PADDING_X: f32 = 10.0;
const CHIP_PADDING_Y: f32 = 5.0;
const CHIP_LABEL_SIZE: f32 = 10.5;
const CHIP_VALUE_SIZE: f32 = 12.5;
const CHIP_LINE_GAP: f32 = 2.0;
/// The air between two chips, and between two rows of them.
const CHIP_GAP: f32 = 8.0;
const CHIPS: [(&str, &str); 3] = [
    ("Orchestrator", "Claude Code"),
    ("Workers", "not chosen"),
    ("Place", "this computer"),
];

/// What the person asked for with the field and its control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Asked {
    Nothing,
    /// Send the draft.
    Send,
    /// Stop the turn of the conversation.
    Stop,
}

/// The rectangles of the composer, worked out before it draws: the whole
/// composer, the field, the send control at the right of the field, and the room
/// of the chips under both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Parts {
    pub(super) rect: Rect,
    pub(super) field: Rect,
    pub(super) send: Rect,
    pub(super) chips: Rect,
}

/// The composer of `draft` at the bottom edge of `content`, whose height follows
/// the rows of the draft and the rows of the chips. A dock that is shorter than
/// that gives the composer the room that it has, so no part leaves the dock.
pub(super) fn parts(ui: &Ui, content: Rect, draft: &str) -> Parts {
    let inner_width = (content.width() - 2.0 * PADDING_X).max(0.0);
    let field_width = field_width(inner_width);
    let field_height = field_height(ui, draft, field_width);
    let height = (LINE_HEIGHT + PADDING_TOP + field_height + ROW_GAP + chips_height(ui, inner_width) + PADDING_BOTTOM)
        .min(content.height());
    let rect = Rect::from_min_max(pos2(content.min.x, content.max.y - height), content.max);
    let field_top = rect.min.y + LINE_HEIGHT + PADDING_TOP;
    let field = Rect::from_min_size(pos2(rect.min.x + PADDING_X, field_top), vec2(field_width, field_height));
    let send = Rect::from_min_size(
        pos2(field.max.x + SEND_GAP, field.max.y - SEND_SIDE - send_inset_y()),
        vec2(SEND_SIDE, SEND_SIDE),
    );
    let chips = Rect::from_min_max(
        pos2(rect.min.x + PADDING_X, field.max.y + ROW_GAP),
        pos2(rect.max.x - PADDING_X, rect.max.y - PADDING_BOTTOM),
    );
    Parts {
        rect,
        field,
        send,
        chips,
    }
}

/// Draws the composer that [`parts`] worked out, with its line above it, and
/// answers what the person asked for.
pub(super) fn render(ui: &mut Ui, parts: &Parts, draft: &mut String, enabled: bool, busy: bool) -> Asked {
    ui.painter().hline(
        parts.rect.x_range(),
        parts.rect.min.y,
        Stroke::new(LINE_HEIGHT, theme::BORDER_SUBTLE()),
    );
    let sent = field(ui, parts.field, draft, enabled, enabled && !busy);
    let control = control(ui, parts.send, enabled, busy);
    chips(ui, parts.chips);
    if control {
        Asked::Stop
    } else if sent {
        Asked::Send
    } else {
        Asked::Nothing
    }
}

/// The width of the field: the row of the composer minus the send control and
/// the air before it.
fn field_width(inner_width: f32) -> f32 {
    (inner_width - SEND_GAP - SEND_SIDE).max(1.0)
}

/// The air between the send control and the bottom edge of the field, which keeps
/// the centre of the control on the centre of a field of one row.
fn send_inset_y() -> f32 {
    (FIELD_MIN_HEIGHT - SEND_SIDE) / 2.0
}

/// The height of the field of `draft`: its text and its margin, from one row to
/// [`FIELD_MAX_ROWS`] rows.
fn field_height(ui: &Ui, draft: &str, width: f32) -> f32 {
    let margin = f32::from(FIELD_MARGIN_Y);
    let galley = draft_galley(ui, draft, width);
    let last_row_top = galley.rows.last().map_or(0.0, |row| row.pos.y);
    let row_height = (galley.size().y - last_row_top).max(FIELD_TEXT_SIZE);
    let tallest = (FIELD_MAX_ROWS * row_height + 2.0 * margin).max(FIELD_MIN_HEIGHT);
    (galley.size().y + 2.0 * margin).clamp(FIELD_MIN_HEIGHT, tallest)
}

/// The text of `draft` at the width that the field gives it.
fn draft_galley(ui: &Ui, draft: &str, width: f32) -> Arc<Galley> {
    let text_width = (width - 2.0 * f32::from(FIELD_MARGIN_X)).max(1.0);
    ui.painter().layout(
        draft.to_owned(),
        FontId::proportional(FIELD_TEXT_SIZE),
        theme::FG(),
        text_width,
    )
}

/// The field of the draft: one row that grows to [`FIELD_MAX_ROWS`] rows and no
/// further, so a long draft never takes the room of the dock. The dock paints the
/// frame of the field on the rectangle that it worked out, and the text edit sits
/// inside it, so a draft that is longer than the field is clipped instead of
/// moving the parts under it.
fn field(ui: &mut Ui, rect: Rect, draft: &mut String, enabled: bool, sends: bool) -> bool {
    ui.painter().rect(
        rect,
        FIELD_RADIUS,
        theme::BG(),
        Stroke::new(1.0_f32, theme::BORDER_STRONG()),
        StrokeKind::Inside,
    );
    let text = rect.shrink2(vec2(f32::from(FIELD_MARGIN_X), f32::from(FIELD_MARGIN_Y)));
    let enter = sends && ui.memory(|memory| memory.has_focus(Id::unique(FIELD_ID))) && plain_enter(ui);
    let edit = egui::TextEdit::multiline(&mut *draft)
        .id(Id::unique(FIELD_ID))
        .hint_text(FIELD_HINT)
        .font(FontId::proportional(FIELD_TEXT_SIZE))
        .text_color(theme::FG())
        .desired_width(text.width())
        .desired_rows(FIELD_MIN_ROWS)
        // Shift+Enter makes a new line: the return key of the edit is that one,
        // so the plain Enter reaches the rule of the send.
        .return_key(Some(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter)))
        // The whole field takes the pointer, so a click anywhere in it gives the
        // text edit the keyboard.
        .min_size(text.size())
        .frame(egui::Frame::NONE);
    let mut field_ui = ui.new_child(UiBuilder::new().max_rect(text));
    field_ui.shrink_clip_rect(text);
    if enabled {
        field_ui.add(edit);
    } else {
        field_ui.add_enabled(false, edit);
    }
    enter
}

/// Whether this frame holds a press of Enter alone, without Shift, Alt or the
/// command key. The press stays in the events of the frame, so a widget that
/// reads it still does.
fn plain_enter(ui: &Ui) -> bool {
    ui.input(|input| {
        input.events.iter().any(|event| {
            matches!(
                event,
                Event::Key {
                    key: Key::Enter,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.shift && !modifiers.alt && !modifiers.command
            )
        })
    })
}

/// The control of the composer: the send control, or the stop control while the
/// conversation is busy. It answers whether the person pressed it.
fn control(ui: &Ui, rect: Rect, enabled: bool, busy: bool) -> bool {
    let painter = ui.painter();
    painter.rect_filled(rect, SEND_RADIUS, theme::PANEL_BG_ALT());
    painter.rect_stroke(
        rect,
        SEND_RADIUS,
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        if busy { STOP_GLYPH } else { SEND_GLYPH },
        FontId::proportional(CONTROL_GLYPH_SIZE),
        if enabled { theme::FG() } else { theme::FG_DIM() },
    );
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, Id::unique(SEND_ID), sense);
    let name = if busy { STOP_NAME } else { SEND_NAME };
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, enabled, name));
    if enabled {
        return response.clicked();
    }
    let _ = response.on_hover_text(SEND_HOVER);
    false
}

/// One chip: its rectangle, and the two lines that it shows.
struct Chip {
    rect: Rect,
    label: Arc<Galley>,
    value: Arc<Galley>,
}

/// The three read-only chips of this part, left to right in `rect`: what the
/// orchestrator is, how many workers it has, and where it runs. A dock that is
/// too narrow for three of them puts the next chip on the row under it.
fn chips(ui: &Ui, rect: Rect) {
    for chip in chip_rects(ui, rect) {
        let painter = ui.painter();
        painter.rect_filled(chip.rect, CHIP_RADIUS, theme::PANEL_BG_ALT());
        painter.rect_stroke(
            chip.rect,
            CHIP_RADIUS,
            Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
            StrokeKind::Inside,
        );
        let left = chip.rect.min.x + CHIP_PADDING_X;
        let top = chip.rect.min.y + CHIP_PADDING_Y;
        let label_height = chip.label.size().y;
        painter.galley(pos2(left, top), chip.label, theme::FG_DIM());
        painter.galley(pos2(left, top + label_height + CHIP_LINE_GAP), chip.value, theme::FG());
    }
}

/// The height of the rows of the chips of a composer that is `inner_width`
/// points wide.
fn chips_height(ui: &Ui, inner_width: f32) -> f32 {
    chip_rects(ui, Rect::from_min_size(Pos2::ZERO, vec2(inner_width, 0.0)))
        .last()
        .map_or(0.0, |chip| chip.rect.max.y)
}

/// The rectangle of each chip inside `rect`, in the order of [`CHIPS`].
fn chip_rects(ui: &Ui, rect: Rect) -> Vec<Chip> {
    let max_text = (rect.width() - 2.0 * CHIP_PADDING_X).max(1.0);
    let mut chips = Vec::with_capacity(CHIPS.len());
    let (mut left, mut top) = (rect.min.x, rect.min.y);
    let mut row_height = 0.0_f32;
    for (label, value) in CHIPS {
        let label = text::line(ui, label, CHIP_LABEL_SIZE, Weight::Regular, theme::FG_DIM(), max_text);
        let value = text::line(ui, value, CHIP_VALUE_SIZE, Weight::Regular, theme::FG(), max_text);
        let width = (label.size().x.max(value.size().x) + 2.0 * CHIP_PADDING_X).ceil();
        let height = (label.size().y + CHIP_LINE_GAP + value.size().y + 2.0 * CHIP_PADDING_Y).ceil();
        if left > rect.min.x && left + width > rect.max.x {
            left = rect.min.x;
            top += row_height + CHIP_GAP;
            row_height = 0.0;
        }
        chips.push(Chip {
            rect: Rect::from_min_size(pos2(left, top), vec2(width, height)),
            label,
            value,
        });
        left += width + CHIP_GAP;
        row_height = row_height.max(height);
    }
    chips
}
