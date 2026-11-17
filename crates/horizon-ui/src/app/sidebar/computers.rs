//! The section `Computers` of the sidebar: the other computers of the person
//! that hold a live link to the server of this one, and the terminal panels
//! that each one shares. A press on a row opens the panel that follows it, or
//! goes to the one that is open.
//!
//! The section is there only while another computer is listed, or while the
//! live link is down, which its one line then says. Its rows are the list that
//! the link gave, so a frame that does not draw the sidebar does no work for it.

use egui::{Color32, CursorIcon, Sense, Vec2};
use horizon_sync::LivePanel;

use crate::theme;

use super::super::HorizonApp;
use super::super::shared_terminals::{attention_of, computer_name, kind_of};
use super::panel_rows::{self, PANEL_ROW_HEIGHT, PanelRowLook, PanelRowState};
use super::{SidebarActions, rows, text};

impl HorizonApp {
    /// Draws the section under the machines.
    pub(super) fn render_sidebar_computers(&self, ui: &mut egui::Ui, actions: &mut SidebarActions) {
        let shared = &self.shared_terminals;
        let computers = &shared.follower.computers;
        if computers.is_empty() && shared.down.is_none() {
            return;
        }
        ui.add_space(4.0);
        line(ui, "Computers", (13.0, text::SEMIBOLD, theme::FG_SOFT()), None);
        if let Some(reason) = &shared.down {
            let words = format!("Shared terminals: no link \u{00b7} {reason}");
            line(ui, &words, (11.0, text::REGULAR, text::secondary()), Some(&words));
        }
        for computer in computers {
            ui.add_space(4.0);
            let name = computer_name(computer);
            line(ui, name, (13.0, text::MEDIUM, theme::FG()), Some(&computer.device));
            let last = computer.panels.len().saturating_sub(1);
            for (index, panel) in computer.panels.iter().enumerate() {
                if render_panel(ui, name, panel, index == last) {
                    actions.open_shared_terminal = Some((computer.device.clone(), panel.id.clone()));
                }
            }
        }
        ui.add_space(6.0);
    }
}

/// One line of the section in a size, a weight and a colour, with the text
/// that its hover says.
fn line(ui: &mut egui::Ui, words: &str, (size, weight, color): (f32, f32, Color32), hover: Option<&str>) {
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        let width = ui.available_width() - 18.0;
        let painted = ui.label(text::line(ui, words, size, weight, color, width));
        if let Some(hover) = hover {
            painted.on_hover_text(hover);
        }
    });
}

/// The row of one shared panel of the computer `computer`, with the mark of
/// its state as the row of an agent of a machine shows it. Answers whether the
/// person pressed it, which asks for its follow panel.
fn render_panel(ui: &mut egui::Ui, computer: &str, panel: &LivePanel, last: bool) -> bool {
    let rect = ui.allocate_space(Vec2::new(ui.available_width(), PANEL_ROW_HEIGHT)).1;
    let hovered = ui.rect_contains_pointer(rect);
    let (kind, attention) = (kind_of(&panel.kind), attention_of(&panel.state));
    let title = Some(panel.title.as_str()).filter(|title| !title.is_empty());
    let label = title.unwrap_or(kind.display_name());
    let look = PanelRowLook {
        guide_x: panel_rows::guide_x(rows::card(rect)),
        accent: theme::ACCENT(),
        kind,
        focused: false,
        label,
        attention,
    };
    let close = None;
    panel_rows::paint_panel_row(ui, rect, &look, PanelRowState { hovered, close, last });
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let id = ui.make_persistent_id(("sidebar_computer_panel", computer, &panel.id));
    let response = ui.interact(rect, id, Sense::click());
    let spoken = text::panel_spoken(kind, label, attention);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &spoken));
    let typing = ["view only", "you can type in it"][usize::from(panel.typing)];
    response
        .on_hover_text(format!("{spoken}\nOn {computer}, {typing}"))
        .clicked()
}
