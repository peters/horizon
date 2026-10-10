//! A production cloud without panels: its steps and output fill the space the
//! panels will take, so progress needs no click to see.
use super::super::Runtime;
use super::status::Status;
use super::steps::{self, StepAction};
use crate::theme;
use egui::{RichText, Stroke, Vec2};
use horizon_core::cloud_panel::CloudLaunch;
use horizon_core::cloud_runtime::progress;

/// Below this width the steps stack above the output.
const SIDE_BY_SIDE: f32 = 760.0;
const STEPS_WIDTH: f32 = 360.0;
const GAP: f32 = 16.0;
/// Room under the output for a hint of up to two lines.
const HINT_HEIGHT: f32 = 44.0;

pub(super) fn show(
    ui: &mut egui::Ui,
    id: u32,
    size: Vec2,
    launch: &CloudLaunch,
    runtime: &mut Runtime,
    status: &Status,
) -> Option<StepAction> {
    ui.set_min_size(size);
    ui.set_max_size(size);
    let hint = hint_text(status, runtime.first_panel_due, super::deleting(runtime));
    let hint_height = if hint.is_some() { HINT_HEIGHT } else { 0.0 };
    if size.x >= SIDE_BY_SIDE {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            let action = ui
                .allocate_ui_with_layout(
                    Vec2::new(STEPS_WIDTH, size.y),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| steps_card(ui, id, launch, runtime, status, size.y),
                )
                .inner;
            ui.vertical(|ui| {
                output_column(ui, id, &launch.id, runtime, size.y - hint_height);
                show_hint(ui, hint, status);
            });
            action
        })
        .inner
    } else {
        let steps_height = (size.y * 0.55).max(250.0).min(size.y - 160.0);
        let action = steps_card(ui, id, launch, runtime, status, steps_height);
        ui.add_space(GAP);
        output_column(ui, id, &launch.id, runtime, size.y - steps_height - GAP - hint_height);
        show_hint(ui, hint, status);
        action
    }
}

fn steps_card(
    ui: &mut egui::Ui,
    id: u32,
    launch: &CloudLaunch,
    runtime: &Runtime,
    status: &Status,
    height: f32,
) -> Option<StepAction> {
    let frame = egui::Frame::new()
        .fill(theme::BG_ELEVATED())
        .stroke(Stroke::new(
            1.0,
            if status.failure.is_some() {
                theme::alpha(theme::PALETTE_RED(), 130)
            } else {
                theme::BORDER_SUBTLE()
            },
        ))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(18, 14));
    let inner = height - frame.total_margin().sum().y;
    frame
        .show(ui, |ui| {
            ui.set_min_height(inner);
            ui.set_max_height(inner);
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Steps").size(12.0).color(theme::FG_DIM()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(timing(runtime)).size(12.0).color(theme::FG_DIM()));
                });
            });
            ui.add_space(10.0);
            let footer = 52.0;
            // A solid bar takes room of its own, so an expanded cause cannot put it over the step times.
            ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
            let action = egui::ScrollArea::vertical()
                .id_salt("cloud-steps")
                .max_height((ui.available_height() - footer).max(60.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let action = steps::vertical(ui, id, runtime, status);
                    // Without panels the drawer offers no Overview; the ready timeline is here.
                    if status.tone == super::status::Tone::Ready {
                        ui.add_space(12.0);
                        super::timeline::show(ui, id, runtime);
                    }
                    action
                })
                .inner;
            let renew = ui
                .with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    let renew = super::github::renew_button(ui, &launch.id, runtime);
                    if let Some((line, connected)) = super::github::summary(runtime) {
                        let color = if connected {
                            theme::PALETTE_GREEN()
                        } else {
                            theme::FG_DIM()
                        };
                        ui.label(RichText::new(line).size(12.5).color(color));
                    }
                    ui.label(RichText::new(worker(runtime)).size(12.5).color(theme::FG_DIM()));
                    ui.label(
                        RichText::new(machine(launch, runtime))
                            .size(12.5)
                            .color(theme::FG_SOFT()),
                    );
                    ui.separator();
                    renew
                })
                .inner;
            if renew { Some(StepAction::Reconnect) } else { action }
        })
        .inner
}

