//! The body of a card panel: a block document drawn in plain egui.
//!
//! A card shows its components in document order and draws nothing else: no
//! chrome of its own and no repaint loop of its own. A button and a selectable
//! row are live while the agent that owns the card is in the workspace: a press
//! changes the row selection, records a click for that agent and asks for the
//! one repaint that settles it. Without the agent, everything is drawn disabled
//! and the card says so.
//!
//! Horizon owns the parts a document cannot reach: the state chip in the
//! titlebar, the footer with the owner and the time of the last change, and the
//! status frames that explain a dead card or a full click queue.

mod badge;
mod buttons;
pub(crate) mod chip;
mod footer;
mod heading;
mod kv;
mod link;
mod list;
mod metrics;
mod progress;
mod size;
mod status;
mod text;
mod tones;

pub(crate) use chip::CardChip;
/// The frame around the body, which only the tests judge on its own.
#[cfg(test)]
pub(crate) use size::CARD_CHROME_HEIGHT;
pub(crate) use size::{CARD_FOOT_SPAN, CARD_HEIGHT_LIMITS, default_card_size, fitted_card_height};

use egui::{Frame, Margin, Pos2, Rect, RichText, Sense, Stroke, Ui, UiBuilder, Vec2};
use horizon_browser_control::block::{BlockDocument, Component, Tone};
use horizon_core::{CardFrames, CardRuntime, PanelId};

use crate::theme;

use metrics::{BODY_PAD_BOTTOM, BODY_PAD_SIDE, BODY_PAD_TOP, COMPONENT_GAP, TEXT_FONT, TEXT_LINE_HEIGHT};

/// What a card with no components says.
pub(crate) const EMPTY_CARD: &str = "This card is empty.";
/// The strong line of the frame that explains a card whose agent left.
pub(crate) const AGENT_GONE_TITLE: &str = "Its agent is not here";
/// The second line of that frame.
pub(crate) const AGENT_GONE_LINE: &str = "Start an agent in this workspace to use the buttons.";
/// The strong line of the frame that explains a refused click.
pub(crate) const CLICKS_REFUSED_TITLE: &str = "Too many clicks are waiting";
/// The second line of that frame.
pub(crate) const CLICKS_REFUSED_LINE: &str = "The agent has not read the clicks yet. Press again after it has.";

/// One card as the person sees it this frame.
pub(crate) struct CardView<'a> {
    /// The document to draw.
    pub(crate) document: &'a BlockDocument,
    /// The clicks and the row selection of this card.
    pub(crate) card: &'a mut CardRuntime,
    /// The revision the person sees, which a click reports back.
    pub(crate) revision: u64,
    /// An agent panel of the owner is in the card's workspace.
    pub(crate) agent_present: bool,
    /// No host dialog covers the card, so a press counts.
    pub(crate) interactive: bool,
    /// Wall-clock time of this frame, in milliseconds.
    pub(crate) now_millis: i64,
    /// Display name of the agent that owns the card, while it is in the
    /// workspace.
    pub(crate) owner_name: Option<&'a str>,
    /// Wall-clock time of the last create or update, in milliseconds.
    pub(crate) updated_at_millis: i64,
    /// The address of the link the person pressed this frame, while the card
    /// shows a live link. The host reads it after the pass that drew the card,
    /// opens the page in a browser panel and records no click.
    pub(crate) opened_link: &'a mut Option<String>,
}

impl CardView<'_> {
    /// Remembers that the person pressed the link of `url`. One press per frame
    /// is enough: a second link of the same frame would open a second panel for
    /// one gesture.
    pub(crate) fn open(&mut self, url: &str) {
        if self.opened_link.is_none() {
            *self.opened_link = Some(url.to_owned());
        }
    }
}

/// Draw `view` in document order, above the footer of the card.
///
/// The footer keeps the bottom of the card: the body scrolls above it when the
/// content needs more room than the card has, and no part of the body reaches
/// under it. The body records the height its content needed, which the host
/// uses to give the card the height that fits.
pub(crate) fn show(ui: &mut Ui, panel: PanelId, mut view: CardView<'_>) {
    // A list that the person has not selected in takes the row its document
    // selects, whatever brought the document here: create, update or a restart.
    view.card.apply_default_selection(view.document);
    let enabled = view.agent_present && view.interactive;
    let full = ui.max_rect();
    let footer_top = (full.max.y - metrics::FOOTER_HEIGHT).max(full.min.y);
    let content = Rect::from_min_max(full.min, Pos2::new(full.max.x, footer_top));
    let footer_rect = Rect::from_min_max(Pos2::new(full.min.x, footer_top), full.max);

    ui.scope_builder(UiBuilder::new().max_rect(content), |ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(content));
        show_content(ui, panel, &mut view, enabled);
    });
    footer::show(ui, footer_rect, &mut view);
}

