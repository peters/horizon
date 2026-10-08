#[cfg(feature = "cloud-workspaces")]
use egui::containers::menu::SubMenuButton;
use egui::{Atom, Button, Context, Id, Key, Modifiers, Painter, Popup, Pos2, Rect, RectAlign, RichText, Stroke, Vec2};
use horizon_core::{AppShortcuts, ShortcutBinding};

use super::MARK_LABEL_GAP;
use crate::app::root_chrome::{ROOT_TOOLBAR_BUTTON_HEIGHT, ROOT_TOOLBAR_MENU_GAP, ROOT_TOOLBAR_MENU_WIDTH};
use crate::app::{HorizonApp, util};
use crate::theme;

const MENU_LABEL: &str = "Menu";
#[cfg(feature = "cloud-workspaces")]
const MENU_TOOLTIP: &str = "Quick Nav, Remote Hosts, Cloud, Sessions and Settings";
#[cfg(not(feature = "cloud-workspaces"))]
const MENU_TOOLTIP: &str = "Quick Nav, Remote Hosts, Sessions and Settings";
const MENU_MARK_SIZE: Vec2 = Vec2::new(12.0, 10.0);
const MENU_MIN_WIDTH: f32 = 220.0;
const MENU_ROW_HEIGHT: f32 = 26.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAction {
    QuickNav,
    RemoteHosts,
    Sessions,
    Settings,
}

impl MenuAction {
    const fn label(self) -> &'static str {
        match self {
            Self::QuickNav => "Quick Nav",
            Self::RemoteHosts => "Remote Hosts",
            Self::Sessions => "Sessions",
            Self::Settings => "Settings",
        }
    }

    const fn shortcut(self, shortcuts: &AppShortcuts) -> ShortcutBinding {
        match self {
            Self::QuickNav => shortcuts.command_palette,
            Self::RemoteHosts => shortcuts.open_remote_hosts,
            Self::Sessions => shortcuts.toggle_sessions,
            Self::Settings => shortcuts.toggle_settings,
        }
    }
}

impl HorizonApp {
    pub(super) fn render_toolbar_menu(&mut self, ui: &mut egui::Ui) {
        let mark_id = Id::new("toolbar-menu-mark");
        let button = Button::new((Atom::custom(mark_id, MENU_MARK_SIZE), util::chrome_label(MENU_LABEL)));
        let atoms = util::chrome_frame(button.gap(MARK_LABEL_GAP))
            .min_size(Vec2::new(ROOT_TOOLBAR_MENU_WIDTH, ROOT_TOOLBAR_BUTTON_HEIGHT))
            .atom_ui(ui);
        if let Some(rect) = atoms.rect(mark_id) {
            paint_menu_mark(ui.painter(), rect);
        }

        let mut response = atoms.response;
        if !Popup::is_id_open(ui.ctx(), Popup::default_response_id(&response)) {
            response = response.on_hover_text(MENU_TOOLTIP);
        }
        Popup::menu(&response)
            .align(RectAlign::BOTTOM_END)
            .gap(ROOT_TOOLBAR_MENU_GAP)
            .show(|ui| self.render_toolbar_menu_rows(ui));
    }

    fn render_toolbar_menu_rows(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(MENU_MIN_WIDTH);

        self.render_toolbar_menu_row(ui, MenuAction::QuickNav);
        self.render_toolbar_menu_row(ui, MenuAction::RemoteHosts);
        #[cfg(feature = "cloud-workspaces")]
        SubMenuButton::from_button(menu_row(ui, "Cloud", SubMenuButton::RIGHT_ARROW))
            .ui(ui, |ui| self.render_cloud_menu(ui));
        self.render_toolbar_menu_row(ui, MenuAction::Sessions);
        ui.separator();
        self.render_toolbar_menu_row(ui, MenuAction::Settings);
    }

