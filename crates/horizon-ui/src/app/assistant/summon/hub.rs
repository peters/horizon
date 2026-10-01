//! Where Quick Nav, Remote Hosts, Cloud, Sessions and Settings live once Horizon has no
//! window of its own to hang them on.
//!
//! The command bar carries a small row of buttons for them. What opens is shown three
//! ways, switched live to compare:
//!
//! - **Satellites**: each page is a native window of its own that appears just above the
//!   bar, with a small arrow pointing at the button it came from.
//! - **Hub**: one native window with a tab for each page.
//! - **Inline**: the page replaces the conversation inside the bar's own window.

use std::collections::HashMap;
use std::time::Instant;

use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, ViewportBuilder,
    ViewportId, pos2, vec2,
};

use super::super::num;
use super::HorizonApp;
use crate::theme;

mod pages;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::app) enum HubPage {
    Nav,
    Hosts,
    Cloud,
    Sessions,
    Settings,
}

impl HubPage {
    pub(super) const ALL: [Self; 5] = [Self::Nav, Self::Hosts, Self::Cloud, Self::Sessions, Self::Settings];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Nav => "Quick nav",
            Self::Hosts => "Remote hosts",
            Self::Cloud => "Cloud",
            Self::Sessions => "Sessions",
            Self::Settings => "Settings",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Nav => "Horizon Quick Nav",
            Self::Hosts => "Horizon Remote Hosts",
            Self::Cloud => "Horizon Cloud",
            Self::Sessions => "Horizon Sessions",
            Self::Settings => "Horizon Settings",
        }
    }

    fn app_id(self) -> &'static str {
        match self {
            Self::Nav => "horizon-hub-nav",
            Self::Hosts => "horizon-hub-hosts",
            Self::Cloud => "horizon-hub-cloud",
            Self::Sessions => "horizon-hub-sessions",
            Self::Settings => "horizon-hub-settings",
        }
    }

    /// Window size as a satellite.
    fn size(self) -> [f32; 2] {
        match self {
            Self::Nav => [660.0, 470.0],
            Self::Hosts => [720.0, 500.0],
            Self::Cloud => [720.0, 470.0],
            Self::Sessions => [640.0, 420.0],
            Self::Settings => [560.0, 400.0],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum HubStyle {
    /// A window for each page, with an arrow to its button.
    #[default]
    Satellites,
    /// One window with tabs.
    Window,
    /// Inside the command bar.
    Inline,
}

impl HubStyle {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Satellites => "A  Satellites",
            Self::Window => "B  Hub",
            Self::Inline => "C  Inline",
        }
    }
}

const HUB_TITLE: &str = "Horizon Hub";
const HUB_SIZE: [f32; 2] = [880.0, 580.0];
const CARET: f32 = 14.0;
const SETTLE_SECS: f32 = 3.0;

#[derive(Default)]
pub(in crate::app::assistant) struct Hub {
    pub(super) style: HubStyle,
    /// The page that is open, if any.
    pub(super) page: Option<HubPage>,
    opened_at: Option<Instant>,
    last_placed: Option<Instant>,
    /// Where each button was drawn in the bar window, so a satellite can point at it.
    buttons: HashMap<HubPage, Rect>,
    pub(super) query: String,
    /// A cloud machine the person allowed the assistant's agent to stop.
    pub(super) stopped: bool,
    pub(super) refresh_requested: bool,
}

impl Hub {
    pub(in crate::app::assistant) fn open(&mut self, page: HubPage) {
        if self.page != Some(page) {
            self.opened_at = Some(Instant::now());
            self.last_placed = None;
            self.query.clear();
        }
        self.page = Some(page);
    }

    pub(in crate::app::assistant) fn close(&mut self) {
        self.page = None;
    }

    pub(in crate::app::assistant) fn toggle(&mut self, page: HubPage) {
        if self.page == Some(page) {
            self.close();
        } else {
            self.open(page);
        }
    }

    pub(in crate::app::assistant) fn mark_stopped(&mut self) {
        self.stopped = true;
    }
}

