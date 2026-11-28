//! The caption of a group of the sidebar: the name of the group in upper case,
//! the count of its rows and the icon controls at its right edge.
//!
//! A caption replaces every group heading: no group of the sidebar names itself
//! with a text button, so a person reads one kind of heading and one kind of
//! control. The count stands 8 after the name, in the one monospace face that the
//! numbers of the sidebar use, and it is built again only when it changes.
//!
//! An icon control is a press target of its own with a hover text and the same
//! accessible name. Its glyph is drawn from lines, so a frame paints it without a
//! font: the sidebar lays out no glyph picture of a control.

use std::collections::BTreeMap;

use egui::{Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, WidgetInfo, pos2};

use crate::theme;

use super::text;

/// The height of a caption.
pub(in crate::app) const HEIGHT: f32 = 22.0;
/// The gap above a group: between two groups of the sidebar.
pub(in crate::app) const GROUP_GAP: f32 = 14.0;
/// The gap from a caption to the first row of its group.
pub(in crate::app) const ROW_GAP: f32 = 6.0;
/// The left inset of the name.
const LEFT_INSET: f32 = 16.0;
/// The gap between the name and the count.
const COUNT_GAP: f32 = 8.0;
/// The size of the name and of the count.
const SIZE: f32 = 10.0;
/// The extra letter spacing of the name.
const SPACING: f32 = 1.1;
/// The side, the radius and the right inset of an icon control.
const CONTROL_SIDE: f32 = 22.0;
const CONTROL_RADIUS: u8 = 7;
const CONTROL_INSET: f32 = 12.0;
/// The gap between two icon controls.
const CONTROL_GAP: f32 = 4.0;
/// The stroke of a glyph and the length of its lines.
const GLYPH_STROKE: f32 = 1.5;
const PLUS_LINE: f32 = 9.0;
/// The side of the square of a fit mark and the length of its four arms.
const FIT_SIDE: f32 = 10.0;
const FIT_ARM: f32 = 3.0;

/// The name of the group `Needs you`, in the form of a caption.
pub(in crate::app) const NEEDS_YOU: &str = "NEEDS YOU";
/// The name of the group `Work items`, in the form of a caption.
pub(in crate::app) const WORK_ITEMS: &str = "WORK ITEMS";
/// The name of the group `Machines`, in the form of a caption.
pub(in crate::app) const MACHINES: &str = "MACHINES";
/// The name of the group `Agents`, in the form of a caption.
pub(in crate::app) const AGENTS: &str = "AGENTS";
/// The name of the group `Computers`, in the form of a caption.
pub(in crate::app) const COMPUTERS: &str = "COMPUTERS";

/// The hover text of the control that fits every work item in view, and of the one
/// that makes a new work item.
pub(super) const FIT_HOVER: &str = "Fit all in view";
pub(super) const NEW_HOVER: &str = "New work item";

/// The mark of an icon control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Glyph {
    /// Two lines that cross at the centre.
    Plus,
    /// The four corner marks of a square.
    Fit,
}

/// One icon control of a caption: its glyph, the words of its hover and of its
/// accessible name, and whether it takes a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Control {
    pub(super) glyph: Glyph,
    /// The hover text, which is also the accessible name of the control.
    pub(super) words: &'static str,
    /// Whether the control takes a press. A control that does not take one is
    /// drawn in the dim colour and takes no click.
    pub(super) enabled: bool,
}

impl Control {
    /// The control of `glyph` that takes a press, with the words of its hover.
    pub(super) const fn new(glyph: Glyph, words: &'static str) -> Self {
        Self {
            glyph,
            words,
            enabled: true,
        }
    }

    /// The control of `glyph` that takes no press.
    pub(super) const fn disabled(glyph: Glyph, words: &'static str) -> Self {
        Self {
            glyph,
            words,
            enabled: false,
        }
    }
}

/// What a press on an icon control asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Press {
    Plus,
    Fit,
}

