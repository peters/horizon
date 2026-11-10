//! The context menus of the sidebar's workspace and panel rows.
use egui::Button;
use horizon_core::{PanelId, WorkspaceLayout};

use crate::theme;

use super::super::HorizonApp;
use super::machines::MachineRow;
use super::{SidebarActions, WorkspaceSidebarEntry};

impl HorizonApp {
    pub(super) fn show_workspace_context_menu(
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
            if ui
                .add(
                    Button::new(
                        egui::RichText::new("Close All Panels")
                            .size(12.0)
                            .color(theme::PALETTE_RED()),
                    )
                    .frame(false),
                )
                .clicked()
            {
                actions.close_all_in_workspace = Some(workspace.id);
                ui.close();
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
                self.queue_panel_restart(panel_id);
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