impl HorizonApp {
    /// The row of buttons under the bar's prompt. Returns the page asked for, if a button was pressed.
    pub(super) fn hub_rail(&mut self, ui: &mut Ui) {
        let ids = HubPage::ALL;
        let gap = 8.0;
        let widths: Vec<f32> = ids
            .iter()
            .map(|page| {
                ui.painter()
                    .layout_no_wrap(page.label().to_string(), FontId::proportional(12.5), theme::FG())
                    .size()
                    .x
                    + 28.0
            })
            .collect();
        let total: f32 = widths.iter().sum::<f32>() + gap * num::count(ids.len() - 1);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
        let mut x = row.center().x - total / 2.0;
        let mut pressed = None;
        let mut restyle = None;
        for (page, width) in ids.into_iter().zip(widths) {
            let rect = Rect::from_min_size(pos2(x, row.top() + 3.0), vec2(width, 28.0));
            x += width + gap;
            self.assistant.summon.hub.buttons.insert(page, rect);
            let selected = self.assistant.summon.hub.page == Some(page);
            let response = ui.interact(rect, Id::new(("hub_button", page.label())), Sense::click());
            let hovered = response.hovered();
            let fill = if selected {
                theme::ACCENT().gamma_multiply(0.22)
            } else if hovered {
                theme::BORDER_SUBTLE()
            } else {
                theme::PANEL_BG_ALT()
            };
            ui.painter().rect(
                rect,
                CornerRadius::same(9),
                fill,
                Stroke::new(
                    1.0,
                    if selected {
                        theme::ACCENT().gamma_multiply(0.7)
                    } else {
                        theme::BORDER_SUBTLE()
                    },
                ),
                StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                page.label(),
                FontId::proportional(12.5),
                if selected { theme::FG() } else { theme::FG_SOFT() },
            );
            if response.clicked() {
                pressed = Some(page);
            }
            // Right-click: how the pages open.
            response.context_menu(|ui| {
                for style in [HubStyle::Satellites, HubStyle::Window, HubStyle::Inline] {
                    if ui.button(style.label()).clicked() {
                        restyle = Some(style);
                        ui.close();
                    }
                }
            });
        }
        if let Some(style) = restyle {
            self.assistant.summon.hub.style = style;
            self.assistant.summon.hub.close();
        }
        if let Some(page) = pressed {
            self.press_hub_button(page);
        }
        // A scripted click draws a ripple on the button it pressed.
        if let Some((page, progress)) = self.assistant.demo.as_ref().and_then(super::demo::Demo::page_press)
            && let Some(rect) = self.assistant.summon.hub.buttons.get(&page)
        {
            super::desk_bar::paint::paint_click(ui, rect.center(), progress);
        }
    }

    /// What a button does depends on the design.
    pub(in crate::app::assistant) fn press_hub_button(&mut self, page: HubPage) {
        self.assistant.summon.hub.toggle(page);
        if self.assistant.summon.hub.style == HubStyle::Inline && self.assistant.summon.hub.page.is_some() {
            self.assistant.summon.expanded = true;
            self.assistant.summon.style = super::ExpandStyle::Sheet;
        }
        if page == HubPage::Hosts {
            self.assistant.summon.hub.refresh_requested = true;
        }
    }

