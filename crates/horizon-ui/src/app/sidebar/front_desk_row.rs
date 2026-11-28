//! The row `Front desk` at the top of the sidebar: the conversation of the dock
//! that belongs to no workspace.
//!
//! The row stands there only while the orchestrator dock is visible, and a press
//! on it shows the front desk in the dock, which is what the back control of the
//! header of the dock does. While the dock shows the front desk, the row is the
//! selected one and no work item is marked as active.
//!
//! The state word at the right edge is the word of the chip of the front desk in
//! the header of the dock. A conversation that started no program says nothing
//! here, because `not connected` is no news in the sidebar.

use egui::{Rect, Sense, Stroke, StrokeKind, Vec2, WidgetInfo, pos2, vec2};

use crate::theme;

use super::super::HorizonApp;
use super::super::orchestrator_dock::{FRONT_DESK, OrchestratorDock};
use super::text;

/// The air above the row.
pub(super) const TOP_GAP: f32 = 12.0;
/// The height of the row, its inset in the sidebar, its radius and its padding.
const HEIGHT: f32 = 38.0;
const INSET: f32 = 8.0;
const RADIUS: u8 = 11;
const PADDING: f32 = 10.0;
/// The side of the glyph and the gap between it and the label.
const GLYPH_SIDE: f32 = 16.0;
const GLYPH_GAP: f32 = 10.0;
/// The label of the row, which is also its accessible name.
pub(in crate::app) const LABEL: &str = "Front desk";
/// The word of a conversation whose program runs and waits for the person.
const WORKING: &str = "working";
/// The word of a conversation whose program runs.
const READY: &str = "ready";
/// The stroke of the glyph, the width of the screen of the desk, its height, its
/// radius and its place under the top of the glyph.
const STROKE: f32 = 1.5;
const SCREEN_WIDTH: f32 = 12.0;
const SCREEN_HEIGHT: f32 = 8.0;
const SCREEN_RADIUS: f32 = 2.5;
const SCREEN_TOP: f32 = 3.0;
/// The length of the stem under the screen and the width of its foot.
const STEM: f32 = 3.0;
const FOOT: f32 = 6.0;

impl HorizonApp {
    /// Draws the row `Front desk`, while the orchestrator dock is visible.
    pub(in crate::app) fn render_front_desk_row(&mut self, ui: &mut egui::Ui) {
        if !self.orchestrator_dock.visible() {
            return;
        }
        let selected = self.orchestrator_dock.front_desk().shows(&self.board);
        let word = front_desk_word(&self.orchestrator_dock);
        if render(ui, selected, word) {
            // A press is the call of the back control of the header.
            self.orchestrator_dock.front_desk_mut().show();
        }
    }
}

/// The state word of the front desk, as the chip of its header reads it, or
/// nothing while the conversation started no program.
fn front_desk_word(dock: &OrchestratorDock) -> Option<&'static str> {
    let turn = dock.turn_of(FRONT_DESK)?;
    Some(if turn.busy { WORKING } else { READY })
}

/// Draws the row in `selected` or in its resting form and answers whether the
/// person pressed it.
fn render(ui: &mut egui::Ui, selected: bool, word: Option<&str>) -> bool {
    ui.add_space(TOP_GAP);
    let row = ui.allocate_space(Vec2::new(ui.available_width(), HEIGHT)).1;
    let card = row.shrink2(vec2(INSET, 0.0));
    let response = ui.interact(card, ui.make_persistent_id("sidebar_front_desk"), Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter();
    if selected {
        painter.rect_filled(card, RADIUS, theme::alpha(theme::ACCENT(), 28));
        painter.rect_stroke(
            card,
            RADIUS,
            Stroke::new(1.0_f32, theme::alpha(theme::ACCENT(), 102)),
            StrokeKind::Inside,
        );
    } else if hovered {
        painter.rect_filled(card, RADIUS, theme::PANEL_BG_ALT());
    }

    let glyph = Rect::from_min_size(
        pos2(card.min.x + PADDING, card.center().y - GLYPH_SIDE / 2.0),
        Vec2::splat(GLYPH_SIDE),
    );
    paint_glyph(painter, glyph);
    let label_left = glyph.max.x + GLYPH_GAP;
    let word = word.map(|word| text::mono(ui, word, 10.0, theme::FG_DIM()));
    let label_right = word.as_ref().map_or(card.max.x - PADDING, |word| {
        card.max.x - PADDING - word.size().x - GLYPH_GAP
    });
    let color = if selected { theme::FG() } else { theme::FG_SOFT() };
    let label = text::line(
        ui,
        LABEL,
        13.0,
        text::SEMIBOLD,
        color,
        (label_right - label_left).max(1.0),
    );
    let label_height = label.size().y;
    ui.painter()
        .galley(pos2(label_left, card.center().y - label_height / 2.0), label, color);
    if let Some(word) = word {
        ui.painter().galley(
            pos2(
                card.max.x - PADDING - word.size().x,
                card.center().y - word.size().y / 2.0,
            ),
            word,
            theme::FG_DIM(),
        );
    }

    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, LABEL));
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.on_hover_text(LABEL).clicked()
}

