//! The toolbar's controls: the primary New cloud button and the Menu that holds
//! Quick Nav, Remote Hosts, Cloud, Sessions and Settings.
use egui::{Align2, CursorIcon, FontId, Popup, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::app::HorizonApp;
use crate::app::root_chrome::{
    ROOT_TOOLBAR_BUTTON_HEIGHT, ROOT_TOOLBAR_MENU_WIDTH, ROOT_TOOLBAR_NEW_CLOUD_COMPACT_WIDTH,
    ROOT_TOOLBAR_NEW_CLOUD_WIDTH, ToolbarAction,
};
use crate::app::sidebar::new_workspace::NewWorkspace;
use crate::app::sidebar::rich_menu;
use crate::app::util;
use crate::theme;

/// What a Menu row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuChoice {
    Action(ToolbarAction),
    #[cfg(feature = "cloud-workspaces")]
    NewCloud,
    #[cfg(feature = "cloud-workspaces")]
    SetupBoard,
    #[cfg(feature = "cloud-workspaces")]
    FitClouds,
}

struct MenuEntry {
    /// A caption that starts a new group above this row.
    group: Option<&'static str>,
    title: &'static str,
    detail: &'static str,
    /// The toolbar action whose shortcut the row shows.
    action: Option<ToolbarAction>,
    choice: MenuChoice,
}

const MENU: &[MenuEntry] = &[
    MenuEntry {
        group: Some("Go to"),
        title: "Quick Nav",
        detail: "Jump to any workspace, panel or command",
        action: Some(ToolbarAction::QuickNav),
        choice: MenuChoice::Action(ToolbarAction::QuickNav),
    },
    MenuEntry {
        group: None,
        title: "Sessions",
        detail: "Switch between saved boards",
        action: Some(ToolbarAction::Sessions),
        choice: MenuChoice::Action(ToolbarAction::Sessions),
    },
    MenuEntry {
        group: None,
        title: "Remote Hosts",
        detail: "Open a terminal or desktop on another machine",
        action: Some(ToolbarAction::RemoteHosts),
        choice: MenuChoice::Action(ToolbarAction::RemoteHosts),
    },
    #[cfg(feature = "cloud-workspaces")]
    MenuEntry {
        group: Some("Cloud"),
        title: "Add cloud here",
        detail: "Start a cloud worker in the current workspace",
        action: None,
        choice: MenuChoice::NewCloud,
    },
    #[cfg(feature = "cloud-workspaces")]
    MenuEntry {
        group: None,
        title: "Setup board",
        detail: "Every repository whose setup agent works on this computer",
        action: None,
        choice: MenuChoice::SetupBoard,
    },
    #[cfg(feature = "cloud-workspaces")]
    MenuEntry {
        group: None,
        title: "Cloud settings",
        detail: "Providers, GitHub and accounts",
        action: None,
        choice: MenuChoice::Action(ToolbarAction::Cloud),
    },
    #[cfg(feature = "cloud-workspaces")]
    MenuEntry {
        group: None,
        title: "Fit all clouds",
        detail: "Show every cloud on the canvas",
        action: None,
        choice: MenuChoice::FitClouds,
    },
    MenuEntry {
        group: Some("Horizon"),
        title: "Settings",
        detail: "Theme, shortcuts, agents and features",
        action: Some(ToolbarAction::Settings),
        choice: MenuChoice::Action(ToolbarAction::Settings),
    },
];

