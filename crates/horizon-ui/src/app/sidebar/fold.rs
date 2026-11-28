//! The fold of the panel rows of a work item, and the control that opens and
//! closes it.
//!
//! A work item shows its panel rows only while its fold is open, so a sidebar
//! with many work items holds one line for each of them. The fold is a state of
//! this run: it is not saved with the session, and a start shows every work item
//! folded. The flag stands for one workspace by its local id, because the id of a
//! workspace restarts at 1 in a replaced session.
//!
//! While the accordion setting is on, one work item shows its panels at a time:
//! opening one folds every other one.

use std::collections::BTreeSet;

use egui::{Pos2, Rect, Sense, Stroke, WidgetInfo, pos2};

use crate::theme;

/// The side of the fold control and its right inset in its row.
pub(super) const SIDE: f32 = 18.0;
pub(super) const INSET: f32 = 6.0;
/// The stroke of the chevron and the half of its width and height.
const STROKE: f32 = 1.5;
const HALF: f32 = 2.5;
/// The hover text of the control, which is also its accessible name.
pub(in crate::app) const SHOW: &str = "Show panels";
pub(in crate::app) const HIDE: &str = "Hide panels";

/// The fold of the panel rows of the work items of this run.
#[derive(Default)]
pub(in crate::app) struct Folds {
    open: BTreeSet<String>,
}

impl Folds {
    /// Whether the work item of `local_id` shows its panel rows.
    pub(in crate::app) fn is_open(&self, local_id: &str) -> bool {
        self.open.contains(local_id)
    }

    /// Opens or folds the work item of `local_id`. With `accordion` on, opening
    /// one work item folds every other one.
    pub(in crate::app) fn toggle(&mut self, local_id: &str, accordion: bool) {
        if self.open.remove(local_id) {
            return;
        }
        if accordion {
            self.open.clear();
        }
        self.open.insert(local_id.to_owned());
    }

    /// Folds the work item of `local_id`, which a work item that leaves the board
    /// does.
    pub(in crate::app) fn close(&mut self, local_id: &str) {
        self.open.remove(local_id);
    }
}

/// The chevron of the fold control of a row: it points right while the work item
/// is folded, down while it is open, and it does not show on a row at rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mark {
    /// The row keeps no room for the control: the pointer is elsewhere and the
    /// work item is folded.
    Hidden,
    Folded,
    Open,
}

impl Mark {
    /// The mark of a row that the pointer is on, or of an open row.
    pub(super) const fn of(open: bool, hovered: bool, foldable: bool) -> Self {
        if !foldable {
            Self::Hidden
        } else if open {
            Self::Open
        } else if hovered {
            Self::Folded
        } else {
            Self::Hidden
        }
    }

    /// Whether the row shows the control and keeps its room clear.
    pub(super) const fn shows(self) -> bool {
        !matches!(self, Self::Hidden)
    }

    /// The hover text of the control.
    const fn words(self) -> &'static str {
        match self {
            Self::Open => HIDE,
            Self::Hidden | Self::Folded => SHOW,
        }
    }
}

/// The rectangle of the fold control of the row `row`: its right end.
pub(super) fn rect(row: Rect) -> Rect {
    Rect::from_min_size(
        pos2(row.max.x - INSET - SIDE, row.center().y - SIDE / 2.0),
        egui::Vec2::splat(SIDE),
    )
}

/// Draws the fold control of `key` in `rect` and answers whether the person
/// pressed it.
pub(super) fn render(ui: &egui::Ui, rect: Rect, key: &str, mark: Mark) -> bool {
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("sidebar_work_item_fold", key)),
        Sense::click(),
    );
    let color = if response.hovered() {
        theme::FG()
    } else {
        theme::FG_DIM()
    };
    paint(ui, rect.center(), mark, color);
    let words = mark.words();
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, words));
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let response = response.on_hover_text(words);
    // The control shows on the frame that the pointer arrives, so a quick click of a
    // person can press and release it inside one frame of 55 ms, before the toolkit
    // knows the control: the press of this frame is read from its events as well.
    response.clicked() || press_and_release_here(ui, rect)
}

/// Whether a press and its release both landed on `rect` in this frame.
fn press_and_release_here(ui: &egui::Ui, rect: Rect) -> bool {
    ui.input(|input| {
        let mut pressed = false;
        let mut released = false;
        for event in &input.events {
            if let egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: down,
                ..
            } = event
                && rect.contains(*pos)
            {
                if *down {
                    pressed = true;
                } else {
                    released = true;
                }
            }
        }
        pressed && released
    })
}

/// Paints the chevron of `mark` at `center`.
fn paint(ui: &egui::Ui, center: Pos2, mark: Mark, color: egui::Color32) {
    let stroke = Stroke::new(STROKE, color);
    let painter = ui.painter();
    match mark {
        Mark::Hidden => {}
        Mark::Folded => {
            painter.line_segment(
                [pos2(center.x - HALF, center.y - HALF), pos2(center.x + HALF, center.y)],
                stroke,
            );
            painter.line_segment(
                [pos2(center.x + HALF, center.y), pos2(center.x - HALF, center.y + HALF)],
                stroke,
            );
        }
        Mark::Open => {
            painter.line_segment(
                [pos2(center.x - HALF, center.y - HALF), pos2(center.x, center.y + HALF)],
                stroke,
            );
            painter.line_segment(
                [pos2(center.x, center.y + HALF), pos2(center.x + HALF, center.y - HALF)],
                stroke,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Folds, HIDE, Mark, SHOW};

    #[test]
    fn every_work_item_starts_folded() {
        let folds = Folds::default();
        assert!(!folds.is_open("a"), "a start folds every work item");
    }

    #[test]
    fn a_press_opens_a_work_item_and_the_next_one_folds_it() {
        let mut folds = Folds::default();
        folds.toggle("a", false);
        assert!(folds.is_open("a"));
        folds.toggle("a", false);
        assert!(!folds.is_open("a"), "the second press folds it again");
    }

    #[test]
    fn the_accordion_keeps_one_work_item_open() {
        let mut folds = Folds::default();
        folds.toggle("a", true);
        folds.toggle("b", true);
        assert!(!folds.is_open("a"), "opening b folded a");
        assert!(folds.is_open("b"));

        // Without the accordion both stay open.
        folds.toggle("a", false);
        assert!(folds.is_open("a") && folds.is_open("b"));
    }

    #[test]
    fn the_control_shows_on_the_row_under_the_pointer_and_on_an_open_row() {
        assert_eq!(Mark::of(false, false, true), Mark::Hidden);
        assert_eq!(Mark::of(false, true, true), Mark::Folded);
        assert_eq!(Mark::of(true, false, true), Mark::Open);
        assert_eq!(Mark::of(true, true, true), Mark::Open);
        assert_eq!(
            Mark::of(false, true, false),
            Mark::Hidden,
            "a work item with no panel has no control"
        );
        assert!(Mark::Folded.shows() && Mark::Open.shows());
        assert!(!Mark::Hidden.shows());
        assert_eq!(Mark::Folded.words(), SHOW);
        assert_eq!(Mark::Open.words(), HIDE);
        assert_eq!(SHOW, "Show panels");
        assert_eq!(HIDE, "Hide panels");
    }
}
