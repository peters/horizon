//! The steps of the current attempt: a vertical list in an empty cloud's body and a
//! horizontal stepper in the drawer, with the running step opened in place.
use super::super::{Runtime, Stage};
use super::status::{Status, Tone};
use super::strip::{stage_color, tone_color};
use super::timeline::short;
use crate::theme;
use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StepAction {
    /// The failure's one next action: its fix, or a retry.
    Next,
    CopyError,
    /// Connect GitHub again: reconnect after the cloud was marked for a new sign-in.
    Reconnect,
}

const ROW: f32 = 30.0;

enum Mark {
    Done,
    Running,
    Failed,
    Pending,
    /// A step this cloud's deployment never runs, such as Build for an image-only profile.
    Skipped,
}

fn mark(status: &Status, index: usize) -> Mark {
    let track = &status.track;
    if track.current == Some(index) {
        if track.failed { Mark::Failed } else { Mark::Running }
    } else if track.skipped.contains(&track.stages[index]) {
        Mark::Skipped
    } else if index < track.finished {
        Mark::Done
    } else {
        Mark::Pending
    }
}

fn paint_mark(ui: &egui::Ui, center: Pos2, mark: &Mark, stage: Stage, faded: bool) {
    let painter = ui.painter();
    match mark {
        Mark::Done => {
            let color = stage_color(stage);
            let color = if faded {
                theme::blend(color, theme::PANEL_BG(), 0.45)
            } else {
                color
            };
            painter.circle_filled(center, 7.0, color);
            check(painter, center, 9.0, theme::PANEL_BG());
        }
        Mark::Failed => {
            painter.circle_filled(center, 8.0, theme::PALETTE_RED());
            cross(painter, center, 9.0, theme::PANEL_BG());
        }
        Mark::Running => {
            // The loading spinner: one turn per 1.2 s, repainted only while it is drawn.
            let turn = super::strip::cycle(ui.input(|input| input.time), 1.0 / 1.2);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(32));
            let points: Vec<Pos2> = (0..=20_u8)
                .map(|step| {
                    let angle = turn * std::f32::consts::TAU + f32::from(step) / 20.0 * 4.2;
                    center + vec2(angle.cos(), angle.sin()) * 6.5
                })
                .collect();
            painter.add(Shape::line(points, Stroke::new(2.0, theme::PALETTE_CYAN())));
        }
        Mark::Pending => {
            painter.circle_stroke(center, 5.5, Stroke::new(1.3, theme::BORDER_STRONG()));
        }
        Mark::Skipped => {
            painter.circle_stroke(center, 5.5, Stroke::new(1.3, theme::BORDER_SUBTLE()));
            painter.line_segment(
                [center - vec2(3.0, 0.0), center + vec2(3.0, 0.0)],
                Stroke::new(1.5, theme::FG_DIM()),
            );
        }
    }
}

pub(super) fn check(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    painter.add(Shape::line(
        vec![
            center + vec2(-0.45, 0.0) * size,
            center + vec2(-0.12, 0.33) * size,
            center + vec2(0.45, -0.3) * size,
        ],
        Stroke::new(size * 0.18, color),
    ));
}

pub(super) fn cross(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    let reach = 0.36 * size;
    let stroke = Stroke::new(size * 0.17, color);
    painter.line_segment([center + vec2(-reach, -reach), center + vec2(reach, reach)], stroke);
    painter.line_segment([center + vec2(-reach, reach), center + vec2(reach, -reach)], stroke);
}

/// What a screen reader says for a step: its name, state and time.
fn spoken(stage: Stage, mark: &Mark, elapsed: Option<std::time::Duration>) -> String {
    let progress = match mark {
        Mark::Done => "done",
        Mark::Running => "running",
        Mark::Failed => "failed",
        Mark::Pending => "pending",
        Mark::Skipped => "skipped",
    };
    match elapsed {
        Some(elapsed) => format!("{}: {progress}, {}", stage.label(), short(elapsed)),
        None => format!("{}: {progress}", stage.label()),
    }
}

/// Registers a painted step with assistive technology.
fn announce(response: &egui::Response, stage: Stage, mark: &Mark, elapsed: Option<std::time::Duration>) {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, spoken(stage, mark, elapsed)));
}

fn label_color(mark: &Mark) -> Color32 {
    match mark {
        Mark::Running => theme::FG(),
        Mark::Failed => theme::PALETTE_RED(),
        Mark::Done => theme::FG_SOFT(),
        Mark::Pending | Mark::Skipped => theme::FG_DIM(),
    }
}

/// The text in a step's time slot: its measured time, or that it is skipped.
fn time_text(runtime: &Runtime, stage: Stage, mark: &Mark) -> Option<String> {
    match mark {
        Mark::Skipped => Some("skipped".to_owned()),
        _ => runtime.progress.stage_duration(stage).map(short),
    }
}

