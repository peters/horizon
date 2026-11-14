//! The steps of a longer piece of work: a row for each step, with its state
//! and an optional short time at the right edge.
//!
//! A step is a report, not a control: it takes no press and no focus, and it
//! reaches no agent. The mark of its state is Horizon's drawing, so a reader
//! hears the line of the step and the time beside it.

use egui::{FontId, Pos2, Rect, Role, Sense, Stroke, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::{Step, StepState};

use crate::theme;
use crate::typeface::Weight;

use super::metrics::{STEP_FONT, STEP_GAP, STEP_MARK, STEP_MARK_GAP, STEP_MARK_RING, STEP_TIME_FONT, STEP_TIME_GAP};
use super::text;

/// The mark of a step in a colour of the theme, and the distance from the top of
/// its row to its centre.
const MARK_TOP: f32 = 1.0;

/// Draw a `steps` component.
pub(super) fn show(ui: &mut Ui, index: usize, steps: &[Step]) {
    for (step_index, step) in steps.iter().enumerate() {
        if step_index > 0 {
            ui.add_space(STEP_GAP);
        }
        show_step(ui, index, step_index, step);
    }
}

fn show_step(ui: &mut Ui, index: usize, step_index: usize, step: &Step) {
    let width = ui.available_width();
    let text_font = FontId::proportional(STEP_FONT);
    let time_font = FontId::monospace(STEP_TIME_FONT);
    let time = step
        .at
        .as_deref()
        .map(|at| text::line(ui.painter(), at, &time_font, theme::FG_DIM(), (width * 0.5).max(0.0)));
    let time_width = time.as_ref().map_or(0.0, |galley| galley.size().x + STEP_TIME_GAP);
    // The label of a failed step keeps the colour of a line: the cross and the
    // time carry the danger.
    let colour = line_colour(step.state);
    let label = text::line_weight(
        ui.painter(),
        &step.label,
        &text_font,
        colour,
        (width - STEP_MARK - STEP_MARK_GAP - time_width).max(0.0),
        step_weight(step.state),
    );
    let height = label
        .size()
        .y
        .max(STEP_MARK + MARK_TOP)
        .max(time.as_ref().map_or(0.0, |galley| galley.size().y));

    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, &step.label));
    if step.at.is_some() {
        time_node(ui, index, step_index, rect, height, step.at.as_deref());
    }

    let painter = ui.painter();
    let mark = Rect::from_min_size(Pos2::new(rect.min.x, rect.min.y + MARK_TOP), Vec2::splat(STEP_MARK));
    paint_mark(painter, mark, step.state);
    let text_left = rect.min.x + STEP_MARK + STEP_MARK_GAP;
    painter.galley(
        Pos2::new(text_left, rect.center().y - label.size().y * 0.5),
        label,
        colour,
    );
    if let Some(galley) = time {
        let time_height = galley.size().y;
        let time_width = galley.size().x;
        painter.galley(
            Pos2::new(rect.max.x - time_width, rect.center().y - time_height * 0.5),
            galley,
            time_colour(step.state),
        );
    }
}

/// The colour of the time at the right edge of a step: the time of a step that
/// did not finish carries the danger.
fn time_colour(state: StepState) -> egui::Color32 {
    match state {
        StepState::Failed => theme::PALETTE_RED(),
        StepState::Done | StepState::Now | StepState::Waiting => theme::FG_DIM(),
    }
}

/// The colour of the line of a step: a step that runs or finished reads as the
/// foreground, a step that waits stays soft.
/// The weight of a step label: the step that is under way is the one the
/// approved card mockups draw heavier.
fn step_weight(state: StepState) -> Weight {
    match state {
        StepState::Now => Weight::Semibold,
        StepState::Done | StepState::Waiting | StepState::Failed => Weight::Regular,
    }
}

fn line_colour(state: StepState) -> egui::Color32 {
    match state {
        StepState::Done | StepState::Now | StepState::Failed => theme::FG(),
        StepState::Waiting => theme::FG_SOFT(),
    }
}

/// Paints the mark of a step: a green disc with a check when it finished, an
/// accent ring with a halo while it runs, an empty ring while it waits, and the
/// red disc with a cross of a failed board item when it did not finish.
fn paint_mark(painter: &egui::Painter, mark: Rect, state: StepState) {
    let center = mark.center();
    let radius = STEP_MARK * 0.5;
    let ink = Stroke::new(STEP_MARK_RING, theme::PANEL_BG());
    match state {
        StepState::Done => {
            painter.circle_filled(center, radius, theme::PALETTE_GREEN());
            let left = Pos2::new(center.x - radius * 0.45, center.y);
            let bottom = Pos2::new(center.x - radius * 0.1, center.y + radius * 0.35);
            let right = Pos2::new(center.x + radius * 0.5, center.y - radius * 0.35);
            painter.line_segment([left, bottom], ink);
            painter.line_segment([bottom, right], ink);
        }
        StepState::Failed => {
            painter.circle_filled(center, radius, theme::PALETTE_RED());
            let left = Pos2::new(center.x - radius * 0.4, center.y - radius * 0.4);
            let right = Pos2::new(center.x + radius * 0.4, center.y + radius * 0.4);
            painter.line_segment([left, right], ink);
            painter.line_segment([Pos2::new(right.x, left.y), Pos2::new(left.x, right.y)], ink);
        }
        StepState::Now => {
            painter.circle_filled(center, radius, theme::alpha(theme::ACCENT(), 48));
            painter.circle_stroke(
                center,
                radius - STEP_MARK_RING * 0.5,
                Stroke::new(STEP_MARK_RING, theme::ACCENT()),
            );
        }
        StepState::Waiting => {
            painter.circle_stroke(
                center,
                radius - STEP_MARK_RING * 0.5,
                Stroke::new(STEP_MARK_RING, theme::BORDER_STRONG()),
            );
        }
    }
}

/// Registers the time of a step as its own node, as the value of a `kv` pair is.
fn time_node(ui: &Ui, index: usize, step_index: usize, row: Rect, height: f32, at: Option<&str>) {
    let Some(at) = at else { return };
    let rect = Rect::from_min_max(
        Pos2::new(row.min.x, row.min.y),
        Pos2::new(row.max.x, row.min.y + height),
    );
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("block_step_time", index, step_index)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, at));
}
