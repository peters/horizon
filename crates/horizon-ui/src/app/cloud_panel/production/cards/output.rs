//! A cloud operation's output as a first-class view: grouped by step, timed, with
//! failures highlighted and the decisive line pinned below the log.
use super::super::{LineKind, LogLine, Runtime, Stage};
use super::status::Failure;
use super::strip::stage_color;
use crate::theme;
use egui::{FontId, RichText, Stroke, Vec2};

const TEXT_SIZE: f32 = 13.0;
/// The pinned root cause keeps this height; a longer cause scrolls inside it, so the
/// view never grows past the height it was given.
const ROOT_CAUSE_HEIGHT: f32 = 72.0;

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
    let pinned = failure.map_or(0.0, |_| ROOT_CAUSE_HEIGHT + 6.0);
    let frame = egui::Frame::new()
        .fill(theme::BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 10));
    let inner = (height - pinned - frame.total_margin().sum().y).max(60.0);
    // Another view scrolled up holds new lines aside; this one, still at the end, shows them.
    let follows = runtime.unpinned_last & view_bit(place) == 0;
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
            .show(ui, |ui| lines(ui, runtime, follows));
        let max_offset = (scroll.content_size.y - scroll.inner_rect.height()).max(0.0);
        if scroll.state.offset.y + 1.0 < max_offset {
            runtime.unpinned_views |= view_bit(place);
            runtime.verbose_unpinned = true;
        }
    });
    if let Some(failure) = failure {
        root_cause(ui, failure);
    }
}

/// Each place a log is shown keeps its own follow state.
fn view_bit(place: &str) -> u8 {
    match place {
        "body" => 1,
        "drawer" => 2,
        _ => 4,
    }
}

/// Called once per cloud per frame before its output views are drawn: lines are held
/// aside only while a view shown last frame was scrolled up.
pub(super) fn begin_frame(runtime: &mut Runtime) {
    runtime.unpinned_last = std::mem::take(&mut runtime.unpinned_views);
    runtime.verbose_unpinned = runtime.unpinned_last != 0;
    if !runtime.verbose_unpinned {
        // Held lines join now, before newer ones, so the output stays in order.
        runtime.accept_followed_logs();
    }
}

/// The lines a view shows: a scrolled-up view the still list, a following view the
/// newest follow-length tail with held lines included.
fn shown(runtime: &Runtime, follows: bool) -> Vec<&LogLine> {
    if !follows {
        return runtime.logs.iter().collect();
    }
    let total = runtime.logs.len() + runtime.pending_logs.len();
    runtime
        .logs
        .iter()
        .chain(&runtime.pending_logs)
        .skip(total.saturating_sub(Runtime::FOLLOW_LOG_LINES))
        .collect()
}

fn lines(ui: &mut egui::Ui, runtime: &Runtime, follows: bool) {
    ui.spacing_mut().item_spacing.y = 3.0;
    let shown = shown(runtime, follows);
    if shown.is_empty() {
        ui.label(RichText::new("No output yet.").size(TEXT_SIZE).color(theme::FG_DIM()));
        return;
    }
    // A heading per step of each attempt: a retry of the same step starts its own.
    let mut previous: Option<(Option<Stage>, u64, usize)> = None;
    // Tools print blank separator lines; they only spread the log out.
    for line in shown.into_iter().filter(|line| !line.text.trim().is_empty()) {
        let group = (line.stage, line.attempt, line.visit);
        if previous != Some(group) {
            if let Some(stage) = line.stage {
                stage_heading(ui, runtime, stage, line.attempt, line.visit);
            } else if previous.is_some() {
                // Notes after a step's lines are not that step's output.
                note_heading(ui);
            }
            previous = Some(group);
        }
        row(ui, line);
    }
}

fn note_heading(ui: &mut egui::Ui) {
    ui.add_space(2.0);
    ui.label(RichText::new("Notes").size(12.0).color(theme::FG_DIM()));
}

/// Only the current attempt's steps are timed from the timeline, each visit with its own
/// time; an earlier attempt's is labelled as such rather than given the current one's.
fn stage_heading(ui: &mut egui::Ui, runtime: &Runtime, stage: Stage, attempt: u64, visit: usize) {
    let color = stage_color(stage);
    let current = attempt == runtime.progress.attempt();
    let duration = runtime.progress.visit_duration(stage, visit).filter(|_| current);
    let label = duration.map_or_else(
        || {
            if current {
                stage.label().to_owned()
            } else {
                format!("{} · earlier attempt", stage.label())
            }
        },
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
    ui.painter().rect_filled(label_rect, 3, theme::BG());
    ui.painter().galley(label_rect.min + Vec2::new(4.0, 0.0), galley, color);
}

fn row(ui: &mut egui::Ui, line: &LogLine) {
    let failure = line.kind == LineKind::Failure;
    let warning = line.kind == LineKind::Warning;
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
        .fill(theme::blend(theme::BG(), theme::PALETTE_RED(), 0.16))
        .stroke(Stroke::new(1.0, theme::alpha(theme::PALETTE_RED(), 120)))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let inner = ROOT_CAUSE_HEIGHT - 16.0;
            ui.set_min_height(inner);
            ui.set_max_height(inner);
            egui::ScrollArea::vertical()
                .id_salt("cloud-root-cause")
                .max_height(inner)
                .auto_shrink([false, false])
                .show(ui, |ui| root_cause_text(ui, failure));
        });
}

fn root_cause_text(ui: &mut egui::Ui, failure: &Failure) {
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
}