    fn render_toolbar_menu_row(&mut self, ui: &mut egui::Ui, action: MenuAction) {
        let shortcut = action
            .shortcut(&self.shortcuts)
            .display_label(util::primary_shortcut_label());
        if ui.add(menu_row(ui, action.label(), &shortcut)).clicked() {
            // The key that chose the row must not also act in the overlay it
            // opens, such as running the first command palette result.
            ui.input_mut(|input| {
                input.consume_key(Modifiers::NONE, Key::Enter);
                input.consume_key(Modifiers::NONE, Key::Space);
            });
            self.perform_toolbar_menu_action(ui.ctx(), action);
            ui.close();
        }
    }

    fn perform_toolbar_menu_action(&mut self, ctx: &Context, action: MenuAction) {
        match action {
            MenuAction::QuickNav => self.open_command_palette(),
            MenuAction::RemoteHosts => self.toggle_remote_hosts_overlay(ctx),
            MenuAction::Sessions => self.toggle_session_manager(),
            MenuAction::Settings => self.toggle_settings(),
        }
    }
}

/// A menu row with a right-aligned hint. Both texts step up a token while the
/// row is hovered or focused, because `FG_DIM` is too faint on the hover fill.
fn menu_row(ui: &egui::Ui, label: &str, hint: &str) -> Button<'static> {
    let active = ui
        .ctx()
        .read_response(ui.next_auto_id())
        .is_some_and(|row| row.hovered() || row.has_focus());
    let (label_color, hint_color) = if active {
        (theme::FG(), theme::FG_SOFT())
    } else {
        (theme::FG_SOFT(), theme::FG_DIM())
    };
    Button::new(RichText::new(label).size(12.0).color(label_color))
        .right_text(RichText::new(hint).monospace().size(10.5).color(hint_color))
        .min_size(Vec2::new(0.0, MENU_ROW_HEIGHT))
}

