//! The toolbar's controls: the Menu that holds Quick Nav, Remote Hosts, Sessions and
//! Settings.
use egui::{Align2, CursorIcon, FontId, Popup, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::app::HorizonApp;
use crate::app::root_chrome::{ROOT_TOOLBAR_BUTTON_HEIGHT, ROOT_TOOLBAR_MENU_WIDTH, ToolbarAction};
use crate::app::session_pickup::{MENU_DETAIL, MENU_TITLE};
use crate::app::sidebar::rich_menu;
use crate::app::util;
use crate::theme;

struct MenuEntry {
    /// A caption that starts a new group above this row.
    group: Option<&'static str>,
    title: &'static str,
    detail: &'static str,
    /// The toolbar action whose shortcut the row shows.
    action: Option<ToolbarAction>,
    /// What the row does when the person picks it.
    choice: ToolbarAction,
}

const MENU: &[MenuEntry] = &[
    MenuEntry {
        group: Some("Go to"),
        title: "Quick Nav",
        detail: "Jump to any workspace, panel or command",
        action: Some(ToolbarAction::QuickNav),
        choice: ToolbarAction::QuickNav,
    },
    MenuEntry {
        group: None,
        title: "Sessions",
        detail: "Switch between saved boards",
        action: Some(ToolbarAction::Sessions),
        choice: ToolbarAction::Sessions,
    },
    MenuEntry {
        group: None,
        title: MENU_TITLE,
        detail: MENU_DETAIL,
        action: None,
        choice: ToolbarAction::RecentSessions,
    },
    MenuEntry {
        group: None,
        title: "Remote Hosts",
        detail: "Open a terminal or desktop on another machine",
        action: Some(ToolbarAction::RemoteHosts),
        choice: ToolbarAction::RemoteHosts,
    },
    MenuEntry {
        group: Some("Get started"),
        title: "Set up",
        detail: "Rent a machine, add the Horizon server, or a browser that stays signed in",
        action: None,
        choice: ToolbarAction::SetUp,
    },
    MenuEntry {
        group: Some("Horizon"),
        title: "Settings",
        detail: "Theme, shortcuts, agents and features",
        action: Some(ToolbarAction::Settings),
        choice: ToolbarAction::Settings,
    },
    MenuEntry {
        group: None,
        title: "Quit Horizon",
        detail: "End Horizon and every terminal and agent program in it",
        action: None,
        choice: ToolbarAction::Quit,
    },
];

impl HorizonApp {
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
                let row = rich_menu::row(ui, entry.title, entry.detail, shortcut.as_deref(), true);
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
            self.perform_toolbar_action(ui.ctx(), choice);
        }
    }

    fn toolbar_shortcut(&self, action: ToolbarAction) -> Option<String> {
        let label = util::primary_shortcut_label();
        let shortcuts = &self.shortcuts;
        let text = match action {
            ToolbarAction::QuickNav => shortcuts.command_palette.display_label(label),
            ToolbarAction::RemoteHosts => shortcuts.open_remote_hosts.display_label(label),
            ToolbarAction::Sessions => shortcuts.toggle_sessions.display_label(label),
            // Set up, the recent agent sessions and Quit have no key of their
            // own: their rows say what they do.
            ToolbarAction::SetUp | ToolbarAction::RecentSessions | ToolbarAction::Quit => String::new(),
            ToolbarAction::Settings => shortcuts.toggle_settings.display_label(label),
        };
        (!text.is_empty()).then_some(text)
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

#[cfg(test)]
mod tests {
    use super::MENU;
    use crate::app::root_chrome::ToolbarAction;

    /// Set up is a row of its own group, above the settings, and it names what
    /// the wizard does.
    #[test]
    fn the_menu_offers_set_up_in_a_group_of_its_own_above_the_settings() {
        let set_up = MENU
            .iter()
            .position(|entry| entry.title == "Set up")
            .expect("the Menu offers Set up");
        let settings = MENU
            .iter()
            .position(|entry| entry.title == "Settings")
            .expect("the Menu offers Settings");

        assert_eq!(MENU[set_up].group, Some("Get started"), "the row starts its own group");
        assert_eq!(
            MENU[set_up].detail,
            "Rent a machine, add the Horizon server, or a browser that stays signed in"
        );
        assert!(set_up < settings, "Set up sits above Settings");
        assert!(
            MENU[set_up + 1..settings].iter().all(|entry| entry.group.is_none()),
            "its group holds that one row and nothing else"
        );
        assert!(
            MENU[set_up].action.is_none(),
            "Set up has no key of its own, so its row names its shortcut as none"
        );
    }

    /// The row starts Set up without a goal, so its agent asks which of the four
    /// the person wants.
    #[test]
    fn the_menu_row_presses_set_up_without_a_goal() {
        let (_temp, ctx, mut app) =
            crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
                runtime_state: Box::new(horizon_core::RuntimeState::default()),
            });
        app.setup.integrations = false;

        app.perform_toolbar_action(&ctx, ToolbarAction::SetUp);

        assert_eq!(app.setup.goal, horizon_core::SetupGoal::Choose);
        assert!(
            app.setup.notice_panel.is_some(),
            "the row started Set up, which draws its notice without integrations"
        );
    }
}
