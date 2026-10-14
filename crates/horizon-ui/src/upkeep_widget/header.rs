//! The board's title, what runs where, and the actions of an Upkeep cloud's board: Run now,
//! Pause and Repositories.
use egui::{Align, Layout, RichText, Ui, Vec2};
use horizon_core::upkeep::{Request, Status};

use crate::theme;

/// What the actions can do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Actions {
    /// A run is in progress, or Run now was pressed and the status does not show it yet.
    pub(super) busy: bool,
}

/// The width the three buttons take side by side, with their spacing.
const ACTIONS_WIDTH: f32 = 92.0 + 104.0 + 78.0 + 2.0 * 8.0;
/// The least width the title and its summary keep beside the buttons.
const TITLE_MIN_WIDTH: f32 = 220.0;

/// Draws the header; with `actions`, also its buttons: beside the title when both fit, under
/// it on a narrow board, where they wrap rather than leave it. Returns what the person
/// pressed.
pub(super) fn show(ui: &mut Ui, status: &Status, actions: Option<Actions>) -> Option<Request> {
    let mut request = None;
    let beside = ui.available_width() >= TITLE_MIN_WIDTH + ACTIONS_WIDTH;
    ui.horizontal(|ui| {
        // A truncated label takes all the width it is given: beside the buttons, the title
        // gets only what they leave.
        let title_width = if beside && actions.is_some() {
            ui.available_width() - ACTIONS_WIDTH - ui.spacing().item_spacing.x
        } else {
            ui.available_width()
        };
        ui.vertical(|ui| {
            ui.set_max_width(title_width);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new("Upkeep").size(18.0).strong().color(theme::FG()));
            ui.add(egui::Label::new(RichText::new(summary(status)).size(12.0).color(theme::FG_SOFT())).truncate());
        });
        if beside && let Some(actions) = actions {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                request = buttons(ui, status, actions, true);
            });
        }
    });
    if !beside && let Some(actions) = actions {
        ui.horizontal_wrapped(|ui| request = buttons(ui, status, actions, false));
    }
    request
}

/// Run now, Repositories and Pause in that order, added from the right when `from_right`.
fn buttons(ui: &mut Ui, status: &Status, actions: Actions, from_right: bool) -> Option<Request> {
    let paused = status.schedule.paused;
    let mut request = None;
    let mut order = [Request::RunNow, Request::Repositories, Request::Pause(!paused)];
    if from_right {
        order.reverse();
    }
    for button in order {
        let clicked = match button {
            Request::RunNow => {
                let reason = if paused {
                    "Resume first: a paused Upkeep does not run."
                } else {
                    "A run is in progress or about to start. Only one runs at a time."
                };
                ui.add_enabled(
                    !actions.busy && !paused,
                    primary("Run now").min_size(Vec2::new(92.0, 30.0)),
                )
                .on_hover_text("Start a run now instead of at the next planned time.")
                .on_disabled_hover_text(reason)
                .clicked()
            }
            Request::Repositories => ui
                .add(chrome("Repositories").min_size(Vec2::new(104.0, 30.0)))
                .on_hover_text("Add or remove repositories. The next run uses the new list.")
                .clicked(),
            Request::Pause(_) => {
                let (label, hint) = if paused {
                    ("Resume", "Start the planned runs again.")
                } else {
                    ("Pause", "Stop the planned runs. A run in progress finishes.")
                };
                ui.add(chrome(label).min_size(Vec2::new(78.0, 30.0)))
                    .on_hover_text(hint)
                    .clicked()
            }
            Request::Open(_) => false,
        };
        if clicked {
            request = Some(button);
        }
    }
    request
}

fn primary(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(12.0).strong().color(theme::BG()))
        .fill(theme::ACCENT())
        .corner_radius(10)
}

fn chrome(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(12.0).color(theme::FG_SOFT()))
        .fill(theme::PANEL_BG_ALT())
        .stroke(egui::Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 210)))
        .corner_radius(10)
}

