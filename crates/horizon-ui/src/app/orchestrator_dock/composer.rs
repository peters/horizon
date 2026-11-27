//! The composer of the dock: the field of the draft of the active workspace,
//! the send control inside it and the three chips that the dock reads of this
//! part.
//!
//! The composer hangs on the bottom edge of the dock, so it works out its
//! rectangles from the bottom up before it draws: the line at its top, the field
//! over the full width of the composer, and the row of the chips under it. A
//! field that grows takes the room upward and never moves the chips.
//!
//! The field writes the draft of the person. Enter sends it and Shift+Enter
//! makes a new line, so the return key of the text edit is Shift+Enter and this
//! module takes the plain Enter itself, while the field holds the keyboard. A
//! draft of white space only sends nothing.
//!
//! The control stands inside the field, at its right end: it is 7 points from
//! the inner edge of the field and, in a field of one row, its centre is the
//! centre of that row. A taller draft keeps it at the bottom right of the field.
//! It shows what a press will do: the accent arrow while a draft can go out, the
//! quiet arrow while it cannot, and the filled square while the conversation is
//! busy. Enter sends nothing while the conversation is busy.
//!
//! The chips are read-only in this part, so each of them carries the chevron of
//! a choice that a later part opens and the hover text that says so.

use std::sync::Arc;

