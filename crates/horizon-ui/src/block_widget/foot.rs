//! The foot of a card that scrolls: the buttons that end its document keep their
//! place above the footer, and the rest of the body scrolls above them. A card
//! that shows its whole content has no foot and is drawn as its document says.

use std::ops::Range;

use egui::{Rect, Stroke, Ui, UiBuilder, Vec2};
use horizon_browser_control::block::{BlockDocument, Component};

use super::form::Form;
use super::metrics::{BODY_PAD_SIDE, COMPONENT_GAP, count};
use super::{CardView, buttons, primary_button, show_component};
use crate::theme;

/// The buttons that a card keeps at its foot this frame.
pub(super) struct Foot {
    /// The components of the document that stand in the foot.
    pub(super) components: Range<usize>,
    /// The room of the foot, which is the room that its components take in a
    /// body: the content of a card measures the same with a foot and without.
    pub(super) height: f32,
}

/// Whether the content that the last pass measured needs more than `room`.
pub(super) fn scrolls(view: &CardView<'_>, room: f32) -> bool {
    let measured = view.card.measured_content_height();
    measured.is_some_and(|content| content > room + 0.5)
}

/// The `buttons` components that end `document`: only `text` may follow them,
/// and another component stands before them, so a body is left to scroll.
pub(super) fn last_buttons(document: &BlockDocument) -> Option<Range<usize>> {
    let buttons = |part: &Component| matches!(part, Component::Buttons { items } if !items.is_empty());
    let text = |part: &Component| matches!(part, Component::Text { .. });
    let end = document.components.iter().rposition(|part| !text(part))? + 1;
    let first = document.components[..end].iter().rposition(|part| !buttons(part))? + 1;
    (first < end).then_some(first..end)
}

/// The foot of the card of `view`, whose body has the room `content`. A card
/// that shows its whole content has none, and neither has a card that is so low
/// that the foot would take more than half of its height `card_height`.
pub(super) fn plan(ui: &Ui, view: &CardView<'_>, content: Rect, card_height: f32) -> Option<Foot> {
    if !scrolls(view, content.height()) {
        return None;
    }
    let components = last_buttons(view.document)?;
    let width = content.width() - 2.0 * f32::from(BODY_PAD_SIDE);
    let (primary, answers) = (primary_button(view.document), Form::of(width).answers());
    // A body puts this gap above every component. The foot puts half of it
    // above its buttons and half of it below them.
    let mut height = count(components.len()) * (COMPONENT_GAP + ui.spacing().item_spacing.y);
    for index in components.clone() {
        if let Component::Buttons { items } = &view.document.components[index] {
            let (_, lines) = buttons::rows(ui.painter(), index, items, primary, width, answers);
            height += buttons::height(lines.len());
        }
    }
    (height <= card_height * 0.5).then_some(Foot { components, height })
}

/// Draws the foot in `rect`: a thin line, and under it the components of the
/// foot as the body draws them.
pub(super) fn show(ui: &mut Ui, rect: Rect, foot: &Foot, view: &mut CardView<'_>, enabled: bool) {
    let line = Stroke::new(1.0, theme::BORDER_SUBTLE());
    ui.painter().hline(rect.x_range(), rect.min.y, line);
    let pad = f32::midpoint(COMPONENT_GAP, ui.spacing().item_spacing.y);
    let inner = rect.shrink2(Vec2::new(f32::from(BODY_PAD_SIDE), pad));
    let (document, primary, form) = (view.document, primary_button(view.document), Form::of(inner.width()));
    ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
        for index in foot.components.clone() {
            if index > foot.components.start {
                ui.add_space(COMPONENT_GAP);
            }
            show_component(ui, index, &document.components[index], view, enabled, primary, form);
        }
    });
}