/// "Codex · every hour · Hetzner cx23, Helsinki · 16 repositories"
fn summary(status: &Status) -> String {
    let agent = match status.agent.as_str() {
        "" => "Agent".to_string(),
        "codex" => "Codex".to_string(),
        "claude" => "Claude Code".to_string(),
        other => other.to_string(),
    };
    let mut parts = vec![agent];
    // No cadence when the status names none; "Next run" then says "Not scheduled".
    match status.schedule.every_minutes {
        0 => {}
        60 => parts.push("every hour".to_string()),
        minutes if minutes % 60 == 0 => parts.push(format!("every {} hours", minutes / 60)),
        minutes => parts.push(format!("every {minutes} min")),
    }
    if !status.worker.name.is_empty() {
        let mut worker = status.worker.name.clone();
        if !status.worker.location.is_empty() {
            worker = format!("{worker}, {}", status.worker.location);
        }
        parts.push(worker);
    }
    let count = status.repositories.len();
    parts.push(format!(
        "{count} {}",
        if count == 1 { "repository" } else { "repositories" }
    ));
    parts.join("  ·  ")
}

#[cfg(test)]
mod tests {
    use egui::accesskit::Rect as Bounds;

    use super::*;
    use crate::test_egui::DiscardTextures as _;

    /// The bounds of the title and of each button the header drew in a column `width` wide.
    fn drawn(width: f32) -> Vec<(String, Bounds)> {
        let status: Status = serde_json::from_str(
            r#"{"agent":"codex","schedule":{"every_minutes":60},"worker":{"name":"Hetzner cx23","location":"Helsinki"},
                "repositories":["acme/api","acme/docs","acme/mobile","acme/web","acme/ops","acme/site"]}"#,
        )
        .unwrap();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let output = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 400.0))),
                    ..egui::RawInput::default()
                },
                |ui| {
                    ui.allocate_ui(egui::vec2(width, 300.0), |ui| {
                        ui.set_max_width(width);
                        let _ = show(ui, &status, Some(Actions { busy: false }));
                    });
                },
            )
            .discard_textures();
        let update = output.platform_output.accesskit_update.expect("accesskit tree");
        let mut found: Vec<(String, Bounds)> = Vec::new();
        for (_, node) in update.nodes {
            let (Some(name), Some(bounds)) = (node.label().or_else(|| node.value()), node.bounds()) else {
                continue;
            };
            // A label can name its text twice: once as a label, once as a value.
            // The summary goes by its first word.
            let name = if name.starts_with("Codex") { "Codex" } else { name };
            if ["Upkeep", "Codex", "Run now", "Repositories", "Pause"].contains(&name)
                && !found.iter().any(|(held, _)| held == name)
            {
                found.push((name.to_owned(), bounds));
            }
        }
        found
    }

    #[test]
    fn the_buttons_stay_inside_a_narrow_board_and_off_the_title() {
        for width in [240.0, 300.0, 480.0, 520.0, 600.0, 700.0, 900.0] {
            let drawn = drawn(width);
            assert_eq!(drawn.len(), 5, "{width} px wide: {drawn:?}");
            for (name, bounds) in &drawn {
                assert!(
                    bounds.x0 >= -0.5 && bounds.x1 <= f64::from(width) + 0.5,
                    "{width} px wide: {name} at {bounds:?}"
                );
            }
            let is_title = |name: &str| name == "Upkeep" || name == "Codex";
            for (title_name, title) in drawn.iter().filter(|(name, _)| is_title(name)) {
                for (name, bounds) in drawn.iter().filter(|(name, _)| !is_title(name)) {
                    let apart = bounds.x1 <= title.x0
                        || bounds.x0 >= title.x1
                        || bounds.y1 <= title.y0
                        || bounds.y0 >= title.y1;
                    assert!(apart, "{width} px wide: {name} covers {title_name}");
                }
            }
        }
    }
}
