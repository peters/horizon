//! A link of a card: the agent's label as a link-style control, and the host of
//! the page under it.
//!
//! The label is the agent's text and may name another site than the one the
//! address reaches, so the card also draws the host of the address. The host is
//! read once per document and kept in the card's runtime state, so a frame that
//! changes nothing parses no URL again.
//!
//! A link needs no agent: it stays live while the card says that its agent is
//! gone, because Horizon opens the page itself. A press records no click: the
//! person who presses a link asks for a browser panel, not for an answer of the
//! agent.

use crate::theme;
use egui::{CursorIcon, FontId, Pos2, Rect, Role, Sense, Stroke, Ui, Vec2, WidgetInfo};

use super::CardView;
use super::metrics::{LINK_FONT, LINK_HOST_FONT, LINK_HOST_GAP, LINK_UNDERLINE};
use super::text;

/// What the pointer says while it is on a link.
pub(crate) const OPENS_IN_BROWSER_PANEL: &str = "Opens in a browser panel";

/// Height of one line of text, as a multiple of its font size.
const LINE_FACTOR: f32 = 1.4;

/// Draw the `link` of component `component` and, when the person presses it,
/// ask the host for the browser panel of its address.
pub(super) fn show(ui: &mut Ui, component: usize, label: &str, url: &str, view: &mut CardView<'_>, live: bool) {
    let color = if live { theme::ACCENT() } else { theme::FG_DIM() };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height()), Sense::hover());

    let label_line = text::line(
        ui.painter(),
        label,
        &FontId::proportional(LINK_FONT),
        color,
        rect.width(),
    );
    let label_rect = Rect::from_min_size(rect.min, label_line.size());
    let response = ui.interact(
        label_rect,
        ui.make_persistent_id(("block_link", component)),
        if live { Sense::click() } else { Sense::hover() },
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Link, live, label));
    let hovered = live && response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let painter = ui.painter();
    painter.galley(label_rect.min, label_line, color);
    painter.hline(
        label_rect.x_range(),
        label_rect.max.y - LINK_UNDERLINE,
        Stroke::new(LINK_UNDERLINE, color),
    );
    draw_host(ui, rect, view.card.link_host(view.revision, component, view.document));

    let response = if hovered {
        response.on_hover_text(hover_hint(url))
    } else {
        response
    };
    if live && response.clicked() {
        view.open(url);
        ui.ctx().request_repaint();
    }
}

/// The address and what a press on the link does, as the pointer says them.
/// Built while the pointer is on the link only.
fn hover_hint(url: &str) -> String {
    format!("{url}\n{OPENS_IN_BROWSER_PANEL}")
}

/// Draw the host of the address under the label, as its own node, so a reader
/// hears where the link goes.
fn draw_host(ui: &Ui, link: Rect, host: Option<&str>) {
    let Some(host) = host else {
        return;
    };
    let top = link.min.y + LINK_FONT * LINE_FACTOR + LINK_HOST_GAP;
    let rect = Rect::from_min_max(Pos2::new(link.min.x, top), Pos2::new(link.max.x, link.max.y));
    let galley = text::line(
        ui.painter(),
        host,
        &FontId::proportional(LINK_HOST_FONT),
        theme::FG_DIM(),
        rect.width(),
    );
    ui.painter().galley(rect.min, galley, theme::FG_DIM());
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("block_link_host", link.min.y.to_bits())),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, host));
}

/// The room one link takes: its label and the host under it.
pub(super) fn height() -> f32 {
    LINK_FONT * LINE_FACTOR + LINK_HOST_GAP + LINK_HOST_FONT * LINE_FACTOR
}
