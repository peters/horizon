use egui::Context;

use crate::app::StartupBootstrapFailure;
use crate::{loading_spinner, theme};

use super::StartupBootstrapFailureAction;

pub(in crate::app) fn render_loading_view(
    ctx: &Context,
    failure: Option<&StartupBootstrapFailure>,
) -> Option<StartupBootstrapFailureAction> {
    let mut action = None;
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(theme::BG()))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.28);
                ui.label(egui::RichText::new("Horizon").size(26.0).strong().color(theme::FG()));
                ui.add_space(16.0);
            });
            if let Some(failure) = failure {
                let (failure_heading, failure_detail) = match failure {
                    StartupBootstrapFailure::WorkerDisconnected => {
                        ("Saved-session validation stopped unexpectedly.", None)
                    }
                    StartupBootstrapFailure::RecoverySaveFailed(error) => {
                        ("The repaired session could not be saved.", Some(error.as_str()))
                    }
                };
                ui.label(egui::RichText::new(failure_heading).color(theme::PALETTE_RED()));
                if let Some(failure_detail) = failure_detail {
                    ui.label(egui::RichText::new(failure_detail).small().color(theme::FG_DIM()));
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if matches!(failure, StartupBootstrapFailure::WorkerDisconnected) && ui.button("Retry").clicked() {
                        action = Some(StartupBootstrapFailureAction::Retry);
                    }
                    let recovery_label = match failure {
                        StartupBootstrapFailure::WorkerDisconnected => Some("Open safely without exact resumes"),
                        StartupBootstrapFailure::RecoverySaveFailed(_) => Some("Retry repaired save and open"),
                    };
                    if recovery_label.is_some_and(|label| ui.button(label).clicked()) {
                        action = Some(StartupBootstrapFailureAction::ContinueWithoutExactResumes);
                    }
                });
            } else {
                loading_spinner::show(
                    ui,
                    egui::Id::new("startup_loading_spinner"),
                    Some("Resolving saved sessions\u{2026}"),
                );
            }
        });
    action
}