/// "2m 05s · last successful 4m 12s", or how long the finished attempt took.
fn timing(runtime: &Runtime) -> String {
    let previous = runtime
        .state
        .as_ref()
        .and_then(|state| state.timeline.as_ref())
        .map(horizon_core::cloud_runtime::timeline::Timeline::total)
        .filter(|total| !total.is_zero());
    match (runtime.progress.elapsed(), previous) {
        (Some(now), Some(previous))
            if runtime.receiver.is_some() && runtime.stage != Some(super::super::Stage::Ready) =>
        {
            format!(
                "{} · last successful {}",
                progress::duration(now),
                progress::duration(previous)
            )
        }
        (Some(now), _) => progress::duration(now),
        (None, Some(previous)) => format!("Last successful {}", progress::duration(previous)),
        (None, None) => String::new(),
    }
}

fn machine(launch: &CloudLaunch, runtime: &Runtime) -> String {
    let profile = runtime.state.as_ref().map_or(&launch.profile, |state| &state.profile);
    let mut line = format!("{} vCPU · {} GB", profile.cpu, profile.memory_gb);
    if profile.gpu {
        line.push_str(" · GPU");
    }
    if let Some(place) = super::placement::short(launch, runtime.state.as_ref()) {
        line.push_str(" · ");
        line.push_str(&place);
    }
    line
}

fn worker(runtime: &Runtime) -> String {
    if runtime.state_unavailable {
        return "Worker: unknown (record unavailable)".to_owned();
    }
    runtime
        .state
        .as_ref()
        .and_then(|state| state.worker.as_ref())
        .map_or_else(
            || {
                // The record is written at Ready; a step past Provision has requested one.
                if super::cost::worker_requested(runtime) {
                    "Worker: requested · awaiting its details".to_owned()
                } else {
                    "Worker: not requested yet".to_owned()
                }
            },
            |worker| format!("Worker: {} (last observed)", worker.desired_status),
        )
}

fn output_column(ui: &mut egui::Ui, id: u32, cloud_id: &str, runtime: &mut Runtime, height: f32) {
    let bottom = ui.cursor().top() + height;
    if super::github::waiting(runtime) {
        super::github::prompt(ui, cloud_id, runtime);
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Output").size(12.0).color(theme::FG_DIM()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{} lines", runtime.logs.len() + runtime.pending_logs.len()))
                    .size(12.0)
                    .color(theme::FG_DIM()),
            );
        });
    });
    // Ends level with the steps card: the heading's own height comes off the log's.
    let room = bottom - ui.cursor().top();
    super::output::show(ui, id, "body", runtime, room);
}

/// What the empty body waits for, when the steps beside it do not already say so.
fn hint_text(status: &Status, starting: bool, deleting: bool) -> Option<&'static str> {
    use super::status::{Primary, Tone};
    if deleting {
        return Some(
            "Closing this cloud. Its worker and storage are being deleted, and the card closes once they are gone.",
        );
    }
    match (status.tone, status.primary) {
        (Tone::Ready, _) if starting => Some("Starting your first panel…"),
        (Tone::Ready, _) => {
            Some("No panels open. Ctrl-double-click inside this cloud to add an agent, browser or terminal.")
        }
        (_, Some(Primary::Resume)) => Some("Resume the worker to open panels here."),
        // A failure already shows its cause and Retry; a running step shows its progress.
        _ => None,
    }
}

/// The hint centered in the room left under the output, so it never reaches the frame.
fn show_hint(ui: &mut egui::Ui, hint: Option<&str>, status: &Status) {
    let Some(text) = hint else { return };
    let ready = status.tone == super::status::Tone::Ready;
    let room = Vec2::new(ui.available_width(), ui.available_height().clamp(0.0, HINT_HEIGHT));
    ui.allocate_ui_with_layout(
        room,
        egui::Layout::centered_and_justified(egui::Direction::TopDown),
        |ui| {
            ui.label(
                RichText::new(text)
                    .size(13.0)
                    .color(if ready { theme::FG_SOFT() } else { theme::FG_DIM() }),
            );
        },
    );
}

#[cfg(test)]
mod tests {
    use super::super::status;
    use super::*;
    use crate::test_egui::DiscardTextures;

    fn launch() -> CloudLaunch {
        let mut config = horizon_core::cloud_panel::CloudConfig::parse(
            "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 8\n    memory_gb: 32\n",
        )
        .unwrap();
        CloudLaunch {
            deployment_started: false,
            id: "body".into(),
            revision: "a".repeat(40),
            profile_name: "dev".into(),
            profile: config.profiles.remove("dev").unwrap(),
            placement: horizon_core::cloud_panel::Placement::default(),
        }
    }