#[cfg(test)]
mod tests {
    use super::super::super::Stage;
    use super::*;
    use crate::test_egui::DiscardTextures;

    #[test]
    fn a_long_root_cause_stays_inside_the_height_it_was_given() {
        let mut runtime = Runtime::default();
        for index in 0..40 {
            runtime
                .logs
                .push_back(LogLine::new(format!("line {index}"), Some(Stage::Push), None));
        }
        let failure = Failure {
            summary: "Uploading image failed; inspect deployment output".into(),
            cause: Some("error from registry: ".to_owned() + &"denied because of a very long reason ".repeat(20)),
            meaning: Some(
                "The registry refused the request. Its saved credentials have expired or lack access to this image.",
            ),
        };
        let mut used = 0.0;
        let _ = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(420.0);
                let top = ui.cursor().top();
                show(ui, 1, "test", &mut runtime, 300.0, Some(&failure));
                used = ui.cursor().top() - top;
            })
            .discard_textures();
        assert!(
            used <= 300.0 + 12.0,
            "the log and its pinned cause fit the given height: {used}"
        );
    }

    fn headings(runtime: &mut Runtime) -> Vec<String> {
        headings_of(runtime, "Push image")
    }

    fn headings_of(runtime: &mut Runtime, step: &str) -> Vec<String> {
        let mut shown = Vec::new();
        let output = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                show(ui, 1, "test", runtime, 400.0, None);
            })
            .discard_textures();
        for shape in &output.shapes {
            if let egui::Shape::Text(text) = &shape.shape
                && text.galley.text().starts_with(step)
            {
                shown.push(text.galley.text().to_owned());
            }
        }
        shown
    }

    #[test]
    fn a_retried_step_gets_its_own_heading_and_the_old_one_no_current_time() {
        let mut runtime = Runtime {
            stage: Some(Stage::Push),
            ..Runtime::default()
        };
        runtime.progress.stage(Stage::Push, std::time::Instant::now());
        runtime.push_log("error from registry: denied".into());
        runtime.progress.reset();
        runtime.progress.stage(Stage::Push, std::time::Instant::now());
        runtime.push_log("docker push registry.example/worker:tag".into());
        let shown = headings(&mut runtime);
        assert_eq!(shown.len(), 2, "{shown:?}");
        assert_eq!(shown[0], "Push image · earlier attempt");
        assert!(shown[1].starts_with("Push image · 0m"), "{shown:?}");
    }

    #[test]
    fn a_step_revisited_in_one_attempt_keeps_each_visits_own_time() {
        use std::time::{Duration, Instant};
        let start = Instant::now().checked_sub(Duration::from_secs(300)).unwrap();
        let mut runtime = Runtime::default();
        let enter = |runtime: &mut Runtime, stage: Stage, at: Duration, text: &str| {
            runtime.stage = Some(stage);
            runtime.progress.stage(stage, start + at);
            runtime.push_log(text.into());
        };
        enter(&mut runtime, Stage::Validate, Duration::ZERO, "checking the profile");
        enter(&mut runtime, Stage::Build, Duration::from_secs(5), "#1 building");
        enter(
            &mut runtime,
            Stage::Validate,
            Duration::from_secs(125),
            "checking the image contract",
        );
        runtime.progress.finish(start + Duration::from_secs(128));
        let shown = headings_of(&mut runtime, "Validate");
        assert_eq!(
            shown,
            ["Validate · 0m 05s", "Validate · 0m 03s"],
            "each visit keeps its own time"
        );
    }

    #[test]
    fn a_view_still_following_shows_lines_another_scrolled_up_view_holds() {
        let mut runtime = Runtime::default();
        runtime.push_log("first".into());
        runtime.push_log("5f70bf18a086: Pushing [=>   ]".into());
        // The drawer's reader scrolled up last frame; the body is still at the end.
        runtime.unpinned_views = view_bit("drawer");
        begin_frame(&mut runtime);
        runtime.push_log("second".into());
        runtime.push_log("5f70bf18a086: Pushed".into());
        let text = |follows| -> Vec<String> {
            shown(&runtime, follows)
                .into_iter()
                .map(|line| line.text.clone())
                .collect()
        };
        let body = runtime.unpinned_last & view_bit("body") == 0;
        let drawer = runtime.unpinned_last & view_bit("drawer") == 0;
        assert!(body && !drawer);
        assert_eq!(
            text(body),
            ["first", "5f70bf18a086: Pushed", "second"],
            "the following view is current"
        );
        assert_eq!(
            text(drawer),
            ["first", "5f70bf18a086: Pushed"],
            "the scrolled-up view keeps its lines where they are; a layer updates in place"
        );
    }

    #[test]
    fn a_note_after_a_step_gets_its_own_heading_instead_of_the_steps() {
        let mut runtime = Runtime {
            stage: Some(Stage::Sessions),
            ..Runtime::default()
        };
        runtime.progress.stage(Stage::Sessions, std::time::Instant::now());
        runtime.push_log("session started".into());
        runtime.push_note("Idle watch: stopped by claude".into());
        let shown: Vec<String> = {
            let output = egui::Context::default()
                .run_ui(egui::RawInput::default(), |ui| {
                    show(ui, 1, "test", &mut runtime, 400.0, None);
                })
                .discard_textures();
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect()
        };
        let heading = shown.iter().position(|text| text == "Notes").expect("a Notes heading");
        let note = shown.iter().position(|text| text.contains("Idle watch")).unwrap();
        let step = shown.iter().position(|text| text == "session started").unwrap();
        assert!(step < heading && heading < note, "{shown:?}");
    }
}
