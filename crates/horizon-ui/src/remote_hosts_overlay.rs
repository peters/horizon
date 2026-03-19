mod layout;
mod paint;
mod query;

use std::time::{Duration, Instant};

use egui::{
    Align, Color32, Context, CornerRadius, FontId, Id, Layout, Margin, Order, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, UiBuilder, Vec2,
};
use horizon_core::{RemoteHostCatalog, SshConnection};

use self::layout::{Columns, INPUT_HEIGHT, OverlayLayout, ROW_HEIGHT, columns, current_epoch_secs, overlay_layout};
use self::paint::{paint_empty, render_column_headers, render_host_row};
use self::query::{connect_action, filtered_indices, parse_user_prefix};
use crate::command_palette::render::paint_card;
use crate::theme;

pub struct RemoteHostsOverlay {
    query: String,
    selected: usize,
    opened_at: Instant,
}

pub enum RemoteHostsOverlayAction {
    None,
    Cancelled,
    OpenSsh { label: String, connection: SshConnection },
}

struct FrameContext<'a> {
    refresh_in_flight: bool,
    user_override: Option<&'a str>,
    now_secs: i64,
    /// Seconds until next auto-refresh, or `None` if refreshing or no timer.
    next_refresh_secs: Option<u64>,
}

impl RemoteHostsOverlay {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            opened_at: Instant::now(),
        }
    }

    pub fn show(
        &mut self,
        ctx: &Context,
        catalog: &RemoteHostCatalog,
        refresh_in_flight: bool,
        next_refresh_secs: Option<u64>,
    ) -> RemoteHostsOverlayAction {
        let (user_override, filter_query) = parse_user_prefix(&self.query);
        let user_override = user_override.map(str::to_string);
        let filtered = filtered_indices(&catalog.hosts, filter_query);
        let layout = overlay_layout(ctx.input(egui::InputState::viewport_rect));
        let frame = FrameContext {
            refresh_in_flight,
            user_override: user_override.as_deref(),
            now_secs: current_epoch_secs(),
            next_refresh_secs,
        };

        // Keep the countdown ticking while overlay is visible.
        ctx.request_repaint_after(Duration::from_secs(1));

        if self.show_backdrop(ctx, layout.screen) {
            return RemoteHostsOverlayAction::Cancelled;
        }

        self.clamp_selection(filtered.len());
        self.show_modal(ctx, catalog, &filtered, &layout, &frame)
    }

    fn clamp_selection(&mut self, count: usize) {
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    fn show_backdrop(&self, ctx: &Context, screen_rect: Rect) -> bool {
        let mut cancelled = false;
        egui::Area::new(Id::new("remote_hosts_backdrop"))
            .fixed_pos(screen_rect.min)
            .constrain(false)
            .order(Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen_rect.size(), Sense::click());
                ui.painter_at(rect)
                    .rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(140));
                if response.clicked() && self.opened_at.elapsed().as_millis() > 200 {
                    cancelled = true;
                }
            });
        cancelled
    }

    fn show_modal(
        &mut self,
        ctx: &Context,
        catalog: &RemoteHostCatalog,
        filtered: &[usize],
        layout: &OverlayLayout,
        frame: &FrameContext<'_>,
    ) -> RemoteHostsOverlayAction {
        let mut action = RemoteHostsOverlayAction::None;

        egui::Area::new(Id::new("remote_hosts_modal"))
            .fixed_pos(layout.card.min)
            .constrain(true)
            .order(Order::Debug)
            .show(ctx, |ui| {
                paint_card(ui, layout.card);

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(layout.inner)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        action = self.show_contents(ui, ctx, catalog, filtered, layout, frame);
                    },
                );
            });

        action
    }

    fn show_contents(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &Context,
        catalog: &RemoteHostCatalog,
        filtered: &[usize],
        layout: &OverlayLayout,
        frame: &FrameContext<'_>,
    ) -> RemoteHostsOverlayAction {
        let total = catalog.hosts.len();
        self.render_query_input(ui, layout.inner, filtered.len(), total, frame);
        if let Some(action) = self.handle_keyboard(ctx, catalog, filtered, frame.user_override) {
            return action;
        }

        ui.allocate_space(Vec2::new(layout.inner.width(), INPUT_HEIGHT));
        ui.add_space(6.0);

        let columns = columns(layout.inner.width());
        render_column_headers(ui, layout.inner.width(), &columns);

        let separator_rect = ui.allocate_space(Vec2::new(layout.inner.width(), 1.0)).1;
        ui.painter_at(separator_rect).rect_filled(
            separator_rect,
            CornerRadius::ZERO,
            theme::alpha(theme::BORDER_SUBTLE, 100),
        );
        ui.add_space(2.0);

        match self.render_results(ui, catalog, filtered, layout, &columns, frame.now_secs) {
            Some(index) => {
                let host = &catalog.hosts[filtered[index]];
                connect_action(host, frame.user_override)
            }
            None => RemoteHostsOverlayAction::None,
        }
    }

    fn render_query_input(
        &mut self,
        ui: &mut egui::Ui,
        inner_rect: Rect,
        filtered_count: usize,
        total_count: usize,
        frame: &FrameContext<'_>,
    ) {
        let input_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(inner_rect.width(), INPUT_HEIGHT));

        ui.painter()
            .rect_filled(input_rect, CornerRadius::same(12), theme::BG_ELEVATED);
        ui.painter().rect_stroke(
            input_rect,
            CornerRadius::same(12),
            Stroke::new(1.0, theme::alpha(theme::ACCENT, 70)),
            StrokeKind::Inside,
        );

        let text_rect = input_rect.shrink2(Vec2::new(14.0, 6.0));
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(text_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );

        child.label(
            RichText::new("SSH Remote")
                .font(FontId::proportional(13.0))
                .color(theme::FG_DIM)
                .strong(),
        );
        child.label(RichText::new(" > ").font(FontId::monospace(13.0)).color(theme::ACCENT));

        let response = child.add(
            egui::TextEdit::singleline(&mut self.query)
                .font(FontId::monospace(14.0))
                .text_color(theme::FG)
                .frame(false)
                .desired_width(text_rect.width() - 260.0)
                .hint_text(
                    RichText::new("type to filter, prefix user@ to connect as that user")
                        .color(theme::FG_DIM)
                        .font(FontId::monospace(11.0)),
                )
                .margin(Margin::ZERO),
        );
        if !response.has_focus() && self.opened_at.elapsed().as_millis() < 100 {
            response.request_focus();
        }
        if response.changed() {
            self.selected = 0;
        }

        child.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let refresh_status = if frame.refresh_in_flight {
                "refreshing...".to_string()
            } else if let Some(secs) = frame.next_refresh_secs {
                format!("{secs}s")
            } else {
                String::new()
            };
            let count_text = if refresh_status.is_empty() {
                format!("{filtered_count}/{total_count}")
            } else {
                format!("{filtered_count}/{total_count}  {refresh_status}")
            };
            ui.label(
                RichText::new(count_text)
                    .font(FontId::monospace(11.0))
                    .color(theme::FG_DIM),
            );
        });
    }

    fn handle_keyboard(
        &mut self,
        ctx: &Context,
        catalog: &RemoteHostCatalog,
        filtered: &[usize],
        user_override: Option<&str>,
    ) -> Option<RemoteHostsOverlayAction> {
        let (up, down, enter, escape) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.key_pressed(egui::Key::Enter),
                input.key_pressed(egui::Key::Escape),
            )
        });

        if escape {
            return Some(RemoteHostsOverlayAction::Cancelled);
        }
        if up && self.selected > 0 {
            self.selected -= 1;
        }
        if down && !filtered.is_empty() && self.selected < filtered.len() - 1 {
            self.selected += 1;
        }
        if enter && !filtered.is_empty() {
            let host = &catalog.hosts[filtered[self.selected]];
            return Some(connect_action(host, user_override));
        }

        None
    }

    fn render_results(
        &mut self,
        ui: &mut egui::Ui,
        catalog: &RemoteHostCatalog,
        filtered: &[usize],
        layout: &OverlayLayout,
        columns: &Columns,
        now_secs: i64,
    ) -> Option<usize> {
        if filtered.is_empty() {
            paint_empty(ui, "No matching hosts");
            return None;
        }

        let mut clicked_idx = None;
        let scroll_height = layout.results_height.min(layout.inner.max.y - ui.cursor().min.y - 8.0);

        ScrollArea::vertical()
            .max_height(scroll_height)
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, filtered.len(), |ui, row_range| {
                ui.set_min_width(layout.inner.width());

                for filtered_idx in row_range {
                    let host_idx = filtered[filtered_idx];
                    let host = &catalog.hosts[host_idx];
                    let is_selected = self.selected == filtered_idx;
                    if render_host_row(
                        ui,
                        layout.inner.width(),
                        filtered_idx,
                        host,
                        is_selected,
                        columns,
                        now_secs,
                    ) {
                        clicked_idx = Some(filtered_idx);
                    }
                }
            });

        clicked_idx
    }
}