/// Three stacked lines.
fn paint_menu_mark(painter: &Painter, rect: Rect) {
    const LINE_WIDTH: f32 = 1.4;

    let stroke = Stroke::new(LINE_WIDTH, theme::FG_SOFT());
    let inset = LINE_WIDTH * 0.5;
    for y in [rect.top() + inset, rect.center().y, rect.bottom() - inset] {
        painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], stroke);
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, PointerButton, Pos2, Rect, epaint::Shape};
    use horizon_core::{RuntimeState, ShortcutBinding, ShortcutKey, ShortcutModifiers, StartupDecision};

    use super::MenuAction;
    use crate::app::HorizonApp;
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};
    use crate::app::util::primary_shortcut_label;

    const SIZE: [f32; 2] = [1280.0, 800.0];

    fn frame(ctx: &egui::Context, app: &mut HorizonApp, events: Vec<Event>) -> egui::FullOutput {
        let mut input = raw_input(SIZE, None);
        input.events = events;
        run_app_frame_with_input(ctx, app, input)
    }

    fn click(ctx: &egui::Context, app: &mut HorizonApp, position: Pos2) -> egui::FullOutput {
        frame(ctx, app, vec![Event::PointerMoved(position)]);
        for pressed in [true, false] {
            frame(
                ctx,
                app,
                vec![Event::PointerButton {
                    pos: position,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        frame(ctx, app, Vec::new())
    }

    fn press(ctx: &egui::Context, app: &mut HorizonApp, key: Key) -> egui::FullOutput {
        for pressed in [true, false] {
            frame(
                ctx,
                app,
                vec![Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        frame(ctx, app, Vec::new())
    }

    fn focused_rect(ctx: &egui::Context) -> Option<Rect> {
        let focused = ctx.memory(egui::Memory::focused)?;
        ctx.read_response(focused).map(|response| response.rect)
    }

    fn text_rect(output: &egui::FullOutput, label: &str) -> Option<Rect> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == label => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
    }

    fn app_with_workspace() -> (tempfile::TempDir, egui::Context, HorizonApp) {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        app.root_viewport_stabilizer = None;
        let _ = app.board.create_workspace("Sample workspace");
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new());
        }
        (temp, ctx, app)
    }

    fn open_menu(ctx: &egui::Context, app: &mut HorizonApp) -> egui::FullOutput {
        let output = frame(ctx, app, Vec::new());
        let menu = text_rect(&output, "Menu").expect("Menu button").center();
        click(ctx, app, menu)
    }

    #[test]
    fn menu_lists_actions_in_order_with_their_configured_shortcuts() {
        let (_temp, ctx, mut app) = app_with_workspace();
        app.shortcuts.toggle_settings = ShortcutBinding::new(ShortcutModifiers::ALT, ShortcutKey::Letter('S'));
        let closed = frame(&ctx, &mut app, Vec::new());
        assert_eq!(text_rect(&closed, "Remote Hosts"), None, "menu starts closed");

        let output = open_menu(&ctx, &mut app);

        let mut rows = vec![
            MenuAction::QuickNav.label(),
            MenuAction::RemoteHosts.label(),
            MenuAction::Sessions.label(),
            MenuAction::Settings.label(),
        ];
        if cfg!(feature = "cloud-workspaces") {
            rows.insert(2, "Cloud");
        }
        let tops = rows
            .iter()
            .map(|row| {
                text_rect(&output, row)
                    .unwrap_or_else(|| panic!("missing row {row}"))
                    .top()
            })
            .collect::<Vec<_>>();
        assert!(tops.windows(2).all(|pair| pair[0] < pair[1]), "{rows:?} at {tops:?}");

        for action in [
            MenuAction::QuickNav,
            MenuAction::RemoteHosts,
            MenuAction::Sessions,
            MenuAction::Settings,
        ] {
            let label = text_rect(&output, action.label()).expect("row label");
            let expected = action.shortcut(&app.shortcuts).display_label(primary_shortcut_label());
            let shortcut = text_rect(&output, &expected).unwrap_or_else(|| panic!("missing shortcut {expected}"));
            assert!((shortcut.center().y - label.center().y).abs() < 2.0, "{expected}");
            assert!(shortcut.left() > label.right(), "{expected}");
        }
        assert!(
            text_rect(&output, "Alt+S").is_some(),
            "a changed shortcut shows in the menu"
        );
    }

    #[test]
    fn choosing_a_row_runs_its_action_and_closes_the_menu() {
        let (_temp, ctx, mut app) = app_with_workspace();
        let output = open_menu(&ctx, &mut app);
        assert!(app.command_palette.is_none());
        let quick_nav = text_rect(&output, MenuAction::QuickNav.label()).expect("Quick Nav row");
        let remote_hosts = text_rect(&output, MenuAction::RemoteHosts.label()).expect("Remote Hosts row");

        let output = click(&ctx, &mut app, quick_nav.center());

        assert!(app.command_palette.is_some(), "Quick Nav opens the command palette");
        // The palette lists Remote Hosts too, so look for the menu row where it was.
        assert_ne!(
            text_rect(&output, MenuAction::RemoteHosts.label()),
            Some(remote_hosts),
            "menu closes"
        );
    }

    #[test]
    fn keyboard_opens_the_menu_and_moves_through_its_rows() {
        let (_temp, ctx, mut app) = app_with_workspace();
        let output = frame(&ctx, &mut app, Vec::new());
        let menu = text_rect(&output, "Menu").expect("Menu button");

        let mut steps = 0;
        while !focused_rect(&ctx).is_some_and(|rect| rect.contains(menu.center())) {
            steps += 1;
            assert!(steps < 200, "Tab never reached the Menu button");
            press(&ctx, &mut app, Key::Tab);
        }
        let output = press(&ctx, &mut app, Key::Enter);
        let quick_nav = text_rect(&output, MenuAction::QuickNav.label()).expect("Enter opens the menu");
        let remote_hosts = text_rect(&output, MenuAction::RemoteHosts.label()).expect("Remote Hosts row");

        // The menu frame takes focus first, as in every egui menu, then its rows.
        let mut steps = 0;
        while !focused_rect(&ctx)
            .is_some_and(|rect| rect.contains(quick_nav.center()) && !rect.contains(remote_hosts.center()))
        {
            steps += 1;
            assert!(steps <= 2, "Tab does not move from Menu into its first row");
            press(&ctx, &mut app, Key::Tab);
        }
        press(&ctx, &mut app, Key::Enter);
        assert!(
            app.command_palette.is_some(),
            "Enter chooses the focused row, and the palette it opens does not run that Enter"
        );
    }
}
