//! The context menus of the sidebar's workspace and panel rows.
use egui::Button;
use horizon_core::{PanelId, WorkspaceLayout};

use crate::theme;

use super::super::HorizonApp;
use super::machines::MachineRow;
use super::place;
use super::place::PlaceEntry;
use super::{SidebarActions, WorkspaceSidebarEntry};

/// What the person chose in the close-all part of a workspace menu.
enum CloseAllChoice {
    /// Nothing.
    None,
    /// Close the panels now: no panel of a machine holds a session.
    Close,
    /// Ask first, because a panel of a machine holds a session that the close ends.
    AskFirst,
}

/// The Place section of a workspace menu: the check mark of the current place,
/// the machines of the list, and the reason of a machine that is gone. Returns
/// the place the person chose.
fn show_place_menu(ui: &mut egui::Ui, entries: &[PlaceEntry]) -> Option<horizon_core::WorkspacePlace> {
    ui.label(egui::RichText::new("Place").size(11.0).color(theme::FG_DIM()))
        .on_hover_text("A new terminal of this workspace opens there. The panels that are open stay where they are.");
    let mut chosen = None;
    for entry in entries {
        // The check mark is Horizon's character, and a menu is open only while
        // the person reads it, so the line is made then.
        let label = if entry.current {
            format!("\u{2713} {}", entry.label)
        } else {
            entry.label.clone()
        };
        let color = if entry.gone { theme::FG_DIM() } else { theme::FG_SOFT() };
        let mut row = ui.add_enabled(
            !entry.gone,
            Button::new(egui::RichText::new(label).size(12.0).color(color)).frame(false),
        );
        if entry.gone {
            row = row.on_disabled_hover_text("This machine is not in Horizon's machine list anymore.");
        }
        if row.clicked() {
            chosen = Some(entry.place.clone());
        }
    }
    chosen
}

/// The close-all entry of a workspace menu. A close that ends a tmux session
/// leaves the menu for the question that the close of one panel asks.
fn show_close_all_menu(ui: &mut egui::Ui, machine_panels: usize) -> CloseAllChoice {
    let clicked = ui
        .add(
            Button::new(
                egui::RichText::new("Close All Panels")
                    .size(12.0)
                    .color(theme::PALETTE_RED()),
            )
            .frame(false),
        )
        .clicked();
    if !clicked {
        return CloseAllChoice::None;
    }
    // A panel on a machine holds work that a close ends, so the menu asks
    // first, as it does before it forgets a machine.
    if machine_panels > 0 {
        CloseAllChoice::AskFirst
    } else {
        CloseAllChoice::Close
    }
}

impl HorizonApp {
    pub(super) fn show_workspace_context_menu(
        &mut self,
        response: &egui::Response,
        workspace: &WorkspaceSidebarEntry,
        actions: &mut SidebarActions,
    ) {
        // A tooltip layer, as the panel menu uses: the row lives in the sidebar's clipped
        // rectangle, so a menu drawn in the row's own layer is cut to a slice of its width.
        let popup = egui::Popup::context_menu(response)
            .kind(egui::PopupKind::Tooltip)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        let popup_layer = egui::LayerId::new(egui::Order::Tooltip, popup.get_id());
        let shown = popup.show(|ui| {
            ui.set_min_width(160.0);
            ui.label(egui::RichText::new("Arrange Panels").size(11.0).color(theme::FG_DIM()));
            if ui
                .add_enabled(
                    workspace.capabilities.can_arrange,
                    Button::new(egui::RichText::new("Default").size(12.0).color(theme::FG_SOFT())).frame(false),
                )
                .clicked()
            {
                actions.clear_layout = Some(workspace.id);
                ui.close();
            }
            for layout in WorkspaceLayout::ALL {
                let text = egui::RichText::new(layout.label()).size(12.0).color(theme::FG_SOFT());
                if ui
                    .add_enabled(workspace.capabilities.can_arrange, Button::new(text).frame(false))
                    .clicked()
                {
                    actions.arrange_layout = Some((workspace.id, layout));
                    ui.close();
                }
            }

            ui.separator();
            let detach_label = if workspace.detached {
                "Move to Main Window"
            } else {
                "Open in New Window"
            };
            if ui
                .add_enabled(
                    workspace.detached || workspace.capabilities.can_detach,
                    Button::new(egui::RichText::new(detach_label).size(12.0).color(theme::FG_SOFT())).frame(false),
                )
                .clicked()
            {
                if workspace.detached {
                    actions.reattach_workspace = Some(workspace.id);
                } else {
                    actions.detach_workspace = Some(workspace.id);
                }
                ui.close();
            }

            ui.separator();
            // The entries are made only while the menu is open: a frame that
            // draws the row alone builds no line of a machine.
            let entries = place::entries(&workspace.place, &self.machine_list);
            if let Some(place) = show_place_menu(ui, &entries) {
                actions.set_workspace_place = Some((workspace.id, place));
                ui.close();
            }

            ui.separator();
            if ui
                .add_enabled(
                    workspace.capabilities.can_remove_empty,
                    Button::new(
                        egui::RichText::new("Remove Empty Workspace")
                            .size(12.0)
                            .color(theme::FG_SOFT()),
                    )
                    .frame(false),
                )
                .on_disabled_hover_text("This workspace holds a panel, or it is the last one.")
                .clicked()
            {
                actions.remove_empty_workspace = Some(workspace.id);
                ui.close();
            }
            match show_close_all_menu(ui, workspace.machine_panels) {
                CloseAllChoice::Close => {
                    actions.close_all_in_workspace = Some(workspace.id);
                    ui.close();
                }
                CloseAllChoice::AskFirst => {
                    // The question stands above the toolbar of a workspace, so the
                    // menu closes and leaves the press to it.
                    self.close_workspace_confirm = Some(workspace.id);
                    ui.close();
                }
                CloseAllChoice::None => {}
            }
        });
        if shown.is_some() {
            response
                .ctx
                .memory_mut(|memory| memory.areas_mut().set_sublayer(response.layer_id, popup_layer));
        }
    }