use egui::{
    Align2, CursorIcon, Event, FontId, Galley, Id, Key, KeyboardShortcut, Modifiers, Pos2, Rect, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, WidgetInfo, pos2, vec2,
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
/// The margin of the text in the field: 14 points at its left, 12 above and
/// below it, and the text ends 8 points before the control.
const FIELD_MARGIN_X: i8 = 14;
const FIELD_MARGIN_Y: i8 = 12;
const FIELD_MIN_HEIGHT: f32 = 48.0;
const FIELD_RADIUS: u8 = 12;
pub(super) const FIELD_HINT: &str = "Write to the orchestrator";
/// The control inside the field at its right end: its side, the air between it
/// and the inner edge of the field, and its radius.
const SEND_SIDE: f32 = 34.0;
const SEND_INSET: f32 = 7.0;
const SEND_RADIUS: u8 = 9;
/// The air between the text of the field and the control.
const TEXT_CONTROL_GAP: f32 = 8.0;
const SEND_GLYPH: &str = "\u{2191}";
const STOP_GLYPH: &str = "\u{25a0}";
const CONTROL_GLYPH_SIZE: f32 = 14.0;
const SEND_NAME: &str = "Send";
const STOP_NAME: &str = "Stop";
const SEND_HOVER: &str = "The orchestrator is not connected yet";
/// How much of the foreground the fill of the control takes while the pointer
/// stands on it.
const SEND_HOVER_TINT: f32 = 0.12;
const CHIP_RADIUS: u8 = 8;
const CHIP_PADDING_X: f32 = 10.0;
const CHIP_PADDING_Y: f32 = 5.0;
const CHIP_LABEL_SIZE: f32 = 10.5;
const CHIP_VALUE_SIZE: f32 = 12.5;
const CHIP_LINE_GAP: f32 = 2.0;
/// The chevron of a chip that opens a choice: its width, its height and the air
/// between the value of the chip and it.
const CHIP_CHEVRON: f32 = 5.0;
const CHIP_CHEVRON_HEIGHT: f32 = 3.0;
const CHIP_CHEVRON_GAP: f32 = 6.0;
const CHIP_CHEVRON_STROKE: f32 = 1.0;
const CHIP_HOVER: &str = "Choice comes later";
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
/// composer, the field over the full width of the composer, the send control
/// inside the field at its right end, and the room of the chips under both.
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
    let field_width = inner_width.max(1.0);
    let field_height = field_height(ui, draft, field_width);
    let height = (LINE_HEIGHT + PADDING_TOP + field_height + ROW_GAP + chips_height(ui, inner_width) + PADDING_BOTTOM)
        .min(content.height());
    let rect = Rect::from_min_max(pos2(content.min.x, content.max.y - height), content.max);
    let field_top = rect.min.y + LINE_HEIGHT + PADDING_TOP;
    let field = Rect::from_min_size(pos2(rect.min.x + PADDING_X, field_top), vec2(field_width, field_height));
    let send = Rect::from_min_size(
        pos2(
            field.max.x - SEND_INSET - SEND_SIDE,
            field.max.y - SEND_INSET - SEND_SIDE,
        ),
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
    let can_send = enabled && !busy && !draft.trim().is_empty();
    let sent = field(ui, parts.field, draft, enabled, enabled && !busy);
    let control = control(ui, parts.send, can_send, enabled, busy);
    chips(ui, parts.chips);
    if control {
        Asked::Stop
    } else if sent {
        Asked::Send
    } else {
        Asked::Nothing
    }
}

/// The height of the field of `draft`: its text and its margin, from one row to
/// [`FIELD_MAX_ROWS`] rows, and never under [`FIELD_MIN_HEIGHT`].
fn field_height(ui: &Ui, draft: &str, width: f32) -> f32 {
    let margin = f32::from(FIELD_MARGIN_Y);
    let galley = draft_galley(ui, draft, width);
    let last_row_top = galley.rows.last().map_or(0.0, |row| row.pos.y);
    let row_height = (galley.size().y - last_row_top).max(FIELD_TEXT_SIZE);
    let tallest = (FIELD_MAX_ROWS * row_height + 2.0 * margin).max(FIELD_MIN_HEIGHT);
    (galley.size().y + 2.0 * margin).clamp(FIELD_MIN_HEIGHT, tallest)
}

/// The width of the text of the field: the field minus its left margin, the air
/// before the control, the control and the air after it.
fn text_width(field_width: f32) -> f32 {
    (field_width - f32::from(FIELD_MARGIN_X) - TEXT_CONTROL_GAP - SEND_SIDE - SEND_INSET).max(1.0)
}

/// The text of `draft` at the width that the field gives it.
fn draft_galley(ui: &Ui, draft: &str, width: f32) -> Arc<Galley> {
    ui.painter().layout(
        draft.to_owned(),
        FontId::proportional(FIELD_TEXT_SIZE),
        theme::FG(),
        text_width(width),
    )
}

/// The field of the draft: one row that grows to [`FIELD_MAX_ROWS`] rows and no
/// further, so a long draft never takes the room of the dock. The dock paints the
/// frame of the field on the rectangle that it worked out, and the text edit sits
/// inside it, so a draft that is longer than the field is clipped instead of
/// moving the parts under it. The outline turns to the accent while the field
/// holds the keyboard.
fn field(ui: &mut Ui, rect: Rect, draft: &mut String, enabled: bool, sends: bool) -> bool {
    let focused = ui.memory(|memory| memory.has_focus(Id::unique(FIELD_ID)));
    let outline = if focused {
        theme::ACCENT()
    } else {
        theme::BORDER_STRONG()
    };
    ui.painter().rect(
        rect,
        FIELD_RADIUS,
        theme::BG(),
        Stroke::new(1.0_f32, outline),
        StrokeKind::Inside,
    );
    let text = Rect::from_min_size(
        pos2(
            rect.min.x + f32::from(FIELD_MARGIN_X),
            rect.min.y + f32::from(FIELD_MARGIN_Y),
        ),
        vec2(
            text_width(rect.width()),
            (rect.height() - 2.0 * f32::from(FIELD_MARGIN_Y)).max(1.0),
        ),
    );
    let enter = sends && focused && plain_enter(ui);
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

/// The control of the composer, inside the field at its right end: the accent
/// send control while a draft can go out, the quiet one while it cannot, and the
/// stop control while the conversation is busy. It answers whether the person
/// pressed it.
fn control(ui: &Ui, rect: Rect, can_send: bool, enabled: bool, busy: bool) -> bool {
    let clickable = busy || can_send;
    let sense = if clickable { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, Id::unique(SEND_ID), sense);
    let (fill, outline, glyph, glyph_color) = if busy {
        (
            theme::PANEL_BG_ALT(),
            Some(theme::BORDER_STRONG()),
            STOP_GLYPH,
            theme::FG(),
        )
    } else if can_send {
        let fill = if response.hovered() {
            theme::blend(theme::ACCENT(), theme::FG(), SEND_HOVER_TINT)
        } else {
            theme::ACCENT()
        };
        (fill, None, SEND_GLYPH, theme::BG())
    } else {
        (
            theme::PANEL_BG_ALT(),
            Some(theme::BORDER_SUBTLE()),
            SEND_GLYPH,
            theme::FG_DIM(),
        )
    };
    let painter = ui.painter();
    painter.rect_filled(rect, SEND_RADIUS, fill);
    if let Some(color) = outline {
        painter.rect_stroke(rect, SEND_RADIUS, Stroke::new(1.0_f32, color), StrokeKind::Inside);
    }
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(CONTROL_GLYPH_SIZE),
        glyph_color,
    );
    let name = if busy { STOP_NAME } else { SEND_NAME };
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, clickable, name));
    let response = if enabled {
        if can_send {
            response.on_hover_cursor(CursorIcon::PointingHand)
        } else {
            response
        }
    } else {
        response.on_hover_text(SEND_HOVER)
    };
    if clickable { response.clicked() } else { false }
}

