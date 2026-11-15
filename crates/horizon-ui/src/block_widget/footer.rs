//! The footer of a card: who drew it and when it last changed.
//!
//! Both texts belong to Horizon. The agent cannot set them, so a card can
//! never claim to be older, newer or drawn by someone else. The clock time is
//! formatted when the document changes and cached with the card, so a frame
//! only lays the text out.

use egui::{Align, Layout, Rect, RichText, Ui, UiBuilder};

use crate::theme;

use super::CardView;
use super::metrics::{FOOTER_FONT, FOOTER_PAD_SIDE};

/// What the footer says when the card has an owner.
const OWNER_PREFIX: &str = "Card drawn by the agent";
/// What the footer says when the owner's panel is gone.
const NO_OWNER: &str = "Card drawn by an agent";
/// What the footer says for a card that Horizon drew itself.
pub(super) const HOST_DRAWN: &str = "Notice of Horizon, not of an agent";
/// What the footer says before the machine and the session of such an owner.
const ON_MACHINE: &str = "Card drawn by the agent on";

/// The left half of the footer for an owner, as a borrowed slice.
pub(super) fn drawn_by(owner_name: Option<&str>) -> (&'static str, Option<&str>) {
    match owner_name {
        Some(name) => (OWNER_PREFIX, Some(name)),
        None => (NO_OWNER, None),
    }
}

/// Draw the footer strip inside `rect`, below the body.
pub(super) fn show(ui: &mut Ui, rect: Rect, view: &mut CardView<'_>) {
    if rect.height() < 1.0 {
        return;
    }
    ui.painter().hline(
        rect.x_range(),
        rect.min.y,
        egui::Stroke::new(1.0, theme::BORDER_SUBTLE()),
    );
    let inner = Rect::from_min_max(
        egui::Pos2::new(rect.min.x + FOOTER_PAD_SIDE, rect.min.y),
        egui::Pos2::new(rect.max.x - FOOTER_PAD_SIDE, rect.max.y),
    );
    let updated = view.card.updated_label(view.revision, view.updated_at_millis);
    let owner_name = view.owner_name;
    let owner_session = view.owner_session;
    let host_owned = view.host_owned;

    let mut footer = ui.new_child(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::left_to_right(Align::Center)),
    );
    footer.spacing_mut().item_spacing.x = 4.0;
    if let Some((machine, session)) = owner_session {
        for part in [ON_MACHINE, machine, "\u{b7}", session] {
            footer.label(RichText::new(part).size(FOOTER_FONT).color(theme::FG_SOFT()));
        }
    } else if host_owned {
        footer.label(RichText::new(HOST_DRAWN).size(FOOTER_FONT).color(theme::FG_SOFT()));
    } else {
        let (prefix, name) = drawn_by(owner_name);
        footer.label(RichText::new(prefix).size(FOOTER_FONT).color(theme::FG_SOFT()));
        if let Some(name) = name {
            footer.label(RichText::new(name).size(FOOTER_FONT).color(theme::FG_SOFT()));
        }
    }
    footer.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(RichText::new(updated).size(FOOTER_FONT).color(theme::FG_SOFT()));
    });
}
