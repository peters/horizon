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
    let hint_height = 26.0;
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
                output_column(ui, id, runtime, status, size.y - hint_height);
                hint(ui, status);
            });
            action
        })
        .inner
    } else {
        let steps_height = (size.y * 0.55).max(250.0).min(size.y - 160.0);
        let action = steps_card(ui, id, launch, runtime, status, steps_height);
        ui.add_space(GAP);
        output_column(ui, id, runtime, status, size.y - steps_height - GAP - hint_height);
        hint(ui, status);
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
            let action = egui::ScrollArea::vertical()
                .id_salt("cloud-steps")
                .max_height((ui.available_height() - footer).max(60.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let action = steps::vertical(ui, runtime, status);
                    // Without panels the drawer offers no Overview; the ready timeline is here.
                    if status.tone == super::status::Tone::Ready {
                        ui.add_space(12.0);
                        super::timeline::show(ui, id, runtime);
                    }
                    action
                })
                .inner;
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.label(RichText::new(worker(runtime)).size(12.5).color(theme::FG_DIM()));
                ui.label(
                    RichText::new(machine(launch, runtime))
                        .size(12.5)
                        .color(theme::FG_SOFT()),
                );
                ui.separator();
            });
            action
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
            || "Worker: not requested yet".to_owned(),
            |worker| format!("Worker: {} (last observed)", worker.desired_status),
        )
}

fn output_column(ui: &mut egui::Ui, id: u32, runtime: &mut Runtime, status: &Status, height: f32) {
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
    super::output::show(ui, id, "body", runtime, height - 24.0, status.failure.as_ref());
}

/// What the empty body waits for, in the cloud's own terms.
fn hint_text(status: &Status) -> &'static str {
    use super::status::{Primary, Tone};
    match (status.tone, status.primary) {
        (Tone::Ready, _) => {
            "Ready for panels. Ctrl-double-click inside this cloud to add an agent, browser or terminal."
        }
        (_, Some(Primary::Resume)) => "Resume the worker to open panels here.",
        (Tone::Failed, Some(Primary::Retry | Primary::Reconnect)) => {
            "Fix the cause, then retry. Panels open here once the cloud is ready."
        }
        _ => "Panels open here once the cloud is ready.",
    }
}

fn hint(ui: &mut egui::Ui, status: &Status) {
    let ready = status.tone == super::status::Tone::Ready;
    ui.vertical_centered(|ui| {
        ui.add_space(6.0);
        ui.label(RichText::new(hint_text(status)).size(13.0).color(if ready {
            theme::FG_SOFT()
        } else {
            theme::FG_DIM()
        }));
    });
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
                    .map(|(_, rect)| *rect)
                    .unwrap_or_else(|| panic!("{label} is painted at {size:?}"))
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
}
