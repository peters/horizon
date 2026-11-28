//! The footer of the sidebar: the line of the server at the left and the link
//! `Set up` at the right, fixed at the bottom under the scroll area.
//!
//! The footer says in one line whether this Horizon follows a server, and it
//! keeps the way to set one up next to that line. It stands under every group of
//! the sidebar and does not move with them, so a person finds the link at the same
//! place in every window.

use egui::{Rect, Sense, Stroke, Vec2, WidgetInfo, pos2};

use crate::theme;

use super::{SidebarActions, text};

/// The height of the footer: the line that separates it, the padding above and
/// under its text, and the text itself.
pub(super) const HEIGHT: f32 = 37.0;
/// The padding above the text and the inset at each side. The height keeps 12
/// under the text as well.
const PAD_TOP: f32 = 10.0;
const SIDE: f32 = 16.0;
/// The side of the dot in front of the line, the gap after it, and the size of
/// the line and of the link.
const DOT_SIDE: f32 = 7.0;
const DOT_GAP: f32 = 8.0;
const SIZE: f32 = 11.0;
/// The gap that the line keeps before the link.
const LINK_GAP: f32 = 10.0;
/// What the footer says while this Horizon follows no server.
pub(in crate::app) const NO_SERVER: &str = "Server: not set";
/// The label of the link, which is also its accessible name.
pub(in crate::app) const SET_UP: &str = "Set up";
/// The colour of the dot of the line: the dot of the server line of today.
fn dot_color() -> egui::Color32 {
    theme::FG_DIM()
}

/// Draws the footer across a sidebar `width` wide in the `height` of [`HEIGHT`],
/// and the line across it. A press on the link asks for Set up.
pub(super) fn render(ui: &mut egui::Ui, width: f32, words: &str, reason: Option<&str>, actions: &mut SidebarActions) {
    let footer = ui.allocate_space(Vec2::new(width, HEIGHT)).1;
    let painter = ui.painter();
    painter.line_segment(
        [footer.left_top(), footer.right_top()],
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
    );

    let link = text::line(ui, SET_UP, SIZE, text::REGULAR, theme::ACCENT(), 60.0);
    let link_rect = Rect::from_min_size(
        pos2(footer.max.x - SIDE - link.size().x, footer.min.y + PAD_TOP),
        link.size(),
    );
    ui.painter().galley(link_rect.min, link.clone(), theme::ACCENT());
    let dot = pos2(
        footer.min.x + SIDE + DOT_SIDE / 2.0,
        footer.min.y + PAD_TOP + link.size().y / 2.0,
    );
    ui.painter().circle_filled(dot, DOT_SIDE / 2.0, dot_color());
    let text_left = dot.x + DOT_SIDE / 2.0 + DOT_GAP;
    let text_right = link_rect.min.x - LINK_GAP;
    let server = text::line(
        ui,
        words,
        SIZE,
        text::REGULAR,
        theme::FG_DIM(),
        (text_right - text_left).max(1.0),
    );
    let elided = server.elided;
    let server_rect = Rect::from_min_size(pos2(text_left, dot.y - server.size().y / 2.0), server.size());
    ui.painter().galley(server_rect.min, server, theme::FG_DIM());
    // The line that the footer cuts shows its whole text, and the reason of the last
    // attempt at the link stands beside it.
    if elided || reason.is_some() {
        let response = ui.interact(
            server_rect,
            ui.make_persistent_id("sidebar_footer_line"),
            Sense::hover(),
        );
        let response = match reason {
            Some(reason) => response.on_hover_text(reason),
            None => response.on_hover_text(words),
        };
        response.widget_info(|| WidgetInfo::labeled(egui::Role::Label, true, words));
    }

    let response = ui.interact(
        link_rect,
        ui.make_persistent_id("sidebar_footer_set_up"),
        Sense::click(),
    );
    if response.hovered() {
        // The link underlines itself in its hover, and the pointer shows a press.
        ui.painter().line_segment(
            [
                pos2(link_rect.min.x, link_rect.max.y + 1.0),
                pos2(link_rect.max.x, link_rect.max.y + 1.0),
            ],
            Stroke::new(1.0_f32, theme::ACCENT()),
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, SET_UP));
    if response.on_hover_text(SET_UP).clicked() {
        actions.press_set_up = true;
    }
}

/// The place of the line and of the link of the footer of a sidebar `width` wide
/// at `top`, which a test reads.
#[cfg(test)]
pub(super) fn parts(width: f32, top: f32) -> (Rect, egui::Pos2, f32) {
    let footer = Rect::from_min_size(pos2(0.0, top), Vec2::new(width, HEIGHT));
    let dot = pos2(footer.min.x + SIDE + DOT_SIDE / 2.0, footer.min.y + PAD_TOP + 7.0);
    (footer, dot, footer.max.x - SIDE)
}

#[cfg(test)]
mod tests {
    use super::{HEIGHT, NO_SERVER, SET_UP, parts};

    #[test]
    fn the_footer_keeps_the_geometry_of_the_design() {
        // The footer of a sidebar 214 wide, under a scroll area that ends at 600.
        let (footer, dot, link_right) = parts(214.0, 600.0);
        assert!((footer.min.y - 600.0).abs() < f32::EPSILON);
        assert!((footer.height() - HEIGHT).abs() < f32::EPSILON);
        assert!((footer.width() - 214.0).abs() < f32::EPSILON);
        assert!((dot.x - (16.0 + 3.5)).abs() < f32::EPSILON, "{dot:?}");
        assert!((dot.y - (610.0 + 7.0)).abs() < f32::EPSILON, "{dot:?}");
        assert!((link_right - (214.0 - 16.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn the_footer_says_what_the_server_line_says() {
        assert_eq!(NO_SERVER, "Server: not set");
        assert_eq!(SET_UP, "Set up");
    }
}
