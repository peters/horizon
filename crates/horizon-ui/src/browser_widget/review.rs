//! Compiler reviewer for a stopped Teach recording.

use egui::{RichText, TextWrapMode, Ui};
use horizon_core::browser::BrowserPanelState;

use crate::theme;

/// Draw compiled plan rows and the explicit save control.
pub fn show(ui: &mut Ui, browser: &mut BrowserPanelState, interactive: bool) -> bool {
    let stopped = browser
        .teach()
        .is_some_and(horizon_core::browser::TeachMode::is_stopped);
    if !stopped {
        return false;
    }
    let title = browser.title.clone();
    let mut clicked = false;
    ui.separator();
    ui.label(RichText::new("Review plan").size(12.0).strong());
    match browser.teach().map(|teach| teach.compile_review(&title)) {
        Some(Ok(rows)) if rows.is_empty() => {
            ui.label(RichText::new("No compiled steps").size(11.0).color(theme::FG_DIM()));
        }
        Some(Ok(rows)) => {
            egui::ScrollArea::vertical()
                .max_height(120.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for row in rows {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "{action} {target}\n{mutation} · {resume} · {mcp}",
                                    action = row.action,
                                    target = row.target,
                                    mutation = row.mutation,
                                    resume = row.resume,
                                    mcp = row.mcp
                                ))
                                .size(11.0)
                                .color(theme::FG_SOFT()),
                            )
                            .wrap_mode(TextWrapMode::Wrap),
                        );
                    }
                });
        }
        Some(Err(error)) => {
            ui.label(RichText::new(error.to_string()).size(10.5).color(theme::PALETTE_RED()));
        }
        None => {}
    }
    ui.horizontal(|ui| {
        let mut reviewed = browser
            .teach()
            .is_some_and(horizon_core::browser::TeachMode::identities_reviewed);
        if ui
            .add_enabled(interactive, egui::Checkbox::new(&mut reviewed, "Identities reviewed"))
            .changed()
            && let Some(teach) = browser.teach_mut()
        {
            teach.set_identities_reviewed(reviewed);
            clicked = true;
        }
        let can_save = interactive && reviewed;
        if ui
            .add_enabled(can_save, egui::Button::new(RichText::new("Save routine").size(11.0)))
            .clicked()
        {
            if let Some(teach) = browser.teach_mut()
                && let Err(error) = teach.save_reviewed(&title)
            {
                tracing::warn!(target: "browser", "teach save failed: {error}");
            }
            clicked = true;
        }
    });
    clicked
}