/// Every step top to bottom. The running step shows its measured progress, a failed
/// one its cause with its next action and Copy error.
pub(super) fn vertical(ui: &mut egui::Ui, runtime: &Runtime, status: &Status) -> Option<StepAction> {
    let mut action = None;
    let stages = status.track.stages;
    ui.spacing_mut().item_spacing.y = 0.0;
    for (index, stage) in stages.iter().enumerate() {
        let mark = mark(status, index);
        let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
        announce(&row, *stage, &mark, runtime.progress.stage_duration(*stage));
        let center = pos2(rect.left() + 8.0, rect.top() + 11.0);
        let open = matches!(mark, Mark::Running | Mark::Failed);
        let done = matches!(mark, Mark::Done);
        let next_top = rect.bottom();
        paint_mark(ui, center, &mark, *stage, status.track.faded);
        ui.painter().text(
            pos2(rect.left() + 26.0, rect.top() + 3.0),
            Align2::LEFT_TOP,
            stage.label(),
            FontId::proportional(14.5),
            label_color(&mark),
        );
        if let Some(time) = time_text(runtime, *stage, &mark) {
            ui.painter().text(
                pos2(rect.right(), rect.top() + 4.0),
                Align2::RIGHT_TOP,
                time,
                FontId::monospace(12.5),
                if open { theme::FG_SOFT() } else { theme::FG_DIM() },
            );
        }
        if open {
            ui.horizontal(|ui| {
                ui.add_space(26.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 5.0;
                    action = opened(ui, runtime, status).or(action);
                });
            });
            ui.add_space(8.0);
        }
        // The connector runs from this mark to the next one.
        if index + 1 < stages.len() {
            let bottom = ui.cursor().top().max(next_top) + 3.0;
            ui.painter().line_segment(
                [center + vec2(0.0, 8.0), pos2(center.x, bottom)],
                Stroke::new(
                    1.5,
                    if done {
                        stage_color(*stage)
                    } else {
                        theme::BORDER_SUBTLE()
                    },
                ),
            );
        }
    }
    action
}

fn opened(ui: &mut egui::Ui, runtime: &Runtime, status: &Status) -> Option<StepAction> {
    if let Some(failure) = &status.failure {
        ui.label(
            RichText::new(failure.headline())
                .monospace()
                .size(13.0)
                .color(theme::PALETTE_RED()),
        );
        if let Some(meaning) = failure.meaning {
            ui.label(RichText::new(meaning).size(12.0).color(theme::FG_SOFT()));
        } else if failure.cause.is_some() {
            ui.label(RichText::new(&failure.summary).size(12.0).color(theme::FG_SOFT()));
        }
        ui.add_space(4.0);
        let mut action = None;
        // A long next action moves Copy error to the next row rather than wrapping a label.
        let button =
            |label| crate::app::cloud_panel::runtime::action_button(label).wrap_mode(egui::TextWrapMode::Extend);
        ui.horizontal_wrapped(|ui| {
            if let Some(next) = super::next::Next::of(status)
                && ui.add(button(next.label())).clicked()
            {
                action = Some(StepAction::Next);
            }
            if ui.add(button("Copy error")).clicked() {
                action = Some(StepAction::CopyError);
            }
            super::docker::button(ui, failure);
        });
        let retry = super::next::Next::of(status).filter(|next| matches!(next, super::next::Next::Retry(_)));
        if super::docker::status(ui, failure, retry.map(super::next::Next::label)) {
            action = Some(StepAction::Next);
        }
        return action;
    }
    let measured = runtime.progress.measured();
    let detail = measured
        .as_ref()
        .map(|measured| measured.detail)
        .or_else(|| runtime.progress.activity());
    if let Some(detail) = detail {
        ui.add(egui::Label::new(RichText::new(detail).size(12.0).color(theme::FG_DIM())).truncate());
    }
    if let Some(fraction) = measured.as_ref().and_then(super::super::progress::Measured::fraction) {
        bar(
            ui,
            fraction,
            status
                .track
                .current
                .map_or(theme::ACCENT(), |index| stage_color(status.track.stages[index])),
        );
    }
    let numbers = [status.numbers.as_str(), status.tail.as_str()]
        .into_iter()
        .filter(|text| !text.is_empty() && Some(*text) != detail)
        .collect::<Vec<_>>()
        .join("  ·  ");
    if !numbers.is_empty() {
        ui.label(RichText::new(numbers).size(12.0).color(theme::FG_SOFT()));
    }
    None
}

pub(super) fn bar(ui: &mut egui::Ui, fraction: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    ui.painter().rect_filled(rect, 3, theme::BORDER_SUBTLE());
    let filled = Rect::from_min_size(rect.min, vec2(rect.width() * fraction.clamp(0.0, 1.0), rect.height()));
    ui.painter().rect_filled(filled, 3, color);
}

