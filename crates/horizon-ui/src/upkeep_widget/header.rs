//! The board's title and what runs where.
use egui::{RichText, Ui};
use horizon_core::upkeep::Status;

use crate::theme;

pub(super) fn show(ui: &mut Ui, status: &Status) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new("Upkeep").size(18.0).strong().color(theme::FG()));
            ui.add(egui::Label::new(RichText::new(summary(status)).size(12.0).color(theme::FG_SOFT())).truncate());
        });
    });
}

/// "Codex · every hour · Hetzner cx23, Helsinki · 16 repositories"
fn summary(status: &Status) -> String {
    let agent = match status.agent.as_str() {
        "" => "Agent".to_string(),
        "codex" => "Codex".to_string(),
        "claude" => "Claude Code".to_string(),
        other => other.to_string(),
    };
    let every = match status.schedule.every_minutes {
        0 | 60 => "every hour".to_string(),
        minutes if minutes % 60 == 0 => format!("every {} hours", minutes / 60),
        minutes => format!("every {minutes} min"),
    };
    let mut parts = vec![agent, every];
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