impl HorizonApp {
    /// "+ New cloud" makes a workspace and opens New cloud for it, and brings back a draft a
    /// dismissal parked. Cancel ends the draft and takes its workspace away when it is empty.
    /// The tooltip shows the machine of a quick start and its price, asked for only while the
    /// pointer is on the button.
    pub(super) fn render_toolbar_new_cloud(&mut self, ui: &mut Ui, compact: bool) {
        let width = if compact {
            ROOT_TOOLBAR_NEW_CLOUD_COMPACT_WIDTH
        } else {
            ROOT_TOOLBAR_NEW_CLOUD_WIDTH
        };
        let ready = self.new_cloud_workspace_ready();
        // A dimmed button takes no click or focus, but still explains itself on hover.
        let sense = if ready { Sense::click() } else { Sense::hover() };
        let (rect, button) = ui.allocate_exact_size(Vec2::new(width, ROOT_TOOLBAR_BUTTON_HEIGHT), sense);
        let button = if ready {
            button.on_hover_cursor(CursorIcon::PointingHand)
        } else {
            button
        };
        button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ready, "New cloud"));
        paint_new_cloud(ui.painter(), rect, &button, compact, ready);
        if !ready {
            button.on_hover_text(super::super::NEW_CLOUD_NOT_READY);
            return;
        }
        #[cfg(feature = "cloud-workspaces")]
        let machine = button
            .hovered()
            .then(|| self.new_cloud_machine_line(ui.ctx()))
            .flatten();
        #[cfg(not(feature = "cloud-workspaces"))]
        let machine: Option<String> = None;
        let button = button.on_hover_ui(|ui| {
            ui.label("A new workspace with a cloud worker. Agents run there, also while this computer sleeps.");
            if let Some(machine) = &machine {
                ui.label(egui::RichText::new(machine).color(theme::FG_DIM()));
            }
        });
        if button.clicked() {
            self.create_new_workspace(ui.ctx(), NewWorkspace::Cloud, None);
        }
    }

    /// One menu for everything that used to be its own toolbar button.
    pub(super) fn render_toolbar_menu(&mut self, ui: &mut Ui) {
        let (rect, button) = ui.allocate_exact_size(
            Vec2::new(ROOT_TOOLBAR_MENU_WIDTH, ROOT_TOOLBAR_BUTTON_HEIGHT),
            Sense::click(),
        );
        let button = button.on_hover_cursor(CursorIcon::PointingHand);
        button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Menu"));
        paint_menu_button(ui.painter(), rect, &button);
        let popup = Popup::menu(&button).kind(egui::PopupKind::Tooltip);
        let popup_layer = egui::LayerId::new(egui::Order::Tooltip, popup.get_id());
        let mut chosen = None;
        let shown = popup.show(|ui| {
            ui.set_width(400.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            for (index, entry) in MENU.iter().enumerate() {
                if let Some(group) = entry.group {
                    if index > 0 {
                        rich_menu::divider(ui);
                    }
                    rich_menu::caption(ui, group);
                }
                let shortcut = entry.action.and_then(|action| self.toolbar_shortcut(action));
                let enabled = self.menu_entry_enabled(entry.choice);
                let row = rich_menu::row(ui, entry.title, entry.detail, shortcut.as_deref(), enabled);
                // A dimmed cloud row says why, as + New cloud does.
                let row = match disabled_reason(entry.choice) {
                    Some(reason) if !enabled => row.on_hover_text(reason),
                    _ => row,
                };
                if row.clicked() {
                    chosen = Some(entry.choice);
                    ui.close();
                }
            }
        });
        if shown.is_some() {
            button
                .ctx
                .memory_mut(|memory| memory.areas_mut().set_sublayer(button.layer_id, popup_layer));
        }
        if let Some(choice) = chosen {
            self.perform_menu_choice(ui.ctx(), choice);
        }
    }

    fn menu_entry_enabled(&self, choice: MenuChoice) -> bool {
        match choice {
            #[cfg(feature = "cloud-workspaces")]
            MenuChoice::NewCloud => self.cloud_launch_ready(),
            #[cfg(feature = "cloud-workspaces")]
            MenuChoice::FitClouds => !self.cloud_prototype.groups.0.is_empty(),
            _ => true,
        }
    }

    fn perform_menu_choice(&mut self, ctx: &egui::Context, choice: MenuChoice) {
        match choice {
            MenuChoice::Action(action) => self.perform_toolbar_action(ctx, action),
            #[cfg(feature = "cloud-workspaces")]
            MenuChoice::NewCloud => {
                // A draft that a dismissal parked comes back instead of a second one.
                if !self.reopen_cloud_draft(ctx) {
                    let workspace = self.board.ensure_workspace();
                    self.open_cloud_for_workspace(ctx, workspace);
                }
            }
            #[cfg(feature = "cloud-workspaces")]
            MenuChoice::SetupBoard => self.open_setup_board(ctx, true),
            #[cfg(feature = "cloud-workspaces")]
            MenuChoice::FitClouds => self.cloud_overview(ctx),
        }
    }

    fn toolbar_shortcut(&self, action: ToolbarAction) -> Option<String> {
        let label = util::primary_shortcut_label();
        let shortcuts = &self.shortcuts;
        let text = match action {
            ToolbarAction::QuickNav => shortcuts.command_palette.display_label(label),
            ToolbarAction::RemoteHosts => shortcuts.open_remote_hosts.display_label(label),
            ToolbarAction::Sessions => shortcuts.toggle_sessions.display_label(label),
            ToolbarAction::Settings => shortcuts.toggle_settings.display_label(label),
            #[cfg(feature = "cloud-workspaces")]
            ToolbarAction::Cloud => return None,
        };
        (!text.is_empty()).then_some(text)
    }
}

