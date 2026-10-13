//! The toolbar's two controls: the primary New workspace button and the Menu that holds
//! Quick Nav, Remote Hosts, Cloud, Sessions and Settings.
use egui::{
    Align2, CornerRadius, CursorIcon, FontId, Popup, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};

use crate::app::HorizonApp;
use crate::app::root_chrome::{
    ROOT_TOOLBAR_BUTTON_HEIGHT, ROOT_TOOLBAR_MENU_WIDTH, ROOT_TOOLBAR_NEW_COMPACT_WIDTH, ROOT_TOOLBAR_NEW_WIDTH,
    ToolbarAction,
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
        title: "New cloud",
        detail: "Start a cloud worker in the current workspace",
        action: None,
        choice: MenuChoice::NewCloud,
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

/// Width of the part that chooses where the workspace runs.
const CHOOSER_WIDTH: f32 = 82.0;

impl HorizonApp {
    /// "+ New workspace" makes a workspace where the default says; the part after the
    /// divider opens the menu of places.
    pub(super) fn render_toolbar_new_workspace(&mut self, ui: &mut Ui, compact: bool) {
        let width = if compact {
            ROOT_TOOLBAR_NEW_COMPACT_WIDTH
        } else {
            ROOT_TOOLBAR_NEW_WIDTH
        };
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, ROOT_TOOLBAR_BUTTON_HEIGHT), Sense::hover());
        let split = rect.max.x - CHOOSER_WIDTH;
        let create_rect = Rect::from_min_max(rect.min, Pos2::new(split, rect.max.y));
        let choose_rect = Rect::from_min_max(Pos2::new(split, rect.min.y), rect.max);
        let create = ui
            .interact(create_rect, ui.id().with("toolbar-new-workspace"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        let choose = ui
            .interact(choose_rect, ui.id().with("toolbar-new-workspace-place"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        let default = if self.new_cloud_workspace_ready() {
            NewWorkspace::Cloud
        } else {
            NewWorkspace::ThisPc
        };
        let create = create.on_hover_text(format!("New workspace: {}", default.detail()));
        create.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "New workspace"));
        choose.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Choose where it runs"));

        let painter = ui.painter();
        let radius = CornerRadius::same(10);
        painter.rect_filled(rect, radius, theme::ACCENT());
        // The hovered part lightens; the other keeps the solid accent.
        let lift = |response: &egui::Response| {
            if response.is_pointer_button_down_on() {
                Some(theme::blend(theme::ACCENT(), theme::BG(), 0.14))
            } else if response.hovered() {
                Some(theme::blend(theme::ACCENT(), theme::FG(), 0.16))
            } else {
                None
            }
        };
        if let Some(fill) = lift(&create) {
            let corners = CornerRadius {
                nw: 10,
                sw: 10,
                ne: 0,
                se: 0,
            };
            painter.rect_filled(create_rect, corners, fill);
        }
        if let Some(fill) = lift(&choose) {
            let corners = CornerRadius {
                nw: 0,
                sw: 0,
                ne: 10,
                se: 10,
            };
            painter.rect_filled(choose_rect, corners, fill);
        }
        let ink = theme::BG();
        painter.line_segment(
            [Pos2::new(split, rect.min.y + 7.0), Pos2::new(split, rect.max.y - 7.0)],
            Stroke::new(1.0, theme::alpha(ink, 90)),
        );
        let label = if compact { "+  New" } else { "+  New workspace" };
        painter.text(
            Pos2::new(create_rect.min.x + 14.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(12.5),
            ink,
        );
        let icon_center = Pos2::new(choose_rect.min.x + 17.0, rect.center().y);
        let glyph = if default == NewWorkspace::ThisPc {
            rich_menu::Glyph::Screen
        } else {
            rich_menu::Glyph::Cloud
        };
        rich_menu::paint(painter, glyph, icon_center, ink);
        let place = if default == NewWorkspace::ThisPc { "PC" } else { "Cloud" };
        painter.text(
            Pos2::new(icon_center.x + 12.0, rect.center().y),
            Align2::LEFT_CENTER,
            place,
            FontId::proportional(12.0),
            ink,
        );
        paint_chevron(painter, Pos2::new(choose_rect.max.x - 13.0, rect.center().y), ink);
        if create.has_focus() || choose.has_focus() {
            painter.rect_stroke(rect.expand(2.0), 12, Stroke::new(2.0, theme::FG()), StrokeKind::Outside);
        }

        if create.clicked() {
            self.create_new_workspace(ui.ctx(), default, None);
        }
        if let Some(choice) = self.new_workspace_menu(&choose) {
            self.create_new_workspace(ui.ctx(), choice, None);
        }
    }

    /// One menu for everything that used to be its own toolbar button.
    pub(super) fn render_toolbar_menu(&mut self, ui: &mut Ui) {
        let button = ui.add(
            egui::Button::new(RichText::new("     Menu").size(12.0).color(theme::FG_SOFT()))
                .fill(theme::PANEL_BG_ALT())
                .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 210)))
                .corner_radius(10)
                .min_size(Vec2::new(ROOT_TOOLBAR_MENU_WIDTH, ROOT_TOOLBAR_BUTTON_HEIGHT)),
        );
        paint_lines(
            ui.painter(),
            Pos2::new(button.rect.min.x + 18.0, button.rect.center().y),
            theme::FG_SOFT(),
        );
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
                if rich_menu::row(ui, None, entry.title, entry.detail, None, shortcut.as_deref(), enabled).clicked() {
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
                let workspace = self.board.ensure_workspace();
                self.open_cloud_for_workspace(ctx, workspace);
            }
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

fn paint_chevron(painter: &egui::Painter, center: Pos2, color: egui::Color32) {
    painter.add(Shape::line(
        vec![
            Pos2::new(center.x - 3.5, center.y - 1.5),
            Pos2::new(center.x, center.y + 2.0),
            Pos2::new(center.x + 3.5, center.y - 1.5),
        ],
        Stroke::new(1.5, color),
    ));
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
