//! The section `Computers` of the sidebar: the other computers of the person
//! that hold a live link to the server of this one, and the terminal panels
//! that each one shares. A press on a row opens the panel that follows it, or
//! goes to the one that is open. The section is there only while another
//! computer is listed, or while the live link is down, which its line says.
//! A frame that does not draw the sidebar does no work for it.

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
        let note = (11.0, text::REGULAR, text::secondary());
        if let Some(reason) = &shared.down {
            line(ui, "Shared terminals: no link", note, None);
            paragraph(ui, reason, note);
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

/// The most rows that the reason of a link that is down takes: the sidebar is
/// narrow, so a longer reason would push the computers out of the section.
const REASON_ROWS: usize = 3;

/// One line of the section in a size, a weight and a colour, with its hover text.
fn line(ui: &mut egui::Ui, words: &str, style: (f32, f32, Color32), hover: Option<&str>) {
    indented(ui, words, style, text::ONE_ROW, hover);
}

/// The reason of a link that is down, under the line that says it: the rows that
/// its words need and at most [`REASON_ROWS`] of them, with the whole reason while
/// the pointer rests on it.
fn paragraph(ui: &mut egui::Ui, words: &str, style: (f32, f32, Color32)) {
    indented(ui, words, style, REASON_ROWS, Some(words));
}

/// One text of the section under its heading, in a size, a weight and a colour,
/// wrapped over at most `rows` rows, with its hover text.
fn indented(
    ui: &mut egui::Ui,
    words: &str,
    (size, weight, color): (f32, f32, Color32),
    rows: usize,
    hover: Option<&str>,
) {
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        let width = ui.available_width() - 18.0;
        let painted = ui.label(text::paragraph_rows(ui, words, size, weight, color, width, rows));
        if let Some(hover) = hover {
            painted.on_hover_text(hover);
        }
    });
}

/// The row of one shared panel of `computer`, with the state mark of an agent
/// row. Answers whether the person pressed it, which asks for its follow panel.
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
