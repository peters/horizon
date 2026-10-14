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

/// Draws the header; with `actions`, also its buttons. Returns what the person pressed.
pub(super) fn show(ui: &mut Ui, status: &Status, actions: Option<Actions>) -> Option<Request> {
    let mut request = None;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new("Upkeep").size(18.0).strong().color(theme::FG()));
            ui.add(egui::Label::new(RichText::new(summary(status)).size(12.0).color(theme::FG_SOFT())).truncate());
        });
        let Some(actions) = actions else { return };
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let paused = status.schedule.paused;
            let (pause, hint) = if paused {
                ("Resume", "Start the planned runs again.")
            } else {
                ("Pause", "Stop the planned runs. A run in progress finishes.")
            };
            if ui
                .add(chrome(pause).min_size(Vec2::new(78.0, 30.0)))
                .on_hover_text(hint)
                .clicked()
            {
                request = Some(Request::Pause(!paused));
            }
            if ui
                .add(chrome("Repositories").min_size(Vec2::new(104.0, 30.0)))
                .on_hover_text("Add or remove repositories. The next run uses the new list.")
                .clicked()
            {
                request = Some(Request::Repositories);
            }
            let reason = if paused {
                "Resume first: a paused Upkeep does not run."
            } else {
                "A run is in progress. Only one runs at a time."
            };
            let run = ui
                .add_enabled(
                    !actions.busy && !paused,
                    primary("Run now").min_size(Vec2::new(92.0, 30.0)),
                )
                .on_hover_text("Start a run now instead of at the next planned time.")
                .on_disabled_hover_text(reason);
            if run.clicked() {
                request = Some(Request::RunNow);
            }
        });
    });
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
