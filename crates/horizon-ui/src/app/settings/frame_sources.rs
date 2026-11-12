//! The frame source programs of the **Remote** tab.
//!
//! A native Device panel can show the pictures of a program of this computer. The
//! tab holds the list of those programs: a name for the person and for an agent, and
//! the one command line that starts it. Horizon holds no codec of such a program, so
//! this list is the whole of what it knows about one.
//!
//! Every row is two lines: the name with the button that removes it, and the command
//! line under it. Two fields of a three-field row do not fit the 444 points of the
//! panel of the smallest window that this build opens.

use egui::Ui;
use horizon_core::{Config, MAX_NAME_CHARS, MAX_SOURCES};

use crate::theme;

/// What one pass of the tab changed.
#[derive(Clone, Copy, Default)]
pub(super) struct FrameSourcesOutcome {
    /// The list of the settings changed.
    pub(super) changed: bool,
}

pub(super) fn render(ui: &mut Ui, config: &mut Config) -> FrameSourcesOutcome {
    let mut outcome = FrameSourcesOutcome::default();
    super::section_heading(ui, "Frame source programs");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "A native Device panel can show the pictures of one program of this computer, which gives them \
             itself: Horizon holds no screen capture and no codec of such a program. The command line starts \
             the program when a viewer opens it, and {target} is the text that an agent may put there.",
        );
        ui.add_space(8.0);
        let mut remove = None;
        for (index, source) in config.frame_sources.sources.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label("Name");
                let name = ui.add(
                    egui::TextEdit::singleline(&mut source.name)
                        .desired_width(150.0)
                        .hint_text("a name of your own"),
                );
                if name.changed() {
                    outcome.changed = true;
                }
                ui.add_space(4.0);
                if ui.button("Remove").clicked() {
                    remove = Some(index);
                }
            });
            let command = ui.add(
                egui::TextEdit::singleline(&mut source.command)
                    .desired_width(ui.available_width())
                    .hint_text("the program and its arguments"),
            );
            if command.changed() {
                outcome.changed = true;
            }
            ui.add_space(6.0);
        }
        if let Some(index) = remove {
            config.frame_sources.sources.remove(index);
            outcome.changed = true;
        }
        let full = config.frame_sources.sources.len() >= MAX_SOURCES;
        if ui
            .add_enabled(!full, egui::Button::new("Add a program"))
            .on_disabled_hover_text(format!(
                "This Horizon keeps at most {MAX_SOURCES} frame source programs"
            ))
            .clicked()
        {
            config.frame_sources.sources.push(horizon_core::FrameSource::default());
            outcome.changed = true;
        }
        if let Err(error) = config.frame_sources.validate() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(error).size(11.0).color(theme::FG_DIM()));
        }
        ui.add_space(2.0);
        super::dim_label(
            ui,
            &format!("A name is 1 to {MAX_NAME_CHARS} characters of A-Z, a-z, 0-9, '-' and '_'."),
        );
    });
    outcome
}
