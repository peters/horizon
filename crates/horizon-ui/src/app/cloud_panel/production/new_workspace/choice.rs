//! The choice the person keeps for a repository in New cloud from New workspace: Cloud or
//! This PC. It is kept when the cloud starts or the repository opens on This PC, and New
//! cloud then starts from it for that repository.
use super::Intent;
use crate::theme;
use egui::{RichText, Ui};
use horizon_core::cloud_panel::WorkspacePlacement;

/// The row under the notes: the kept choice with Forget, or the box that keeps the next
/// choice and the way to This PC. `asked` says that the repository asks for This PC, so the
/// box below already offers it. Returns the kept choice that is still in force.
pub(super) fn row(
    ui: &mut Ui,
    intent: &mut Intent,
    repository: &str,
    asked: bool,
    chosen: &mut bool,
) -> Option<WorkspacePlacement> {
    let mut kept = intent.choices.get(repository);
    ui.horizontal_wrapped(|ui| match kept {
        Some(placement) => {
            let place = if placement == WorkspacePlacement::Local {
                "This PC"
            } else {
                "Cloud"
            };
            ui.label(
                RichText::new(format!("You keep {place} for this repository."))
                    .size(12.5)
                    .color(theme::FG_DIM()),
            );
            if ui.link("Forget").clicked() {
                intent.error = match intent.root.clone() {
                    Some(root) => intent
                        .choices
                        .set(&root, repository, None)
                        .err()
                        .map(|error| error.to_string()),
                    None => Some("Horizon has no cloud settings".to_owned()),
                };
                if intent.error.is_none() {
                    kept = None;
                }
            }
        }
        None => {
            ui.checkbox(&mut intent.keep, "Keep my choice for this repository");
            if !asked && ui.link("Open on This PC instead").clicked() {
                *chosen = true;
            }
        }
    });
    if let Some(error) = &intent.error {
        ui.colored_label(theme::PALETTE_RED(), error);
    }
    kept
}
