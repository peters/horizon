//! The callout: a strip that asks the person something.
//!
//! The wide card draws one row: the caption and the question at the left, the
//! answers at the right. The narrow card stacks them: caption, question and the
//! answers under one another. A callout has no state of its own; its tint is the
//! tone that the document states, so nothing here claims a Horizon status.

use egui::{CornerRadius, FontId, Pos2, Rect, Stroke, StrokeKind, Ui, UiBuilder, Vec2};
use horizon_browser_control::block::{Button, Tone};

use crate::theme;
use crate::typeface::Weight;

use super::form::Form;
use super::metrics::{
    BUTTON_GAP, BUTTON_HEIGHT, CALLOUT_CAPTION_GAP, CALLOUT_PAD_SIDE, CALLOUT_PAD_Y, CALLOUT_RADIUS, CALLOUT_ROW_GAP,
    CALLOUT_STACK_GAP, CALLOUT_STACK_TEXT_FONT, CALLOUT_TEXT_FONT, CAPTION_FONT, CAPTION_TRACKING, count,
};
use super::{CardView, buttons, text, tones};

/// The parts of a callout, as the document states them.
#[derive(Clone, Copy)]
pub(super) struct Parts<'a> {
    /// The tone of the strip.
    pub(super) tone: Tone,
    /// An optional caption above the question.
    pub(super) caption: Option<&'a str>,
    /// The question or the statement.
    pub(super) text: &'a str,
    /// The answers, in draw order.
    pub(super) buttons: &'a [Button],
}

/// Draw one `callout` component. Its buttons are buttons in every sense, so the
/// first primary button of the card is drawn as the primary one here too.
pub(super) fn show(
    ui: &mut Ui,
    component: usize,
    parts: Parts<'_>,
    primary: Option<(usize, usize)>,
    view: &mut CardView<'_>,
    enabled: bool,
    form: Form,
) {
    let Parts {
        tone,
        caption,
        text: text_body,
        buttons: items,
    } = parts;
    let colours = tones::status(tone);
    let width = ui.available_width();
    let inner = (width - 2.0 * CALLOUT_PAD_SIDE).max(0.0);
    let painter = ui.painter().clone();
    let caption = caption.map(|caption| {
        text::spaced_line(
            &painter,
            caption,
            &FontId::monospace(CAPTION_FONT),
            colours.text,
            inner,
            CAPTION_TRACKING,
            Weight::Semibold,
        )
    });
    let head = caption.as_ref().map_or(0.0, |galley| galley.size().y) + CALLOUT_CAPTION_GAP;
    let widths = buttons::widths(&painter, items, component, primary);
    let row = widths.iter().sum::<f32>() + count(items.len().saturating_sub(1)) * BUTTON_GAP;
    let buttons_width = if form.is_wide() {
        row.min(inner)
    } else {
        widths.iter().copied().fold(0.0, f32::max).min(inner)
    };
    let buttons_height = match items.len() {
        0 => 0.0,
        _ if form.is_wide() => BUTTON_HEIGHT,
        _ => count(items.len()) * BUTTON_HEIGHT + count(items.len() - 1) * BUTTON_GAP,
    };
    let body_room = match form {
        Form::Wide => (inner - buttons_width - CALLOUT_ROW_GAP).max(0.0),
        Form::Narrow => inner,
    };
    let body_font = if form.is_wide() {
        CALLOUT_TEXT_FONT
    } else {
        CALLOUT_STACK_TEXT_FONT
    };
    let body = text::wrapped_weight(
        &painter,
        text_body,
        &FontId::proportional(body_font),
        theme::FG(),
        body_room,
        None,
        Weight::Semibold,
    );
    let stack = head + body.size().y;
    let content = match form {
        Form::Wide => stack.max(buttons_height),
        Form::Narrow => stack + CALLOUT_STACK_GAP + buttons_height,
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, content + 2.0 * CALLOUT_PAD_Y), egui::Sense::hover());
    let radius = CornerRadius::same(CALLOUT_RADIUS);
    painter.rect_filled(rect, radius, colours.fill);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, colours.outline), StrokeKind::Inside);

    let left = rect.min.x + CALLOUT_PAD_SIDE;
    let mut top = rect.min.y + CALLOUT_PAD_Y;
    if let Some(galley) = caption {
        painter.galley(Pos2::new(left, top), galley, colours.text);
    }
    top += head;
    painter.galley(Pos2::new(left, top), body.clone(), theme::FG());
    if items.is_empty() {
        return;
    }
    let top = if form.is_wide() {
        rect.min.y + CALLOUT_PAD_Y + (content - buttons_height) * 0.5
    } else {
        top + body.size().y + CALLOUT_STACK_GAP
    };
    // The wide form closes its row at the right edge; the narrow form puts the
    // answers under the question, at the left edge of the strip.
    let left = if form.is_wide() {
        rect.max.x - CALLOUT_PAD_SIDE - buttons_width
    } else {
        rect.min.x + CALLOUT_PAD_SIDE
    };
    let button_rect = Rect::from_min_size(Pos2::new(left, top), Vec2::new(buttons_width, buttons_height));
    let mut child = ui.new_child(UiBuilder::new().max_rect(button_rect));
    buttons::show(&mut child, component, items, primary, view, enabled, form);
}
