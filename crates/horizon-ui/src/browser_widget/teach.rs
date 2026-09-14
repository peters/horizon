//! Teach-mode recording banner and final-outcome picker.

use egui::{RichText, TextWrapMode, Ui};
use horizon_core::browser::BrowserPanelState;

use crate::theme;

/// Draw the Teach banner when a session is active. Returns whether a control
/// was clicked this frame.
pub fn show(ui: &mut Ui, browser: &mut BrowserPanelState, interactive: bool) -> bool {
    if browser.teach().is_none() {
        return false;
    }
    let mut clicked = false;
    ui.scope(|ui| {
        ui.visuals_mut().override_text_color = Some(theme::PALETTE_CYAN());
        clicked |= controls(ui, browser, interactive);
        previews(ui, browser);
        if browser
            .teach()
            .is_some_and(horizon_core::browser::TeachMode::is_stopped)
        {
            clicked |= outcome_picker(ui, browser, interactive);
        }
        if let Some(error) = browser.teach().and_then(horizon_core::browser::TeachMode::last_error) {
            ui.label(RichText::new(error).size(10.5).color(theme::PALETTE_RED()));
        }
    });
    clicked
}

fn controls(ui: &mut Ui, browser: &mut BrowserPanelState, interactive: bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        let paused = browser.teach().is_some_and(horizon_core::browser::TeachMode::is_paused);
        let stopped = browser
            .teach()
            .is_some_and(horizon_core::browser::TeachMode::is_stopped);
        let status = if stopped {
            "Teach stopped"
        } else if paused {
            "Teach paused"
        } else {
            "Teaching"
        };
        ui.label(RichText::new(status).size(12.0).strong());
        let mut name = browser
            .teach()
            .map(|teach| teach.name().to_string())
            .unwrap_or_default();
        let name_edit = ui.add_enabled(
            interactive && !stopped,
            egui::TextEdit::singleline(&mut name)
                .desired_width(140.0)
                .hint_text("routine name"),
        );
        if name_edit.changed()
            && let Some(teach) = browser.teach_mut()
        {
            teach.set_name(&name);
        }
        clicked |= name_edit.clicked();
        if !stopped {
            let pause_label = if paused { "Resume" } else { "Pause" };
            if ui
                .add_enabled(interactive, egui::Button::new(RichText::new(pause_label).size(11.0)))
                .clicked()
            {
                if paused {
                    browser.resume_teach();
                } else {
                    browser.pause_teach();
                }
                clicked = true;
            }
            if ui
                .add_enabled(interactive, egui::Button::new(RichText::new("Stop").size(11.0)))
                .clicked()
            {
                browser.stop_teach();
                clicked = true;
            }
        }
        if ui
            .add_enabled(interactive, egui::Button::new(RichText::new("Discard").size(11.0)))
            .clicked()
        {
            let _ = browser.discard_teach();
            clicked = true;
        }
    });
    clicked
}

fn previews(ui: &mut Ui, browser: &BrowserPanelState) {
    let Some(teach) = browser.teach() else {
        return;
    };
    let previews = teach.action_previews();
    if previews.is_empty() {
        ui.label(
            RichText::new("No semantic actions yet")
                .size(11.0)
                .color(theme::FG_DIM()),
        );
        return;
    }
    let summary = previews.into_iter().rev().take(4).rev().collect::<Vec<_>>().join(" · ");
    ui.add(
        egui::Label::new(RichText::new(summary).size(11.0).color(theme::FG_SOFT())).wrap_mode(TextWrapMode::Truncate),
    );
}

fn outcome_picker(ui: &mut Ui, browser: &mut BrowserPanelState, interactive: bool) -> bool {
    let title = browser.title.clone();
    let Some(teach) = browser.teach_mut() else {
        return false;
    };
    let mut clicked = false;
    ui.horizontal(|ui| {
        let mut use_title = teach.use_title_outcome();
        if ui
            .add_enabled(
                interactive && !title.is_empty(),
                egui::Checkbox::new(&mut use_title, format!("Outcome: {title}")),
            )
            .changed()
        {
            teach.set_use_title_outcome(use_title);
            clicked = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Heading").size(11.0));
        let mut heading = teach.completion_heading().to_string();
        let edit = ui.add_enabled(
            interactive,
            egui::TextEdit::singleline(&mut heading)
                .desired_width(220.0)
                .hint_text("optional extra assertion"),
        );
        if edit.changed() {
            teach.set_completion_heading(&heading);
        }
        clicked |= edit.clicked();
    });
    clicked
}
