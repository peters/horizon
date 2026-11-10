//! The buttons of a card, from the left in document order, with every
//! destructive button pushed to the right edge.
//!
//! Only the first `primary` button of a card is drawn as the primary action.
//! That one decision is made for the whole document by [`super::primary_button`]
//! and handed down here, so a second primary button cannot compete with it.

use egui::{CornerRadius, CursorIcon, FontId, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::{Button, Tone};

use crate::theme;

use super::metrics::{BUTTON_FONT, BUTTON_GAP, BUTTON_HEIGHT, BUTTON_PAD, BUTTON_PAD_PRIMARY, BUTTON_RADIUS, count};
use super::{CardView, text, tones};

/// Draw every button of a `buttons` component.
///
/// `primary` is the position of the card's first primary button as
/// `(component, item)`, so this component draws its own primary only when it
/// holds that one.
pub(super) fn show(
    ui: &mut Ui,
    component: usize,
    items: &[Button],
    primary: Option<(usize, usize)>,
    view: &mut CardView<'_>,
    enabled: bool,
) {
    if items.is_empty() {
        return;
    }
    let available = ui.available_width();
    let tones_and_widths: Vec<(bool, f32)> = items
        .iter()
        .enumerate()
        .map(|(item, button)| {
            let is_primary = primary == Some((component, item));
            let pad = if is_primary { BUTTON_PAD_PRIMARY } else { BUTTON_PAD };
            (is_primary, (label_width(ui, &button.label) + 2.0 * pad).min(available))
        })
        .collect();

    let lines = lines_of(items, &tones_and_widths, available);
    let height = count(lines.len()) * BUTTON_HEIGHT + count(lines.len().saturating_sub(1)) * BUTTON_GAP;
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

/// One line of buttons: the ordinary ones from the left, the destructive ones
/// right aligned.
struct ButtonLine {
    left: Vec<usize>,
    right: Vec<usize>,
}

/// Split `items` into lines that fit `available`, keeping document order. The
/// ordinary buttons fill the lines from the left; the destructive ones keep the
/// right edge, and move to a right-aligned line of their own when the last line
/// has no room for them.
fn lines_of(items: &[Button], widths: &[(bool, f32)], available: f32) -> Vec<ButtonLine> {
    let mut lines = vec![ButtonLine {
        left: Vec::new(),
        right: Vec::new(),
    }];
    let mut used = 0.0;
    for (item, button) in items.iter().enumerate() {
        if button.tone == Tone::Danger {
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
        if button.tone != Tone::Danger {
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
        if lines.last().is_some() && used + BUTTON_GAP + width <= available {
            if let Some(line) = lines.last_mut() {
                line.right.extend(first);
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

/// The width the label of `text` needs at the button font.
fn label_width(ui: &Ui, label: &str) -> f32 {
    text::line(
        ui.painter(),
        label,
        &FontId::proportional(BUTTON_FONT),
        theme::FG(),
        f32::INFINITY,
    )
    .size()
    .x
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
    let radius = CornerRadius::same(BUTTON_RADIUS);
    painter.rect_filled(rect, radius, colours.fill);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, colours.outline), StrokeKind::Inside);
    let label = text::line(
        ui.painter(),
        &button.label,
        &FontId::proportional(BUTTON_FONT),
        colours.text,
        (rect.width() - 2.0 * BUTTON_PAD).max(0.0),
    );
    let height = label.size().y;
    let width = label.size().x;
    painter.galley(
        Pos2::new(rect.center().x - width * 0.5, rect.center().y - height * 0.5),
        label,
        colours.text,
    );

    if enabled && response.clicked() {
        let (document, revision, now_millis) = (view.document, view.revision, view.now_millis);
        view.card.press(&button.id, revision, document, now_millis);
        ui.ctx().request_repaint();
    }
}