    /// Painted texts and where they are, for a body of `size`.
    fn texts(size: Vec2) -> Vec<(String, egui::Rect)> {
        let mut runtime = Runtime::default();
        let status = status::of(&runtime, status::Occupancy::default(), std::time::SystemTime::now());
        let launch = launch();
        egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                let _ = show(ui, 1, size, &launch, &mut runtime, &status);
            })
            .discard_textures()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some((text.galley.text().to_owned(), text.visual_bounding_rect())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_steps_stack_top_to_bottom_beside_or_above_the_output() {
        for size in [Vec2::new(1370.0, 620.0), Vec2::new(520.0, 600.0)] {
            let shown = texts(size);
            let find = |label: &str| {
                shown
                    .iter()
                    .find(|(text, _)| text == label)
                    .map_or_else(|| panic!("{label} is painted at {size:?}"), |(_, rect)| *rect)
            };
            let tops: Vec<f32> = crate::app::cloud_panel::production::Stage::ALL
                .iter()
                .map(|stage| find(stage.label()).top())
                .collect();
            assert!(
                tops.windows(2).all(|pair| pair[0] + 20.0 <= pair[1]),
                "{tops:?} at {size:?}"
            );
            let lefts: Vec<f32> = crate::app::cloud_panel::production::Stage::ALL
                .iter()
                .map(|stage| find(stage.label()).left())
                .collect();
            assert!(
                lefts.iter().all(|left| (left - lefts[0]).abs() < 3.0),
                "one column at {size:?}: {lefts:?}"
            );
            let machine = find("8 vCPU · 32 GB");
            assert!(machine.height() < 24.0, "the footer stays on one line at {size:?}");
            let output = find("No output yet.");
            if size.x >= SIDE_BY_SIDE {
                assert!(
                    output.left() > find("Validate").right() + 100.0,
                    "output beside the steps"
                );
            } else {
                assert!(output.top() > find("Ready").bottom(), "output under the steps");
            }
        }
    }

    #[test]
    fn the_hint_shows_only_what_the_steps_do_not_say_and_stays_inside_the_body() {
        use super::super::super::Stage;
        let status_of =
            |runtime: &Runtime| status::of(runtime, status::Occupancy::default(), std::time::SystemTime::now());
        let building = Runtime {
            stage: Some(Stage::Build),
            ..Runtime::default()
        };
        assert_eq!(
            hint_text(&status_of(&building), false, false),
            None,
            "a running step speaks for itself"
        );
        assert_eq!(hint_text(&status_of(&Runtime::default()), false, false), None);
        assert!(hint_text(&status_of(&building), false, true).is_some_and(|hint| hint.starts_with("Closing")));
        let mut state = super::super::super::rebuild::tests::deployment(
            std::path::Path::new("/synthetic"),
            true,
            super::super::super::rebuild::tests::Phase::None,
        );
        state.stage = Stage::Stopped;
        let mut stopped = Runtime {
            stage: Some(Stage::Stopped),
            state: Some(state),
            ..Runtime::default()
        };
        let status = status_of(&stopped);
        let hint = hint_text(&status, false, false).expect("a stopped cloud says to resume it");
        let launch = launch();
        for size in [Vec2::new(1370.0, 620.0), Vec2::new(520.0, 600.0)] {
            let shapes = egui::Context::default()
                .run_ui(egui::RawInput::default(), |ui| {
                    let _ = show(ui, 1, size, &launch, &mut stopped, &status);
                })
                .discard_textures()
                .shapes;
            let painted = shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == hint => Some(text.visual_bounding_rect()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("the hint is painted at {size:?}"));
            assert!(painted.bottom() <= size.y + 0.5, "{painted:?} below {size:?}");
        }
    }

    #[test]
    fn a_sent_worker_request_is_not_called_unrequested() {
        use super::super::super::Stage;
        let mut runtime = Runtime::default();
        assert_eq!(worker(&runtime), "Worker: not requested yet");
        // Past Provision the worker was requested, though the record is written at Ready.
        for stage in [Stage::Provision, Stage::Readiness, Stage::Worktrees, Stage::Sessions] {
            let deploying = Runtime {
                stage: Some(stage),
                ..Runtime::default()
            };
            assert_eq!(
                worker(&deploying),
                "Worker: requested · awaiting its details",
                "{stage:?}"
            );
        }
        runtime.state = Some(
            serde_json::from_value(serde_json::json!({
                "version":1,"cloud_id":"requested","repository":"/synthetic","revision":"a",
                "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
                "stage":"Provision","operation":{"state":"requested"},"spec":null,"sessions":[],"worker":null
            }))
            .unwrap(),
        );
        assert_eq!(worker(&runtime), "Worker: requested · awaiting its details");
        runtime.state_unavailable = true;
        assert_eq!(worker(&runtime), "Worker: unknown (record unavailable)");
    }
}