impl Glyph {
    /// The press of this control.
    const fn press(self) -> Press {
        match self {
            Self::Plus => Press::Plus,
            Self::Fit => Press::Fit,
        }
    }
}

/// The count strings of the captions, built again only when a count changes.
#[derive(Default)]
pub(in crate::app) struct Counts {
    held: BTreeMap<&'static str, (usize, String)>,
}

impl Counts {
    /// The text of the count `value` of the caption `group`.
    pub(in crate::app) fn text(&mut self, group: &'static str, value: usize) -> &str {
        let held = self.held.entry(group).or_insert_with(|| (value, value.to_string()));
        if held.0 != value {
            *held = (value, value.to_string());
        }
        &held.1
    }
}

/// Draws the group `caption` of the sidebar: the gap above it, the caption with
/// its count and its icon controls, and the gap under it, before its first row.
/// It answers the press on one of the icon controls.
pub(super) fn group(ui: &mut egui::Ui, caption: &str, count: Option<&str>, controls: &[Control]) -> Option<Press> {
    ui.add_space(GROUP_GAP);
    let press = render(ui, caption, count, controls);
    ui.add_space(ROW_GAP);
    press
}

/// Draws one caption in the height [`HEIGHT`], with its count and its icon
/// controls. The first control stands at the right edge of the caption.
pub(super) fn render(ui: &mut egui::Ui, caption: &str, count: Option<&str>, controls: &[Control]) -> Option<Press> {
    let row = ui.allocate_space(Vec2::new(ui.available_width(), HEIGHT)).1;
    let painter = ui.painter();
    let left = row.min.x + LEFT_INSET;
    let right = row.max.x - CONTROL_INSET - CONTROL_SIDE * usize_to_f32(controls.len());
    let right = right - CONTROL_GAP * usize_to_f32(controls.len().saturating_sub(1));
    let name = text::spaced(
        ui,
        caption,
        SIZE,
        text::SEMIBOLD,
        theme::FG_DIM(),
        SPACING,
        (right - left).max(1.0),
    );
    let name_height = name.size().y;
    let name_rect = Rect::from_min_size(pos2(left, row.center().y - name_height / 2.0), name.size());
    painter.galley(name_rect.min, name, theme::FG_DIM());
    if let Some(count) = count {
        let galley = text::mono(ui, count, SIZE, theme::FG_DIM());
        let count_left = name_rect.max.x + COUNT_GAP;
        ui.painter().galley(
            pos2(count_left, row.center().y - galley.size().y / 2.0),
            galley,
            theme::FG_DIM(),
        );
    }

    let mut pressed = None;
    let mut control_right = row.max.x - CONTROL_INSET;
    for control in controls {
        let rect = Rect::from_min_size(
            pos2(control_right - CONTROL_SIDE, row.center().y - CONTROL_SIDE / 2.0),
            Vec2::splat(CONTROL_SIDE),
        );
        if let Some(press) = render_control(ui, rect, caption, *control) {
            pressed = Some(press);
        }
        control_right = rect.min.x - CONTROL_GAP;
    }
    pressed
}

/// Draws one icon control and answers its press. A control that takes no press
/// answers none and shows no hover, as the text button that it replaces.
pub(super) fn render_control(ui: &mut egui::Ui, rect: Rect, caption: &str, control: Control) -> Option<Press> {
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("sidebar_caption_control", caption, control.words)),
        Sense::click(),
    );
    let hovered = control.enabled && response.hovered();
    let color = match (control.enabled, hovered) {
        (true, true) => theme::FG(),
        (true, false) => theme::FG_SOFT(),
        (false, _) => theme::FG_DIM(),
    };
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(rect, CONTROL_RADIUS, theme::PANEL_BG_ALT());
        painter.rect_stroke(
            rect,
            CONTROL_RADIUS,
            Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
            StrokeKind::Inside,
        );
    }
    paint_glyph(painter, rect.center(), control.glyph, color);
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, control.enabled, control.words));
    let response = response.on_hover_text(control.words);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    (control.enabled && response.clicked()).then(|| control.glyph.press())
}