/// Paints the glyph of the desk in `glyph`: the screen, the stem under it and its
/// foot, in the accent colour.
fn paint_glyph(painter: &egui::Painter, glyph: Rect) {
    let stroke = Stroke::new(STROKE, theme::ACCENT());
    let center = glyph.center().x;
    let screen = Rect::from_min_size(
        pos2(center - SCREEN_WIDTH / 2.0, glyph.min.y + SCREEN_TOP),
        vec2(SCREEN_WIDTH, SCREEN_HEIGHT),
    );
    painter.rect_stroke(screen, SCREEN_RADIUS, stroke, StrokeKind::Inside);
    let top = screen.max.y;
    painter.line_segment([pos2(center, top), pos2(center, top + STEM)], stroke);
    let foot = top + STEM;
    painter.line_segment(
        [pos2(center - FOOT / 2.0, foot), pos2(center + FOOT / 2.0, foot)],
        stroke,
    );
}

/// The state word of the front desk of `app`, which a test reads.
#[cfg(test)]
pub(in crate::app) fn state_word_for_test(app: &HorizonApp) -> Option<&'static str> {
    front_desk_word(&app.orchestrator_dock)
}

/// The rectangles of the marks of the row, which a test reads.
#[cfg(test)]
pub(super) struct Parts {
    /// The card of the row.
    pub(super) card: Rect,
    /// The glyph at the left of the card.
    pub(super) glyph: Rect,
    /// The left edge of the label.
    pub(super) label_left: f32,
}

/// The rectangles of the marks of a row of `height` at `top` in a sidebar
/// `width` wide.
#[cfg(test)]
pub(super) fn parts(width: f32, top: f32) -> Parts {
    let card = Rect::from_min_size(pos2(INSET, top + TOP_GAP), vec2(width - 2.0 * INSET, HEIGHT));
    let glyph = Rect::from_min_size(
        pos2(card.min.x + PADDING, card.center().y - GLYPH_SIDE / 2.0),
        Vec2::splat(GLYPH_SIDE),
    );
    Parts {
        card,
        glyph,
        label_left: glyph.max.x + GLYPH_GAP,
    }
}

#[cfg(test)]
mod tests {
    use super::{LABEL, READY, TOP_GAP, WORKING, front_desk_word, parts};

    #[test]
    fn the_row_keeps_the_geometry_of_the_design() {
        // A sidebar 214 wide: the row is 8 in at each side, 38 high, and its
        // glyph stands 10 in from the left of the card.
        let parts = parts(214.0, 0.0);
        assert!((parts.card.min.x - 8.0).abs() < f32::EPSILON);
        assert!((parts.card.max.x - 206.0).abs() < f32::EPSILON);
        assert!((parts.card.height() - 38.0).abs() < f32::EPSILON);
        assert!((parts.card.min.y - TOP_GAP).abs() < f32::EPSILON);
        assert!((parts.glyph.min.x - 18.0).abs() < f32::EPSILON);
        assert!((parts.glyph.width() - 16.0).abs() < f32::EPSILON);
        assert!((parts.label_left - 44.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_words_of_the_row_are_the_words_of_the_chip() {
        assert_eq!(LABEL, "Front desk");
        assert_eq!(WORKING, "working");
        assert_eq!(READY, "ready");
        let (_temp, app) = crate::app::test_support::test_app();
        assert_eq!(
            front_desk_word(&app.orchestrator_dock),
            None,
            "a conversation with no program says nothing"
        );
    }
}