fn show_content(ui: &mut Ui, panel: PanelId, view: &mut CardView<'_>, enabled: bool) {
    let output = egui::ScrollArea::vertical()
        .id_salt(("block_card", panel.0))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            Frame::NONE
                .inner_margin(Margin {
                    left: BODY_PAD_SIDE,
                    right: BODY_PAD_SIDE,
                    top: BODY_PAD_TOP,
                    bottom: BODY_PAD_BOTTOM,
                })
                .show(ui, |ui| show_body(ui, view, enabled));
        });
    // The content size is what the body really needed, independent of the
    // room the card gave it: the host fits the card to it once per document.
    view.card.observe_content_height(output.content_size.y);
}

fn show_body(ui: &mut Ui, view: &mut CardView<'_>, enabled: bool) {
    let refused = view.card.refused_notice(view.now_millis);
    // The frames above the body are Horizon's, not the document's, and they take
    // room too: the host fits the card when this set changes, not only when the
    // document does.
    view.card.observe_frames(CardFrames {
        agent_gone: !view.agent_present,
        clicks_refused: refused,
    });
    let primary = primary_button(view.document);
    let mut first = true;
    if !view.agent_present {
        status::show(ui, "agent_gone", Tone::Warn, AGENT_GONE_TITLE, AGENT_GONE_LINE);
        first = false;
    }
    if refused {
        if !first {
            ui.add_space(COMPONENT_GAP);
        }
        status::show(
            ui,
            "clicks_refused",
            Tone::Info,
            CLICKS_REFUSED_TITLE,
            CLICKS_REFUSED_LINE,
        );
        first = false;
    }
    let document = view.document;
    if document.components.is_empty() {
        if !first {
            ui.add_space(COMPONENT_GAP);
        }
        ui.label(RichText::new(EMPTY_CARD).size(TEXT_FONT).color(theme::FG_SOFT()));
        return;
    }
    for (index, component) in document.components.iter().enumerate() {
        if !first {
            ui.add_space(COMPONENT_GAP);
        }
        first = false;
        show_component(ui, index, component, view, enabled, primary);
    }
}

fn show_component(
    ui: &mut Ui,
    index: usize,
    component: &Component,
    view: &mut CardView<'_>,
    enabled: bool,
    primary: Option<(usize, usize)>,
) {
    match component {
        Component::Heading { text, subtitle, badge } => {
            heading::show(ui, index, text, subtitle.as_deref(), badge.as_ref(), view.agent_present);
        }
        Component::Text { text } => {
            ui.add(
                egui::Label::new(
                    RichText::new(text)
                        .size(TEXT_FONT)
                        .line_height(Some(TEXT_FONT * TEXT_LINE_HEIGHT))
                        .color(theme::FG_SOFT()),
                )
                .wrap(),
            );
        }
        Component::Kv { rows } => kv::show(ui, index, rows),
        Component::Badge { text, tone } => badge::show(ui, index, text, *tone, view.agent_present),
        Component::Progress { label, percent, detail } => {
            progress::show(ui, index, label, *percent, detail.as_deref());
        }
        Component::Divider => show_divider(ui),
        Component::List { selection, rows } => list::show(ui, index, *selection, rows, view, enabled),
        Component::Buttons { items } => buttons::show(ui, index, items, primary, view, enabled),
        // A link opens a browser panel of Horizon itself, so it needs no agent:
        // only a host dialog over the card keeps it from taking a press.
        Component::Link { label, url } => {
            // Only a host dialog over the card keeps a link from taking a press.
            let live = view.interactive;
            link::show(ui, index, label, url, view, live);
        }
    }
}

fn show_divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
    );
}

/// The position of the first `primary` button of a card, as
/// `(component, item)`. Only that one is drawn as the primary action; a later
/// one is drawn neutral so two buttons never both claim to be the next step.
pub(super) fn primary_button(document: &BlockDocument) -> Option<(usize, usize)> {
    document
        .components
        .iter()
        .enumerate()
        .find_map(|(index, component)| match component {
            Component::Buttons { items } => items
                .iter()
                .position(|item| item.tone == Tone::Primary)
                .map(|item| (index, item)),
            _ => None,
        })
}

#[cfg(test)]
pub(crate) mod tests;