    /// The menu of one machine row: Connect, and Forget behind a confirm line.
    pub(super) fn show_machine_context_menu(
        &mut self,
        response: &egui::Response,
        row: &MachineRow,
        actions: &mut SidebarActions,
    ) {
        // A tooltip layer, as the workspace and panel menus use: the row lives in
        // the sidebar's clipped rectangle, so a menu drawn in the row's own layer
        // is cut to a slice of its width.
        let popup = egui::Popup::context_menu(response)
            .kind(egui::PopupKind::Tooltip)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        let popup_layer = egui::LayerId::new(egui::Order::Tooltip, popup.get_id());
        let confirming = self.forget_machine.as_deref() == Some(row.name.as_str());
        let mut open_confirm = false;
        let shown = popup.show(|ui| {
            ui.set_min_width(180.0);
            let connect = if row.panel.is_some() { "Focus" } else { "Connect" };
            if ui
                .add(Button::new(egui::RichText::new(connect).size(12.0).color(theme::FG_SOFT())).frame(false))
                .clicked()
            {
                if row.panel.is_some() {
                    actions.focus_panel = row.panel;
                    actions.pan_to_panel = row.panel;
                } else {
                    actions.connect_machine = Some(row.name.clone());
                }
                ui.close();
            }
            ui.separator();
            if self.board.active_workspace.is_some()
                && ui
                    .add(
                        Button::new(
                            egui::RichText::new("Put the active workspace here")
                                .size(12.0)
                                .color(theme::FG_SOFT()),
                        )
                        .frame(false),
                    )
                    .on_hover_text(
                        "Its new terminals open on this machine. The panels that are open stay where they are.",
                    )
                    .clicked()
            {
                actions.put_active_workspace_here = Some(row.name.clone());
                ui.close();
            }
            ui.separator();
            if confirming {
                ui.label(
                    egui::RichText::new(format!("Forget {}? Its key stays.", row.name))
                        .size(11.0)
                        .color(theme::FG_DIM()),
                );
                if ui
                    .add(Button::new(egui::RichText::new("Forget").size(12.0).color(theme::PALETTE_RED())).frame(false))
                    .clicked()
                {
                    actions.forget_machine = Some(row.name.clone());
                    self.forget_machine = None;
                    ui.close();
                }
                if ui
                    .add(Button::new(egui::RichText::new("Cancel").size(12.0).color(theme::FG_SOFT())).frame(false))
                    .clicked()
                {
                    self.forget_machine = None;
                    ui.close();
                }
            } else if ui
                .add(
                    Button::new(
                        egui::RichText::new("Forget this machine")
                            .size(12.0)
                            .color(theme::FG_SOFT()),
                    )
                    .frame(false),
                )
                .on_hover_text("Removes the row. The machine key stays until you delete it.")
                .clicked()
            {
                open_confirm = true;
            }
        });
        if open_confirm {
            self.forget_machine = Some(row.name.clone());
        }
        if shown.is_some() {
            response
                .ctx
                .memory_mut(|memory| memory.areas_mut().set_sublayer(response.layer_id, popup_layer));
        }
    }

    pub(super) fn show_sidebar_panel_context_menu(
        &mut self,
        response: &egui::Response,
        workspace: &WorkspaceSidebarEntry,
        panel_id: PanelId,
        kind: horizon_core::PanelKind,
        actions: &mut SidebarActions,
    ) {
        let popup = egui::Popup::context_menu(response)
            .kind(egui::PopupKind::Tooltip)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        let popup_layer = egui::LayerId::new(egui::Order::Tooltip, popup.get_id());
        let shown = popup.show(|ui| {
            ui.set_min_width(160.0);
            if let Some(destination) = self.show_workspace_destination(ui, response, workspace.id) {
                self.board.assign_panel_to_workspace(panel_id, destination);
                self.mark_runtime_dirty();
            }

            if kind.is_agent() {
                ui.separator();
                let idle = self
                    .board
                    .panel(panel_id)
                    .is_some_and(|panel| self.panel_attention_of(panel) == horizon_core::Attention::Idle);
                if ui
                    .add_enabled(
                        idle,
                        Button::new(egui::RichText::new("Mark as unread").size(12.0).color(theme::FG_SOFT()))
                            .frame(false),
                    )
                    .on_disabled_hover_text("This agent works, waits for you or is already unread.")
                    .on_hover_text("Shows this panel as done in the sidebar until you look at it again.")
                    .clicked()
                {
                    self.mark_panel_unread(panel_id);
                    ui.close();
                }
            }

            ui.separator();
            if (kind.is_agent() || kind == horizon_core::PanelKind::Ssh)
                && ui
                    .add(
                        Button::new(
                            egui::RichText::new(if kind == horizon_core::PanelKind::Ssh {
                                "Reconnect"
                            } else {
                                "Restart"
                            })
                            .size(12.0)
                            .color(theme::FG_SOFT()),
                        )
                        .frame(false),
                    )
                    .clicked()
            {
                self.reconnect_panel_now(panel_id);
                ui.close();
            }
            if ui
                .add(Button::new(egui::RichText::new("Close").size(12.0).color(theme::PALETTE_RED())).frame(false))
                .clicked()
            {
                actions.close_panel = Some(panel_id);
                ui.close();
            }
        });
        if shown.is_some() {
            response
                .ctx
                .memory_mut(|memory| memory.areas_mut().set_sublayer(response.layer_id, popup_layer));
        }
    }
}