/// Why a dimmed Menu row cannot be chosen, for the rows that need cloud workspaces.
fn disabled_reason(choice: MenuChoice) -> Option<&'static str> {
    match choice {
        #[cfg(feature = "cloud-workspaces")]
        MenuChoice::NewCloud => Some(super::super::NEW_CLOUD_NOT_READY),
        _ => None,
    }
}

/// Paints the New cloud button: the accent pill, lighter while hovered and darker while
/// pressed. When no cloud can start, it is a framed pill with dim text, as disabled
/// controls are.
fn paint_new_cloud(painter: &egui::Painter, rect: Rect, button: &egui::Response, compact: bool, ready: bool) {
    let label = if compact { "+  Cloud" } else { "+  New cloud" };
    let paint_label = |painter: &egui::Painter, ink| {
        painter.text(
            Pos2::new(rect.min.x + 14.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(12.5),
            ink,
        );
    };
    if !ready {
        painter.rect_filled(rect, 10, theme::PANEL_BG_ALT());
        painter.rect_stroke(
            rect,
            10,
            Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 210)),
            StrokeKind::Inside,
        );
        paint_label(painter, theme::FG_DIM());
        return;
    }
    let fill = if button.is_pointer_button_down_on() {
        theme::blend(theme::ACCENT(), theme::BG(), 0.14)
    } else if button.hovered() {
        theme::blend(theme::ACCENT(), theme::FG(), 0.16)
    } else {
        theme::ACCENT()
    };
    painter.rect_filled(rect, 10, fill);
    paint_label(painter, theme::BG());
    if button.has_focus() {
        painter.rect_stroke(rect.expand(2.0), 12, Stroke::new(2.0, theme::FG()), StrokeKind::Outside);
    }
}

/// Paints the Menu button: a framed pill with three lines and the word Menu.
fn paint_menu_button(painter: &egui::Painter, rect: Rect, button: &egui::Response) {
    paint_framed_button(painter, rect, button, |painter, ink| {
        paint_lines(painter, Pos2::new(rect.min.x + 18.0, rect.center().y), ink);
        painter.text(
            Pos2::new(rect.min.x + 32.0, rect.center().y),
            Align2::LEFT_CENTER,
            "Menu",
            FontId::proportional(12.0),
            ink,
        );
    });
}

/// Paints a framed pill, its content in `ink`, and the focus ring.
fn paint_framed_button(
    painter: &egui::Painter,
    rect: Rect,
    button: &egui::Response,
    content: impl FnOnce(&egui::Painter, egui::Color32),
) {
    let fill = if button.hovered() || button.has_focus() {
        theme::blend(theme::PANEL_BG_ALT(), theme::FG(), 0.08)
    } else {
        theme::PANEL_BG_ALT()
    };
    painter.rect_filled(rect, 10, fill);
    painter.rect_stroke(
        rect,
        10,
        Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 210)),
        StrokeKind::Inside,
    );
    content(painter, theme::FG_SOFT());
    if button.has_focus() {
        painter.rect_stroke(rect.expand(2.0), 12, Stroke::new(2.0, theme::FG()), StrokeKind::Outside);
    }
}

fn paint_lines(painter: &egui::Painter, center: Pos2, color: egui::Color32) {
    for offset in [-4.0, 0.0, 4.0] {
        painter.line_segment(
            [
                Pos2::new(center.x - 5.5, center.y + offset),
                Pos2::new(center.x + 5.5, center.y + offset),
            ],
            Stroke::new(1.4, color),
        );
    }
}