/// One chip: its rectangle, the two lines that it shows, and the place of the
/// chevron of its choice.
struct Chip {
    rect: Rect,
    label: Arc<Galley>,
    value: Arc<Galley>,
    chevron: Pos2,
}

/// The three read-only chips of this part, left to right in `rect`: what the
/// orchestrator is, how many workers it has, and where it runs. A dock that is
/// too narrow for three of them puts the next chip on the row under it.
fn chips(ui: &Ui, rect: Rect) {
    for (index, chip) in chip_rects(ui, rect).into_iter().enumerate() {
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
        chevron(painter, chip.chevron);
        let response = ui.interact(chip.rect, Id::unique(("orchestrator_dock_chip", index)), Sense::hover());
        let _ = response.on_hover_text(CHIP_HOVER);
    }
}

/// The chevron of a chip that opens a choice later: a small mark that points
/// down from `at`, where its top left corner stands.
fn chevron(painter: &egui::Painter, at: Pos2) {
    let stroke = Stroke::new(CHIP_CHEVRON_STROKE, theme::FG_DIM());
    painter.line_segment(
        [at, pos2(at.x + CHIP_CHEVRON / 2.0, at.y + CHIP_CHEVRON_HEIGHT)],
        stroke,
    );
    painter.line_segment(
        [
            pos2(at.x + CHIP_CHEVRON / 2.0, at.y + CHIP_CHEVRON_HEIGHT),
            pos2(at.x + CHIP_CHEVRON, at.y),
        ],
        stroke,
    );
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
        let value_max = (max_text - CHIP_CHEVRON_GAP - CHIP_CHEVRON).max(1.0);
        let value = text::line(ui, value, CHIP_VALUE_SIZE, Weight::Regular, theme::FG(), value_max);
        let widest = label.size().x.max(value.size().x + CHIP_CHEVRON_GAP + CHIP_CHEVRON);
        let width = (widest + 2.0 * CHIP_PADDING_X).ceil();
        let height = (label.size().y + CHIP_LINE_GAP + value.size().y + 2.0 * CHIP_PADDING_Y).ceil();
        if left > rect.min.x && left + width > rect.max.x {
            left = rect.min.x;
            top += row_height + CHIP_GAP;
            row_height = 0.0;
        }
        let chip_rect = Rect::from_min_size(pos2(left, top), vec2(width, height));
        let value_top = top + CHIP_PADDING_Y + label.size().y + CHIP_LINE_GAP;
        let chevron = pos2(
            left + CHIP_PADDING_X + value.size().x + CHIP_CHEVRON_GAP,
            value_top + value.size().y / 2.0 - CHIP_CHEVRON_HEIGHT / 2.0,
        );
        chips.push(Chip {
            rect: chip_rect,
            label,
            value,
            chevron,
        });
        left += width + CHIP_GAP;
        row_height = row_height.max(height);
    }
    chips
}

#[cfg(test)]
mod tests {
    use super::{FIELD_MARGIN_X, SEND_INSET, SEND_SIDE, TEXT_CONTROL_GAP, text_width};

    #[test]
    fn the_text_of_the_field_ends_before_the_control_inside_it() {
        let field_width = 400.0;
        let text_right = f32::from(FIELD_MARGIN_X) + text_width(field_width);
        let control_left = field_width - SEND_INSET - SEND_SIDE;

        assert!(
            (control_left - text_right - TEXT_CONTROL_GAP).abs() < 0.01,
            "the text ends at {text_right} and the control starts at {control_left}"
        );
        assert!(control_left - SEND_INSET > 0.0, "the control stands inside the field");
    }
}