/// A step's name that fits `room`: its label, else its short name; when neither fits,
/// only the first, last and running or failed steps are named.
fn label_for(ui: &egui::Ui, stage: Stage, room: f32, index: usize, count: usize, mark: &Mark) -> Option<&'static str> {
    let fits = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), FontId::proportional(13.0), theme::FG())
            .size()
            .x
            <= room
    };
    if fits(stage.label()) {
        return Some(stage.label());
    }
    let short = short_label(stage);
    let named = index == 0 || index + 1 == count || matches!(mark, Mark::Running | Mark::Failed);
    (fits(short) || named).then_some(short)
}

/// One word per step for narrow steppers.
pub(super) fn short_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Validate => "Validate",
        Stage::Build => "Build",
        Stage::Push => "Push",
        Stage::Replace => "Replace",
        Stage::Provision => "Provision",
        Stage::Readiness => "Readiness",
        Stage::Worktrees => "Worktrees",
        Stage::Sessions => "Sessions",
        Stage::Ready => "Ready",
        Stage::Stopped => "Stopped",
        Stage::Stopping => "Stopping",
        Stage::Deleted => "Deleted",
        Stage::ReleaseDevices => "Devices",
        Stage::DeleteWorker => "Worker",
        Stage::DeleteStorage => "Storage",
    }
}

/// Nodes across the drawer with each step's name and time.
pub(super) fn horizontal(ui: &mut egui::Ui, runtime: &Runtime, status: &Status) {
    let stages = status.track.stages;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
    let step = rect.width() / crate::app::util::usize_to_f32(stages.len().max(1));
    let y = rect.top() + 12.0;
    ui.painter().line_segment(
        [pos2(rect.left() + step / 2.0, y), pos2(rect.right() - step / 2.0, y)],
        Stroke::new(2.0, theme::BORDER_SUBTLE()),
    );
    for (index, stage) in stages.iter().enumerate() {
        let center = pos2(rect.left() + step * (crate::app::util::usize_to_f32(index) + 0.5), y);
        let mark = mark(status, index);
        let node = Rect::from_center_size(pos2(center.x, rect.center().y), vec2(step, rect.height()));
        announce(
            &ui.interact(node, ui.id().with(("step", index)), Sense::hover()),
            *stage,
            &mark,
            runtime.progress.stage_duration(*stage),
        );
        if matches!(mark, Mark::Done) && index + 1 < stages.len() {
            ui.painter().line_segment(
                [center, center + vec2(step, 0.0)],
                Stroke::new(2.0, stage_color(*stage)),
            );
        }
        if matches!(mark, Mark::Running) {
            ui.painter().circle_filled(center, 11.0, theme::PANEL_BG_ALT());
        }
        paint_mark(ui, center, &mark, *stage, status.track.faded);
        let color = match mark {
            Mark::Running => tone_color(Tone::Live),
            Mark::Failed => theme::PALETTE_RED(),
            Mark::Done => theme::FG_SOFT(),
            Mark::Pending | Mark::Skipped => theme::FG_DIM(),
        };
        let clip = ui
            .painter()
            .with_clip_rect(Rect::from_center_size(center + vec2(0.0, 30.0), vec2(step - 4.0, 44.0)));
        if let Some(label) = label_for(ui, *stage, step - 6.0, index, stages.len(), &mark) {
            clip.text(
                center + vec2(0.0, 20.0),
                Align2::CENTER_TOP,
                label,
                FontId::proportional(13.0),
                color,
            );
        }
        if let Some(time) = time_text(runtime, *stage, &mark) {
            clip.text(
                center + vec2(0.0, 38.0),
                Align2::CENTER_TOP,
                time,
                FontId::monospace(12.0),
                theme::FG_DIM(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::status::Track;
    use super::*;

    #[test]
    fn a_skipped_step_is_neither_done_nor_pending_and_says_so() {
        let track = Track {
            stages: &Stage::ALL,
            current: Some(4),
            finished: 4,
            fraction: None,
            failed: false,
            faded: false,
            skipped: &[Stage::Build, Stage::Push],
        };
        let status = Status {
            tone: Tone::Live,
            verb: String::new(),
            numbers: String::new(),
            tail: String::new(),
            right: String::new(),
            failure: None,
            track,
            primary: None,
            binds_image: false,
        };
        let marks: Vec<_> = (0..Stage::ALL.len()).map(|index| mark(&status, index)).collect();
        assert!(matches!(marks[0], Mark::Done));
        assert!(matches!(marks[1], Mark::Skipped) && matches!(marks[2], Mark::Skipped));
        assert!(matches!(marks[3], Mark::Done));
        assert!(matches!(marks[4], Mark::Running));
        assert_eq!(spoken(Stage::Build, &marks[1], None), "Build locally: skipped");
        let runtime = Runtime::default();
        assert_eq!(time_text(&runtime, Stage::Build, &marks[1]).as_deref(), Some("skipped"));
        assert_eq!(time_text(&runtime, Stage::Validate, &marks[0]), None);
    }
}
