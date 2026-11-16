//! The footer of a card: who drew it and when it last changed.
//!
//! Both texts belong to Horizon. The agent cannot set them, so a card can
//! never claim to be older, newer or drawn by someone else. The clock time is
//! formatted when the document changes and cached with the card, so a frame
//! only lays the text out.
//!
//! The time keeps the right edge of the card, and the line that names the owner
//! ends with an ellipsis in the room that the time leaves, so a long session name
//! cannot be painted under the time.

use std::borrow::Cow;
use std::sync::Arc;

use egui::{FontId, Galley, Pos2, Rect, Role, Sense, Ui, WidgetInfo};

use crate::theme;

use super::CardView;
use super::metrics::{FOOTER_FONT, FOOTER_PAD_SIDE};
use super::text;

/// What the footer says when the card has an owner.
const OWNER_PREFIX: &str = "Card drawn by the agent";
/// What the footer says when the owner's panel is gone.
const NO_OWNER: &str = "Card drawn by an agent";
/// What the footer says for a card that Horizon drew itself.
pub(super) const HOST_DRAWN: &str = "Notice of Horizon, not of an agent";
/// What the footer says before the machine and the session of such an owner.
const ON_MACHINE: &str = "Card drawn by the agent on";
/// The gap between the line that names the owner and the time.
const TIME_GAP: f32 = 8.0;

/// The left half of the footer for an owner, as a borrowed slice.
pub(super) fn drawn_by(owner_name: Option<&str>) -> (&'static str, Option<&str>) {
    match owner_name {
        Some(name) => (OWNER_PREFIX, Some(name)),
        None => (NO_OWNER, None),
    }
}

/// The whole left text of the footer: who drew the card, and where its owner runs
/// when that owner is a session of a machine.
fn foot_line<'a>(view: &'a CardView<'a>) -> Cow<'a, str> {
    if let Some((machine, session)) = view.owner_session {
        return format!("{ON_MACHINE} {machine} \u{b7} {session}").into();
    }
    if view.host_owned {
        return Cow::Borrowed(HOST_DRAWN);
    }
    match drawn_by(view.owner_name) {
        (prefix, None) => Cow::Borrowed(prefix),
        (prefix, Some(name)) => format!("{prefix} {name}").into(),
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
        Pos2::new(rect.min.x + FOOTER_PAD_SIDE, rect.min.y),
        Pos2::new(rect.max.x - FOOTER_PAD_SIDE, rect.max.y),
    );
    let font = FontId::proportional(FOOTER_FONT);
    let colour = theme::FG_SOFT();
    let updated = view
        .card
        .updated_label(view.revision, view.updated_at_millis)
        .to_owned();
    let clock = text::line(ui.painter(), &updated, &font, colour, inner.width());
    let words = foot_line(view);
    let room = (inner.width() - clock.size().x - TIME_GAP).max(0.0);
    let owner = text::line(ui.painter(), &words, &font, colour, room);
    let top = rect.center().y - owner.size().y * 0.5;
    let owner_rect = Rect::from_min_size(Pos2::new(inner.min.x, top), owner.size());
    let clock_rect = Rect::from_min_size(Pos2::new(inner.max.x - clock.size().x, top), clock.size());
    ui.painter().galley(owner_rect.min, owner.clone(), colour);
    ui.painter().galley(clock_rect.min, clock.clone(), colour);
    spoken(ui, "owner", owner_rect, &owner, &words);
    spoken(ui, "time", clock_rect, &clock, &updated);
}

/// Registers one painted footer text as the node that a reader hears, with its
/// whole words while the pointer rests on a line that was cut.
fn spoken(ui: &Ui, salt: &str, line: Rect, galley: &Arc<Galley>, words: &str) {
    let response = ui.interact(line, ui.make_persistent_id(("block_footer", salt)), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, words));
    if galley.elided {
        response.on_hover_text(words);
    }
}