/// Paints the glyph `glyph` at `center` in lines of [`GLYPH_STROKE`].
fn paint_glyph(painter: &egui::Painter, center: Pos2, glyph: Glyph, color: egui::Color32) {
    let stroke = Stroke::new(GLYPH_STROKE, color);
    match glyph {
        Glyph::Plus => {
            let half = PLUS_LINE / 2.0;
            painter.line_segment(
                [pos2(center.x - half, center.y), pos2(center.x + half, center.y)],
                stroke,
            );
            painter.line_segment(
                [pos2(center.x, center.y - half), pos2(center.x, center.y + half)],
                stroke,
            );
        }
        Glyph::Fit => {
            // The four corner marks of a square: each one turns the corner of the
            // square with two lines of `FIT_ARM`.
            let half = FIT_SIDE / 2.0;
            let (left, right) = (center.x - half, center.x + half);
            let (top, bottom) = (center.y - half, center.y + half);
            for (corner, across, down) in [
                (pos2(left, top), 1.0_f32, 1.0_f32),
                (pos2(right, top), -1.0_f32, 1.0_f32),
                (pos2(left, bottom), 1.0_f32, -1.0_f32),
                (pos2(right, bottom), -1.0_f32, -1.0_f32),
            ] {
                painter.line_segment([corner, pos2(corner.x + across * FIT_ARM, corner.y)], stroke);
                painter.line_segment([corner, pos2(corner.x, corner.y + down * FIT_ARM)], stroke);
            }
        }
    }
}

/// `value` as a float, for the geometry of a caption.
fn usize_to_f32(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

/// The rectangle of the control at `index` of a caption `width` wide, which a
/// test reads.
#[cfg(test)]
pub(super) fn control_rect(width: f32, row_top: f32, index: usize) -> Rect {
    let mut right = width - CONTROL_INSET;
    for _ in 0..index {
        right -= CONTROL_SIDE + CONTROL_GAP;
    }
    Rect::from_min_size(
        pos2(right - CONTROL_SIDE, row_top + (HEIGHT - CONTROL_SIDE) / 2.0),
        Vec2::splat(CONTROL_SIDE),
    )
}

#[cfg(test)]
mod tests {
    use super::{Counts, Glyph, Press, control_rect};

    #[test]
    fn a_count_is_built_again_only_when_it_changes() {
        let mut counts = Counts::default();
        assert_eq!(counts.text("WORK ITEMS", 2), "2");
        assert_eq!(counts.text("WORK ITEMS", 2), "2");
        assert_eq!(counts.text("WORK ITEMS", 3), "3", "a changed count is read again");
        assert_eq!(counts.held.len(), 1, "one caption holds one count");
    }

    #[test]
    fn a_caption_draws_its_controls_at_its_right_edge() {
        // The controls of a caption at the width of the sidebar of the design.
        let fit = control_rect(214.0, 0.0, 0);
        assert!((fit.max.x - (214.0 - 12.0)).abs() < f32::EPSILON);
        assert!((fit.width() - 22.0).abs() < f32::EPSILON);
        assert!((fit.height() - 22.0).abs() < f32::EPSILON);
        let plus = control_rect(214.0, 0.0, 1);
        assert!((fit.min.x - plus.max.x - 4.0).abs() < f32::EPSILON);
        assert!((plus.min.x - 154.0).abs() < f32::EPSILON, "{plus:?}");
    }

    #[test]
    fn the_words_of_the_controls_are_the_words_of_the_design() {
        assert_eq!(super::Control::new(Glyph::Plus, "New work item").words, "New work item");
        assert_eq!(
            super::Control::new(Glyph::Fit, "Fit all in view").words,
            "Fit all in view"
        );
        assert!(!super::Control::disabled(Glyph::Fit, "Fit all in view").enabled);
        assert_eq!(Glyph::Plus.press(), Press::Plus);
        assert_eq!(Glyph::Fit.press(), Press::Fit);
    }
}
