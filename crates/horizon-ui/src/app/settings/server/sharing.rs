//! The part of the Server tab about the terminals of this computer: the name
//! that the other computers of the person read for it, and the two switches of
//! what those computers may do. The section `server` stays with this computer,
//! so no other computer changes them.

use egui::Ui;
use horizon_core::{SHARED_TERMINALS_NOTICE, ServerConfig};

use super::TEXT_SIZE;
use crate::theme;

/// What the two switches say.
pub(super) const SEE: &str = "The other computers see the terminals of this computer";
pub(super) const TYPE: &str = "The other computers type in them";

/// Draws the name field and the two switches. Answers whether one changed.
pub(super) fn render(ui: &mut Ui, server: &mut ServerConfig) -> bool {
    super::super::section_heading(ui, "Shared terminals");
    super::super::section_card(ui, |ui| {
        super::super::dim_label(ui, SHARED_TERMINALS_NOTICE);
        ui.add_space(8.0);
        let mut changed = false;
        ui.horizontal(|ui| {
            let label = egui::RichText::new("Name of this computer");
            ui.label(label.color(theme::FG_SOFT()).size(TEXT_SIZE));
            let field = egui::TextEdit::singleline(&mut server.computer_name)
                .hint_text("the host name")
                .desired_width((ui.available_width() - ui.spacing().item_spacing.x).max(120.0))
                .id_salt("settings_server_computer_name");
            changed |= ui.add(field).changed();
        });
        ui.add_space(4.0);
        changed |= ui.checkbox(&mut server.share_terminals, SEE).changed();
        // Nobody types in a terminal that no other computer sees.
        let typing = egui::Checkbox::new(&mut server.remote_typing, TYPE);
        changed |= ui.add_enabled(server.share_terminals, typing).changed();
        changed
    })
}

#[cfg(test)]
mod tests;
