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
                let (failure_heading, failure_detail, recovery_explanation) = match failure {
                    StartupBootstrapFailure::ExactValidationFailed {
                        message,
                        all_exact_session_ids: false,
                        ..
                    } => (
                        "Some saved exact resumes could not be verified.",
                        Some(message.as_str()),
                        Some(
                            "Opening without them removes only those bindings from this Horizon session. Provider conversations are not deleted.",
                        ),
                    ),
                    StartupBootstrapFailure::ExactValidationFailed {
                        message,
                        all_exact_session_ids: true,
                        ..
                    } => (
                        "Saved-session validation could not determine a safe scope.",
                        Some(message.as_str()),
                        Some(
                            "Opening without saved resumes removes all exact bindings from this Horizon session. Provider conversations are not deleted.",
                        ),
                    ),
                    StartupBootstrapFailure::WorkerDisconnected => {
                        (
                            "Saved-session validation stopped unexpectedly.",
                            None,
                            Some(
                                "Validation scope is unknown. Opening without saved resumes removes all exact bindings from this Horizon session. Provider conversations are not deleted.",
                            ),
                        )
                    }
                    StartupBootstrapFailure::RecoverySaveFailed { message, .. } => (
                        "The repaired session could not be saved.",
                        Some(message.as_str()),
                        Some(
                            "Retry the save now, or open the repaired state without waiting. Normal autosave may persist it later.",
                        ),
                    ),
                };
                ui.label(egui::RichText::new(failure_heading).color(theme::PALETTE_RED()));
                if let Some(failure_detail) = failure_detail {
                    ui.label(egui::RichText::new(failure_detail).small().color(theme::FG_DIM()));
                }
                if let Some(recovery_explanation) = recovery_explanation {
                    ui.label(egui::RichText::new(recovery_explanation).small().color(theme::FG_DIM()));
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Retry").clicked() {
                        action = Some(StartupBootstrapFailureAction::Retry);
                    }
                    let recovery_label = match failure {
                        StartupBootstrapFailure::ExactValidationFailed { .. } => {
                            Some("Open without unverified resumes")
                        }
                        StartupBootstrapFailure::WorkerDisconnected => Some("Open without saved exact resumes"),
                        StartupBootstrapFailure::RecoverySaveFailed { .. } => Some("Open repaired session"),
                    };
                    if recovery_label.is_some_and(|label| ui.button(label).clicked()) {
                        action = Some(if matches!(failure, StartupBootstrapFailure::RecoverySaveFailed { .. }) {
                            StartupBootstrapFailureAction::OpenWithoutSaving
                        } else {
                            StartupBootstrapFailureAction::ContinueWithoutExactResumes
                        });
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
