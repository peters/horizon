//! The buttons of a card, from the left in document order, with every
//! destructive button pushed to the right edge of a row. The wide form pushes
//! every button to the right edge, so a row of answers closes a wide strip. A
//! stack draws every answer from the left edge of its room instead.
//!
//! Only the first `primary` button of a card is drawn as the primary action.
//! That one decision is made for the whole document by [`super::primary_button`]
//! and handed down here, so a second primary button cannot compete with it.

use egui::{CornerRadius, CursorIcon, FontId, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::{Button, Tone};

use crate::theme;
use crate::typeface::Weight;

use super::form::Answers;
use super::metrics::{BUTTON_FONT, BUTTON_GAP, BUTTON_HEIGHT, BUTTON_PAD, BUTTON_PAD_PRIMARY, RADIUS_CONTROL, count};
use super::{CardView, text, tones};

/// Draw every button of a `buttons` component.
///
/// `primary` is the position of the card's first primary button as
/// `(component, item)`, so this component draws its own primary only when it
/// holds that one. `answers` says whether the buttons keep one row or stack one
/// per line.
pub(super) fn show(
    ui: &mut Ui,
    component: usize,
    items: &[Button],
    primary: Option<(usize, usize)>,
    view: &mut CardView<'_>,
    enabled: bool,
    answers: Answers,
) {
    if items.is_empty() {
        return;
    }
    let available = ui.available_width();
    let (tones_and_widths, lines) = rows(ui.painter(), component, items, primary, available, answers);
    let height = height(lines.len());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(available, height), Sense::hover());

    for (line_index, line) in lines.iter().enumerate() {
        let top = rect.min.y + count(line_index) * (BUTTON_HEIGHT + BUTTON_GAP);
        let mut x = rect.min.x;
        let right_start = if line.right.is_empty() {
            rect.max.x
        } else {
            let danger: f32 = line.right.iter().map(|item| tones_and_widths[*item].1).sum::<f32>()
                + count(line.right.len().saturating_sub(1)) * BUTTON_GAP;
            (rect.max.x - danger).max(x + BUTTON_GAP)
        };
        for item in &line.left {
            let width = tones_and_widths[*item].1;
            let button_rect = Rect::from_min_size(Pos2::new(x, top), Vec2::new(width, BUTTON_HEIGHT));
            draw_button(ui, &items[*item], button_rect, tones_and_widths[*item].0, view, enabled);
            x += width + BUTTON_GAP;
        }
        let mut x = right_start;
        for item in &line.right {
            let width = tones_and_widths[*item].1;
            let button_rect = Rect::from_min_size(Pos2::new(x, top), Vec2::new(width, BUTTON_HEIGHT));
            draw_button(ui, &items[*item], button_rect, tones_and_widths[*item].0, view, enabled);
            x += width + BUTTON_GAP;
        }
    }
}

/// The width of every button of a component with whether it is the primary one
/// of the card, and the lines that the buttons take in a room `available` wide.
pub(super) fn rows(
    painter: &egui::Painter,
    component: usize,
    items: &[Button],
    primary: Option<(usize, usize)>,
    available: f32,
    answers: Answers,
) -> (Vec<(bool, f32)>, Vec<ButtonLine>) {
    let width = |(item, button): (usize, &Button)| {
        let is_primary = primary == Some((component, item));
        let width = button_width(painter, &button.label, is_primary).min(available);
        (is_primary, width)
    };
    let tones_and_widths: Vec<(bool, f32)> = items.iter().enumerate().map(width).collect();
    let lines = lines_of(items, &tones_and_widths, available, answers);
    (tones_and_widths, lines)
}

/// The height that `lines` lines of buttons take.
pub(super) fn height(lines: usize) -> f32 {
    count(lines) * BUTTON_HEIGHT + count(lines.saturating_sub(1)) * BUTTON_GAP
}

/// One line of buttons: the ordinary ones from the left, the destructive ones
/// right aligned.
pub(super) struct ButtonLine {
    left: Vec<usize>,
    right: Vec<usize>,
}