    /// The native windows of the satellite and hub designs. Called every frame with the bar.
    pub(super) fn render_hub_windows(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.assistant.summon.hub.refresh_requested) {
            self.refresh_remote_hosts_for_hub();
        }
        let hub = &self.assistant.summon.hub;
        let Some(page) = hub.page else {
            return;
        };
        match hub.style {
            HubStyle::Satellites => self.render_satellite(ctx, page),
            HubStyle::Window => self.render_hub_window(ctx, page),
            HubStyle::Inline => {}
        }
    }

    fn render_satellite(&mut self, ctx: &egui::Context, page: HubPage) {
        let size = page.size();
        let [bar_x, bar_y, _, _] = self.assistant.summon.bar_rect.unwrap_or([0.0, 700.0, 980.0, 280.0]);
        let button = self.assistant.summon.hub.buttons.get(&page).copied();
        let tether = button.map_or(size[0] / 2.0, |rect| rect.center().x);
        let monitor = ctx
            .input(|input| input.viewport().monitor_size)
            .map_or(super::desk_bar::MONITOR, |size| [size.x, size.y]);
        let want_x = (bar_x + tether - size[0] / 2.0).clamp(12.0, monitor[0] - size[0] - 12.0);
        // The arrow points at the button; the window's own x tells it where that is.
        let arrow_at = (bar_x + tether - want_x).clamp(30.0, size[0] - 30.0);
        let want_y = bar_y - size[1] - 10.0;
        self.place_hub_window(page.title(), [want_x, want_y, size[0], size[1]]);
        let builder = ViewportBuilder::default()
            .with_title(page.title())
            .with_app_id(page.app_id())
            .with_decorations(false)
            .with_transparent(true)
            .with_inner_size(size)
            .with_min_inner_size([200.0, 120.0])
            .with_resizable(false);
        let viewport = ViewportId(Id::new(("hub_satellite", page.label())));
        let mut close = false;
        ctx.show_viewport_immediate(viewport, builder, |ui, _class| {
            let full = ui.max_rect();
            let card = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - CARET));
            paint_card(ui, card, Some(arrow_at));
            let inner = card.shrink2(vec2(20.0, 16.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(inner));
            close = Self::hub_header(&mut child, page, true);
            self.hub_page(&mut child, page);
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });
        if close {
            self.assistant.summon.hub.close();
        }
    }

    fn render_hub_window(&mut self, ctx: &egui::Context, page: HubPage) {
        let [bar_x, bar_y, bar_w, _] = self.assistant.summon.bar_rect.unwrap_or([470.0, 700.0, 980.0, 280.0]);
        let x = bar_x + (bar_w - HUB_SIZE[0]) / 2.0;
        self.place_hub_window(HUB_TITLE, [x, bar_y - HUB_SIZE[1] - 10.0, HUB_SIZE[0], HUB_SIZE[1]]);
        let builder = ViewportBuilder::default()
            .with_title(HUB_TITLE)
            .with_app_id("horizon-hub")
            .with_decorations(false)
            .with_transparent(true)
            .with_inner_size(HUB_SIZE)
            .with_min_inner_size([200.0, 120.0])
            .with_resizable(false);
        let viewport = ViewportId(Id::new("hub_window"));
        let mut close = false;
        let mut switch = None;
        ctx.show_viewport_immediate(viewport, builder, |ui, _class| {
            let full = ui.max_rect();
            paint_card(ui, full, None);
            let inner = full.shrink2(vec2(20.0, 16.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(inner));
            // The tab strip.
            let strip = Rect::from_min_size(inner.min, vec2(inner.width(), 34.0));
            let mut x = strip.left();
            for tab in HubPage::ALL {
                let width = child
                    .painter()
                    .layout_no_wrap(tab.label().to_string(), FontId::proportional(13.0), theme::FG())
                    .size()
                    .x
                    + 28.0;
                let rect = Rect::from_min_size(pos2(x, strip.top()), vec2(width, 30.0));
                x += width + 6.0;
                let selected = tab == page;
                let response = child.interact(rect, Id::new(("hub_tab", tab.label())), Sense::click());
                if selected {
                    child
                        .painter()
                        .rect_filled(rect, CornerRadius::same(9), theme::ACCENT().gamma_multiply(0.22));
                } else if response.hovered() {
                    child
                        .painter()
                        .rect_filled(rect, CornerRadius::same(9), theme::PANEL_BG_ALT());
                }
                child.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    tab.label(),
                    FontId::proportional(13.0),
                    if selected { theme::FG() } else { theme::FG_SOFT() },
                );
                if response.clicked() {
                    switch = Some(tab);
                }
            }
            let close_rect = Rect::from_center_size(pos2(strip.right() - 14.0, strip.center().y), vec2(26.0, 26.0));
            if close_button(&mut child, close_rect) {
                close = true;
            }
            child.painter().hline(
                inner.x_range(),
                strip.bottom() + 6.0,
                Stroke::new(1.0, theme::BORDER_SUBTLE()),
            );
            let mut body = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(
                pos2(inner.left(), strip.bottom() + 14.0),
                inner.right_bottom(),
            )));
            self.hub_page(&mut body, page);
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });
        if let Some(tab) = switch {
            self.assistant.summon.hub.open(tab);
            if tab == HubPage::Hosts {
                self.assistant.summon.hub.refresh_requested = true;
            }
        }
        if close {
            self.assistant.summon.hub.close();
        }
    }

    /// Asks the shell to put the window where it belongs, for the first moments after it appears.
    fn place_hub_window(&mut self, title: &str, rect: [f32; 4]) {
        let hub = &mut self.assistant.summon.hub;
        let settling = hub.opened_at.is_none_or(|at| at.elapsed().as_secs_f32() < SETTLE_SECS);
        let due = hub.last_placed.is_none_or(|at| at.elapsed().as_millis() > 400);
        if !(settling && due) {
            return;
        }
        hub.last_placed = Some(Instant::now());
        #[allow(clippy::cast_possible_truncation)]
        let rect = rect.map(|value| value.round() as i32);
        if let Some(desk) = self.assistant.desk.as_ref() {
            desk.place_titled(title, rect);
        }
    }

    /// Title row of a page. Returns true when its close button was pressed.
    fn hub_header(ui: &mut Ui, page: HubPage, closable: bool) -> bool {
        let width = ui.max_rect().width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
        ui.painter().text(
            rect.left_center(),
            Align2::LEFT_CENTER,
            page.label(),
            FontId::proportional(17.0),
            theme::FG(),
        );
        let mut close = false;
        if closable {
            let button = Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), vec2(26.0, 26.0));
            close = close_button(ui, button);
        }
        ui.add_space(6.0);
        close
    }

    /// A page inside the bar, on the same recessed panel as the conversation.
    pub(super) fn inline_page(&mut self, ui: &mut Ui, height: f32, page: HubPage) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(14), theme::BG());
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(14),
            Stroke::new(1.0, theme::BORDER_SUBTLE()),
            StrokeKind::Inside,
        );
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(18.0, 14.0))));
        if Self::hub_header(&mut child, page, true) {
            self.assistant.summon.hub.close();
        }
        self.hub_page(&mut child, page);
    }

    /// The page's content, filling the space the caller gave.
    pub(super) fn hub_page(&mut self, ui: &mut Ui, page: HubPage) {
        match page {
            HubPage::Nav => self.page_nav(ui),
            HubPage::Hosts => self.page_hosts(ui),
            HubPage::Cloud => self.page_cloud(ui),
            HubPage::Sessions => self.page_sessions(ui),
            HubPage::Settings => self.page_settings(ui),
        }
    }
}

