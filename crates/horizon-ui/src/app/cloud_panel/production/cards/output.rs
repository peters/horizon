//! A cloud operation's output as a first-class view: grouped by step, timed, with
//! failures highlighted and the decisive line pinned below the log.
use super::super::{LogLine, Runtime, Stage};
use super::status::Failure;
use super::strip::stage_color;
use crate::theme;
use egui::{Color32, FontId, RichText, Stroke, Vec2};
use horizon_core::cloud_runtime::diagnosis;

const LOG_BG: Color32 = Color32::from_rgb(9, 12, 19);
const TEXT_SIZE: f32 = 13.0;

/// The log in `height`, with `failure` pinned under it. `place` keeps the scroll
/// position of the body's log apart from the drawer's.
pub(super) fn show(
    ui: &mut egui::Ui,
    id: u32,
    place: &str,
    runtime: &mut Runtime,
    height: f32,
    failure: Option<&Failure>,
) {
    let pinned = failure.map_or(0.0, |_| 62.0);
    let frame = egui::Frame::new()
        .fill(LOG_BG)
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 10));
    let inner = (height - pinned - frame.total_margin().sum().y).max(60.0);
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        // The painted log stays at the follow-mode tail. Newer lines wait in
        // `pending_logs` until the reader returns to the end, so this frame
        // does not lay out the unread tail or shift the lines on screen.
        if !runtime.verbose_unpinned {
            runtime.accept_followed_logs();
        }
        let scroll = egui::ScrollArea::vertical()
            .id_salt(("cloud-output", id, place))
            .max_height(inner)
            .min_scrolled_height(inner)
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| lines(ui, runtime));
        let max_offset = (scroll.content_size.y - scroll.inner_rect.height()).max(0.0);
        runtime.verbose_unpinned = scroll.state.offset.y + 1.0 < max_offset;
    });
    if let Some(failure) = failure {
        root_cause(ui, failure);
    }
}

fn lines(ui: &mut egui::Ui, runtime: &Runtime) {
    ui.spacing_mut().item_spacing.y = 3.0;
    if runtime.logs.is_empty() {
        ui.label(RichText::new("No output yet.").size(TEXT_SIZE).color(theme::FG_DIM()));
        return;
    }
    let mut previous: Option<Option<Stage>> = None;
    // Tools print blank separator lines; they only spread the log out.
    for line in runtime.logs.iter().filter(|line| !line.text.trim().is_empty()) {
        if previous != Some(line.stage) {
            if let Some(stage) = line.stage {
                stage_heading(ui, runtime, stage);
            }
            previous = Some(line.stage);
        }
        row(ui, line);
    }
}

fn stage_heading(ui: &mut egui::Ui, runtime: &Runtime, stage: Stage) {
    let color = stage_color(stage);
    let label = runtime.progress.stage_duration(stage).map_or_else(
        || stage.label().to_owned(),
        |elapsed| {
            format!(
                "{} · {}",
                stage.label(),
                horizon_core::cloud_runtime::progress::duration(elapsed)
            )
        },
    );
    ui.add_space(2.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), egui::Sense::hover());
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        Stroke::new(1.0, theme::alpha(color, 50)),
    );
    let galley = ui.painter().layout_no_wrap(label, FontId::proportional(12.0), color);
    let label_rect = egui::Rect::from_min_size(
        rect.left_top() + Vec2::new(52.0, (rect.height() - galley.size().y) / 2.0),
        galley.size() + Vec2::new(8.0, 0.0),
    );
    ui.painter().rect_filled(label_rect, 3, LOG_BG);
    ui.painter().galley(label_rect.min + Vec2::new(4.0, 0.0), galley, color);
}

fn row(ui: &mut egui::Ui, line: &LogLine) {
    let failure = diagnosis::is_failure(&line.text);
    let warning = line.text.trim_start().to_ascii_lowercase().starts_with("warning");
    let color = if failure {
        theme::PALETTE_RED()
    } else if warning {
        theme::PALETTE_YELLOW()
    } else {
        theme::FG_SOFT()
    };
    let background = ui.painter().add(egui::Shape::Noop);
    let response = ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let time = line.at.map_or_else(String::new, |at| {
            let seconds = at.as_secs();
            format!("{:>2}:{:02}", seconds / 60, seconds % 60)
        });
        ui.add_sized(
            [38.0, TEXT_SIZE + 3.0],
            egui::Label::new(
                RichText::new(time)
                    .monospace()
                    .size(TEXT_SIZE - 1.0)
                    .color(theme::FG_DIM()),
            ),
        );
        let (bar, _) = ui.allocate_exact_size(Vec2::new(2.0, TEXT_SIZE + 3.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(bar, 1, line.stage.map_or(theme::BORDER_SUBTLE(), stage_color));
        ui.add(egui::Label::new(RichText::new(&line.text).monospace().size(TEXT_SIZE).color(color)).wrap());
    });
    if failure {
        ui.painter().set(
            background,
            egui::Shape::rect_filled(
                response.response.rect.expand2(Vec2::new(4.0, 1.0)),
                3,
                theme::alpha(theme::PALETTE_RED(), 28),
            ),
        );
    }
}

/// The cause, what it means, and Horizon's own summary when the cause is a different line.
fn root_cause(ui: &mut egui::Ui, failure: &Failure) {
    egui::Frame::new()
        .fill(theme::blend(LOG_BG, theme::PALETTE_RED(), 0.16))
        .stroke(Stroke::new(1.0, theme::alpha(theme::PALETTE_RED(), 120)))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("Root cause")
                        .monospace()
                        .size(TEXT_SIZE)
                        .color(theme::PALETTE_RED()),
                );
                ui.label(
                    RichText::new(failure.headline())
                        .monospace()
                        .size(TEXT_SIZE)
                        .color(theme::PALETTE_RED()),
                );
            });
            let explanation = match (failure.meaning, &failure.cause) {
                (Some(meaning), _) => meaning.to_owned(),
                (None, Some(_)) => failure.summary.clone(),
                (None, None) => "Horizon found no failure line in the output above.".to_owned(),
            };
            ui.label(RichText::new(explanation).size(12.5).color(theme::FG_SOFT()));
        });
}