/// Split `items` into lines that fit `available`, keeping document order. The
/// ordinary buttons fill the lines from the left; in a row the destructive ones
/// keep the right edge, and move to a right-aligned line of their own when the
/// last line has no room for them. A stack draws every button from the left edge
/// of the room, so every line of it starts at the same edge.
fn lines_of(items: &[Button], widths: &[(bool, f32)], available: f32, answers: Answers) -> Vec<ButtonLine> {
    let keeps_right_edge = |button: &Button| match answers {
        Answers::RowClosedAtTheRight => true,
        Answers::Row => button.tone == Tone::Danger,
        Answers::Stack => false,
    };
    let mut lines = vec![ButtonLine {
        left: Vec::new(),
        right: Vec::new(),
    }];
    let mut used = 0.0;
    for (item, button) in items.iter().enumerate() {
        if keeps_right_edge(button) {
            continue;
        }
        let width = widths[item].1;
        if used > 0.0 && used + BUTTON_GAP + width > available {
            lines.push(ButtonLine {
                left: Vec::new(),
                right: Vec::new(),
            });
            used = width;
        } else {
            used = if used > 0.0 { used + BUTTON_GAP + width } else { width };
        }
        if let Some(line) = lines.last_mut() {
            line.left.push(item);
        }
    }

    let mut danger_lines: Vec<Vec<usize>> = vec![Vec::new()];
    let mut danger_used = 0.0;
    for (item, button) in items.iter().enumerate() {
        if !keeps_right_edge(button) {
            continue;
        }
        let width = widths[item].1;
        if danger_used > 0.0 && danger_used + BUTTON_GAP + width > available {
            danger_lines.push(Vec::new());
            danger_used = width;
        } else {
            danger_used = if danger_used > 0.0 {
                danger_used + BUTTON_GAP + width
            } else {
                width
            };
        }
        if let Some(line) = danger_lines.last_mut() {
            line.push(item);
        }
    }

    let mut danger_lines = danger_lines.into_iter();
    if let Some(first) = danger_lines.next() {
        let width = line_width(&first, widths);
        // A first line that holds no ordinary button keeps the buttons that keep
        // the right edge, so a row of answers of the wide form stays one line.
        let taken = if used > 0.0 { used + BUTTON_GAP } else { 0.0 };
        if lines.last().is_some() && taken + width <= available {
            if let Some(line) = lines.last_mut() {
                line.right = first;
            }
        } else {
            lines.push(ButtonLine {
                left: Vec::new(),
                right: first,
            });
        }
    }
    for line in danger_lines {
        lines.push(ButtonLine {
            left: Vec::new(),
            right: line,
        });
    }
    lines
}

/// The width one line of buttons takes, with the gaps between them.
fn line_width(items: &[usize], widths: &[(bool, f32)]) -> f32 {
    items.iter().map(|item| widths[*item].1).sum::<f32>() + count(items.len().saturating_sub(1)) * BUTTON_GAP
}

/// The tone a button is drawn with. A primary button that is not the card's
/// first one is drawn neutral, so two buttons never both claim the next step.
pub(super) fn drawn_tone(tone: Tone, is_primary: bool) -> Tone {
    if is_primary {
        Tone::Primary
    } else if tone == Tone::Primary {
        Tone::Neutral
    } else {
        tone
    }
}

/// The width that one button needs, from its label and its padding.
fn button_width(painter: &egui::Painter, label: &str, is_primary: bool) -> f32 {
    let pad = if is_primary { BUTTON_PAD_PRIMARY } else { BUTTON_PAD };
    label_width(painter, label, is_primary) + 2.0 * pad
}

/// The width of each button of a component, in draw order, for a strip that
/// places the row of answers itself.
pub(super) fn widths(
    painter: &egui::Painter,
    items: &[Button],
    component: usize,
    primary: Option<(usize, usize)>,
) -> Vec<f32> {
    items
        .iter()
        .enumerate()
        .map(|(item, button)| button_width(painter, &button.label, primary == Some((component, item))))
        .collect()
}

/// The width the label of `text` needs at the button font, in the weight that
/// the button is drawn at.
fn label_width(painter: &egui::Painter, label: &str, is_primary: bool) -> f32 {
    text::line_weight(
        painter,
        label,
        &FontId::proportional(BUTTON_FONT),
        theme::FG(),
        f32::INFINITY,
        button_weight(is_primary),
    )
    .size()
    .x
}

/// The weight of a button label: the primary one of a card is heavier, as the
/// approved card mockups draw it.
fn button_weight(is_primary: bool) -> Weight {
    if is_primary { Weight::Semibold } else { Weight::Regular }
}

/// Draw one button and, when the person presses it, record the click.
fn draw_button(ui: &mut Ui, button: &Button, rect: Rect, is_primary: bool, view: &mut CardView<'_>, enabled: bool) {
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("block_button", &button.id)),
        if enabled { Sense::click() } else { Sense::hover() },
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &button.label));
    let hovered = enabled && response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let colours = tones::button(drawn_tone(button.tone, is_primary), enabled);
    let painter = ui.painter();
    let radius = CornerRadius::same(RADIUS_CONTROL);
    painter.rect_filled(rect, radius, colours.fill);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, colours.outline), StrokeKind::Inside);
    let label = text::line_weight(
        ui.painter(),
        &button.label,
        &FontId::proportional(BUTTON_FONT),
        colours.text,
        (rect.width() - 2.0 * BUTTON_PAD).max(0.0),
        button_weight(is_primary),
    );
    let height = label.size().y;
    let width = label.size().x;
    let cut = label.elided;
    painter.galley(
        Pos2::new(rect.center().x - width * 0.5, rect.center().y - height * 0.5),
        label,
        colours.text,
    );
    if cut {
        response.clone().on_hover_text(&button.label);
    }

    if enabled && response.clicked() {
        if view.host_owned {
            // A card of Horizon's own has no agent to read a click, so the press
            // is the host's command.
            *view.pressed = Some(button.id.clone());
        } else {
            let (document, revision, now_millis) = (view.document, view.revision, view.now_millis);
            if view.card.press(&button.id, revision, document, now_millis) {
                // The host names the press of a card of an orchestrator in the
                // conversation of that workspace, so the person reads there what
                // the program was told.
                let label = button.label.clone();
                view.card.note_press(&label);
            }
        }
        ui.ctx().request_repaint();
    }
}