/// The plate of a satellite or hub window: rounded, with an arrow at the bottom edge when
/// `arrow_at` says where the button is.
fn paint_card(ui: &Ui, card: Rect, arrow_at: Option<f32>) {
    let painter = ui.painter();
    let radius = CornerRadius::same(20);
    let edge = theme::ACCENT().gamma_multiply(0.55);
    painter.rect_filled(card, radius, theme::PANEL_BG());
    painter.rect_stroke(card, radius, Stroke::new(1.2, edge), StrokeKind::Inside);
    if let Some(at) = arrow_at {
        let x = card.left() + at;
        let base = card.bottom() - 1.0;
        let triangle = vec![pos2(x - 12.0, base), pos2(x + 12.0, base), pos2(x, base + CARET)];
        painter.add(Shape::convex_polygon(triangle.clone(), theme::PANEL_BG(), Stroke::NONE));
        painter.add(Shape::line(
            vec![triangle[0], triangle[2], triangle[1]],
            Stroke::new(1.2, edge),
        ));
        // Hide the card's edge between the arrow's feet.
        painter.line_segment(
            [pos2(x - 11.0, base), pos2(x + 11.0, base)],
            Stroke::new(2.0, theme::PANEL_BG()),
        );
    }
}

fn close_button(ui: &mut Ui, rect: Rect) -> bool {
    let response = ui.interact(
        rect,
        Id::new(("hub_close", super::super::num::whole(rect.min.x))),
        Sense::click(),
    );
    if response.hovered() {
        ui.painter()
            .circle_filled(rect.center(), rect.width() / 2.0, theme::PANEL_BG_ALT());
    }
    let c = rect.center();
    let stroke = Stroke::new(1.6, theme::FG_SOFT());
    ui.painter()
        .line_segment([c + vec2(-4.5, -4.5), c + vec2(4.5, 4.5)], stroke);
    ui.painter()
        .line_segment([c + vec2(-4.5, 4.5), c + vec2(4.5, -4.5)], stroke);
    response.clicked()
}

/// A colour for a status word.
fn status_color(word: &str) -> Color32 {
    match word {
        "Running" | "Online" | "Current" => theme::PALETTE_GREEN(),
        "Stopped" | "Offline" => theme::PALETTE_RED(),
        "Idle" => theme::PALETTE_YELLOW(),
        _ => theme::FG_DIM(),
    }
}
