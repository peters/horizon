mod layout;
mod paint;
mod query;

use std::time::{Duration, Instant};

use egui::{
    Align, Color32, Context, CornerRadius, FontId, Id, Layout, Margin, Order, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, UiBuilder, Vec2,
};
use horizon_core::{RemoteHost, RemoteHostCatalog, RemoteHostConnectionSummary, SshConnection};

use self::layout::{Columns, INPUT_HEIGHT, OverlayLayout, columns, current_epoch_secs, overlay_layout};
use self::paint::{HostRowRenderContext, paint_empty, render_column_headers, render_host_details, render_host_row};
use self::query::{connect_action, filtered_indices, parse_user_prefix};
use crate::command_palette::render::paint_card;
use crate::theme;

pub struct RemoteHostsOverlay {
    query: String,
    selected: usize,
    expanded_host: Option<String>,
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

struct OverlayRenderContext<'a, 'b> {
    catalog: &'a RemoteHostCatalog,
    connection_summaries: &'a [RemoteHostConnectionSummary],
    filtered: &'a [usize],
    layout: &'a OverlayLayout,
    frame: &'b FrameContext<'a>,
}

impl RemoteHostsOverlay {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            expanded_host: None,
            opened_at: Instant::now(),
        }
    }

    pub fn show(
        &mut self,
        ctx: &Context,
        catalog: &RemoteHostCatalog,
        connection_summaries: &[RemoteHostConnectionSummary],
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
        let render = OverlayRenderContext {
            catalog,
            connection_summaries,
            filtered: &filtered,
            layout: &layout,
            frame: &frame,
        };

        // Keep the countdown ticking while overlay is visible.
        ctx.request_repaint_after(Duration::from_secs(1));

        if self.show_backdrop(ctx, layout.screen) {
            return RemoteHostsOverlayAction::Cancelled;
        }

        self.clamp_selection(filtered.len());
        self.show_modal(ctx, &render)
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

    fn show_modal(&mut self, ctx: &Context, render: &OverlayRenderContext<'_, '_>) -> RemoteHostsOverlayAction {
        let mut action = RemoteHostsOverlayAction::None;

        egui::Area::new(Id::new("remote_hosts_modal"))
            .fixed_pos(render.layout.card.min)
            .constrain(true)
            .order(Order::Debug)
            .show(ctx, |ui| {
                paint_card(ui, render.layout.card);

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(render.layout.inner)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        action = self.show_contents(ui, ctx, render);
                    },
                );
            });

        action
    }

    fn show_contents(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &Context,
        render: &OverlayRenderContext<'_, '_>,
    ) -> RemoteHostsOverlayAction {
        let total = render.catalog.hosts.len();
        self.render_query_input(ui, render.layout.inner, render.filtered.len(), total, render.frame);
        if let Some(action) = self.handle_keyboard(ctx, render.catalog, render.filtered, render.frame.user_override) {
            return action;
        }

        ui.allocate_space(Vec2::new(render.layout.inner.width(), INPUT_HEIGHT));
        ui.add_space(6.0);

        let columns = columns(render.layout.inner.width());
        render_column_headers(ui, render.layout.inner.width(), &columns);

        let separator_rect = ui.allocate_space(Vec2::new(render.layout.inner.width(), 1.0)).1;
        ui.painter_at(separator_rect).rect_filled(
            separator_rect,
            CornerRadius::ZERO,
            theme::alpha(theme::BORDER_SUBTLE, 100),
        );
        ui.add_space(2.0);

        match self.render_results(ui, &columns, render) {
            Some(index) => {
                let host = &render.catalog.hosts[render.filtered[index]];
                connect_action(host, render.frame.user_override)
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
        columns: &Columns,
        render: &OverlayRenderContext<'_, '_>,
    ) -> Option<usize> {
        if render.filtered.is_empty() {
            paint_empty(ui, "No matching hosts");
            return None;
        }

        let mut clicked_idx = None;
        let scroll_height = render
            .layout
            .results_height
            .min(render.layout.inner.max.y - ui.cursor().min.y - 8.0);

        ScrollArea::vertical()
            .max_height(scroll_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(render.layout.inner.width());

                for (filtered_idx, host_idx) in render.filtered.iter().copied().enumerate() {
                    let host = &render.catalog.hosts[host_idx];
                    let summary = &render.connection_summaries[host_idx];
                    let is_selected = self.selected == filtered_idx;
                    let is_expanded = self.is_expanded(host);
                    let interaction = render_host_row(
                        ui,
                        &HostRowRenderContext {
                            width: render.layout.inner.width(),
                            index: filtered_idx,
                            host,
                            summary,
                            is_selected,
                            is_expanded,
                            columns,
                            now_secs: render.frame.now_secs,
                        },
                    );
                    if interaction.select {
                        self.selected = filtered_idx;
                    }
                    if interaction.toggle_expand {
                        self.toggle_expanded(host);
                    }
                    if interaction.connect {
                        clicked_idx = Some(filtered_idx);
                    }
                    if is_expanded {
                        render_host_details(
                            ui,
                            render.layout.inner.width() - 4.0,
                            host,
                            summary,
                            render.frame.now_secs,
                        );
                        ui.add_space(6.0);
                    }
                }
            });

        clicked_idx
    }

    fn is_expanded(&self, host: &RemoteHost) -> bool {
        self.expanded_host.as_deref() == Some(&host.ssh_connection.display_label())
    }

    fn toggle_expanded(&mut self, host: &RemoteHost) {
        let key = host.ssh_connection.display_label();
        if self.expanded_host.as_deref() == Some(key.as_str()) {
            self.expanded_host = None;
        } else {
            self.expanded_host = Some(key);
        }
    }
}
