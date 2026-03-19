use std::time::Instant;

use egui::{
    Align, Color32, Context, CornerRadius, FontId, Id, Layout, Margin, Order, Pos2, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, UiBuilder, Vec2,
};
use horizon_core::{RemoteHost, RemoteHostCatalog, RemoteHostStatus, SshConnection};

use crate::app::util::usize_to_f32;
use crate::command_palette::render::paint_card;
use crate::theme;

const OVERLAY_WIDTH: f32 = 1020.0;
const INPUT_HEIGHT: f32 = 44.0;
const HEADER_ROW_HEIGHT: f32 = 26.0;
const ROW_HEIGHT: f32 = 28.0;
const MAX_VISIBLE_ROWS: usize = 20;

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

struct OverlayLayout {
    screen: Rect,
    card: Rect,
    inner: Rect,
    results_height: f32,
}

struct FrameContext<'a> {
    refresh_in_flight: bool,
    user_override: Option<&'a str>,
    now_secs: i64,
}

struct Columns {
    alias: f32,
    ipv4: f32,
    tags: f32,
    hostname: f32,
    status: f32,
    last_seen: f32,
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
    ) -> RemoteHostsOverlayAction {
        let (user_override, filter_query) = parse_user_prefix(&self.query);
        let user_override = user_override.map(str::to_string);
        let filtered = filtered_indices(&catalog.hosts, filter_query);
        let layout = overlay_layout(ctx.input(egui::InputState::viewport_rect));
        let fctx = FrameContext {
            refresh_in_flight,
            user_override: user_override.as_deref(),
            now_secs: current_epoch_secs(),
        };

        if self.show_backdrop(ctx, layout.screen) {
            return RemoteHostsOverlayAction::Cancelled;
        }

        self.clamp_selection(filtered.len());
        self.show_modal(ctx, catalog, &filtered, &layout, &fctx)
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
        fctx: &FrameContext<'_>,
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
                        action = self.show_contents(ui, ctx, catalog, filtered, layout, fctx);
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
        fctx: &FrameContext<'_>,
    ) -> RemoteHostsOverlayAction {
        let total = catalog.hosts.len();
        self.render_query_input(ui, layout.inner, filtered.len(), total, fctx.refresh_in_flight);
        if let Some(action) = self.handle_keyboard(ctx, catalog, filtered, fctx.user_override) {
            return action;
        }

        ui.allocate_space(Vec2::new(layout.inner.width(), INPUT_HEIGHT));
        ui.add_space(6.0);

        let cols = columns(layout.inner.width());
        render_column_headers(ui, layout.inner.width(), &cols);

        // Separator
        let sep_rect = ui.allocate_space(Vec2::new(layout.inner.width(), 1.0)).1;
        ui.painter_at(sep_rect)
            .rect_filled(sep_rect, CornerRadius::ZERO, theme::alpha(theme::BORDER_SUBTLE, 100));
        ui.add_space(2.0);

        match self.render_results(ui, catalog, filtered, layout, &cols, fctx.now_secs) {
            Some(index) => {
                let host = &catalog.hosts[filtered[index]];
                connect_action(host, fctx.user_override)
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
        refresh_in_flight: bool,
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
            let count_text = if refresh_in_flight {
                format!("{filtered_count}/{total_count} refreshing...")
            } else {
                format!("{filtered_count}/{total_count}")
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
        cols: &Columns,
        now_secs: i64,
    ) -> Option<usize> {
        let mut clicked_idx = None;
        let scroll_height = layout.results_height.min(layout.inner.max.y - ui.cursor().min.y - 8.0);

        ScrollArea::vertical()
            .max_height(scroll_height)
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, filtered.len(), |ui, row_range| {
                ui.set_min_width(layout.inner.width());

                if filtered.is_empty() {
                    paint_empty(ui, "No matching hosts");
                    return;
                }

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
                        cols,
                        now_secs,
                    ) {
                        clicked_idx = Some(filtered_idx);
                    }
                }
            });

        clicked_idx
    }
}

// -- Layout -------------------------------------------------------------------

fn current_epoch_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}

fn overlay_layout(screen: Rect) -> OverlayLayout {
    let width = OVERLAY_WIDTH.min(screen.width() * 0.92);
    let results_height = usize_to_f32(MAX_VISIBLE_ROWS) * ROW_HEIGHT;
    let card_height = INPUT_HEIGHT + 16.0 + HEADER_ROW_HEIGHT + results_height + 56.0;
    let card_min = Pos2::new((screen.width() - width) * 0.5, (screen.height() - card_height) * 0.22);
    let card = Rect::from_min_size(card_min, Vec2::new(width, card_height));

    OverlayLayout {
        screen,
        inner: card.shrink2(Vec2::new(20.0, 16.0)),
        card,
        results_height,
    }
}

fn columns(content_width: f32) -> Columns {
    // Proportional column layout that adapts to overlay width
    let alias_frac = 0.25;
    let ipv4_frac = 0.14;
    let tags_frac = 0.24;
    let hostname_frac = 0.16;
    let status_frac = 0.06;
    // last_seen gets the remainder

    let x0 = 16.0; // left padding for status indicator
    Columns {
        alias: x0,
        ipv4: x0 + content_width * alias_frac,
        tags: x0 + content_width * (alias_frac + ipv4_frac),
        hostname: x0 + content_width * (alias_frac + ipv4_frac + tags_frac),
        status: x0 + content_width * (alias_frac + ipv4_frac + tags_frac + hostname_frac),
        last_seen: x0 + content_width * (alias_frac + ipv4_frac + tags_frac + hostname_frac + status_frac),
    }
}

// -- Painting -----------------------------------------------------------------

fn render_column_headers(ui: &mut egui::Ui, width: f32, cols: &Columns) {
    let rect = ui.allocate_space(Vec2::new(width, HEADER_ROW_HEIGHT)).1;
    let painter = ui.painter_at(rect);
    let y = rect.center().y;
    let x = rect.min.x;
    let font = FontId::monospace(10.0);
    let color = theme::FG_DIM;

    // Underline each header
    let underline_stroke = Stroke::new(1.0, theme::alpha(theme::ACCENT, 40));
    let headers: &[(&str, f32)] = &[
        ("Alias", cols.alias),
        ("IPv4", cols.ipv4),
        ("Tags", cols.tags),
        ("Hostname", cols.hostname),
        ("On", cols.status),
        ("Last seen", cols.last_seen),
    ];
    for &(label, col_x) in headers {
        let text_rect = painter.text(
            Pos2::new(x + col_x, y),
            egui::Align2::LEFT_CENTER,
            label,
            font.clone(),
            color,
        );
        painter.line_segment(
            [
                Pos2::new(text_rect.min.x, text_rect.max.y + 1.0),
                Pos2::new(text_rect.max.x, text_rect.max.y + 1.0),
            ],
            underline_stroke,
        );
    }
}

fn render_host_row(
    ui: &mut egui::Ui,
    width: f32,
    index: usize,
    host: &RemoteHost,
    is_selected: bool,
    cols: &Columns,
    now_secs: i64,
) -> bool {
    let row_rect = ui.allocate_space(Vec2::new(width, ROW_HEIGHT)).1;
    let mut clicked = false;

    // Selection / hover highlight
    if is_selected {
        ui.painter_at(row_rect).rect_filled(
            row_rect,
            CornerRadius::same(4),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.28), 200),
        );
    } else {
        let hover = ui
            .interact(row_rect, ui.make_persistent_id(("rh_hover", index)), Sense::hover())
            .hovered();
        if hover {
            ui.painter_at(row_rect).rect_filled(
                row_rect,
                CornerRadius::same(4),
                theme::alpha(theme::PANEL_BG_ALT, 120),
            );
        }
    }

    let click = ui.interact(row_rect, ui.make_persistent_id(("rh_click", index)), Sense::click());
    if click.double_clicked() || (click.clicked() && is_selected) {
        clicked = true;
    }

    let painter = ui.painter_at(row_rect);
    let y = row_rect.center().y;
    let x = row_rect.min.x;
    let mono = FontId::monospace(12.0);
    let mono_sm = FontId::monospace(11.0);

    // Status indicator bar (thin colored bar on left)
    let status_color = match host.status {
        RemoteHostStatus::Online => theme::PALETTE_GREEN,
        RemoteHostStatus::Offline => theme::PALETTE_RED,
        RemoteHostStatus::Unknown => theme::FG_DIM,
    };
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(x + 4.0, row_rect.min.y + 5.0),
            Vec2::new(3.0, ROW_HEIGHT - 10.0),
        ),
        CornerRadius::same(2),
        status_color,
    );

    // Alias (colored by status)
    let alias_color = match host.status {
        RemoteHostStatus::Online => Color32::from_rgb(166, 227, 161),
        RemoteHostStatus::Offline => Color32::from_rgb(249, 226, 175),
        RemoteHostStatus::Unknown => theme::FG_SOFT,
    };
    painter.text(
        Pos2::new(x + cols.alias, y),
        egui::Align2::LEFT_CENTER,
        &host.label,
        mono.clone(),
        alias_color,
    );

    // IPv4 (first IP)
    let ip = host.ips.first().map_or("-", String::as_str);
    painter.text(
        Pos2::new(x + cols.ipv4, y),
        egui::Align2::LEFT_CENTER,
        ip,
        mono_sm.clone(),
        theme::FG_SOFT,
    );

    // Tags (colored)
    render_tags(
        &painter,
        Pos2::new(x + cols.tags, y),
        &host.tags,
        &mono_sm,
        x + cols.hostname - 12.0,
    );

    // Hostname
    let hostname = host.hostname.as_deref().unwrap_or("-");
    painter.text(
        Pos2::new(x + cols.hostname, y),
        egui::Align2::LEFT_CENTER,
        hostname,
        mono_sm.clone(),
        theme::FG_DIM,
    );

    // Status (yes/no)
    let (status_text, status_text_color) = match host.status {
        RemoteHostStatus::Online => ("yes", theme::PALETTE_GREEN),
        RemoteHostStatus::Offline => ("no", theme::PALETTE_RED),
        RemoteHostStatus::Unknown => ("-", theme::FG_DIM),
    };
    painter.text(
        Pos2::new(x + cols.status, y),
        egui::Align2::LEFT_CENTER,
        status_text,
        mono_sm.clone(),
        status_text_color,
    );

    // Last seen (human-friendly relative time)
    let last_seen_display = host
        .last_seen_secs
        .map_or_else(|| "-".to_string(), |secs| format_relative_time(secs, now_secs));
    painter.text(
        Pos2::new(x + cols.last_seen, y),
        egui::Align2::LEFT_CENTER,
        &last_seen_display,
        mono_sm,
        theme::FG_DIM,
    );

    clicked
}

fn render_tags(painter: &egui::Painter, pos: Pos2, tags: &[String], font: &FontId, max_x: f32) {
    let mut x = pos.x;
    for (i, tag) in tags.iter().enumerate() {
        if x >= max_x {
            break;
        }
        if i > 0 {
            let comma_rect = painter.text(
                Pos2::new(x, pos.y),
                egui::Align2::LEFT_CENTER,
                ",",
                font.clone(),
                theme::FG_DIM,
            );
            x = comma_rect.max.x;
        }
        let color = tag_color(tag);
        let text_rect = painter.text(Pos2::new(x, pos.y), egui::Align2::LEFT_CENTER, tag, font.clone(), color);
        x = text_rect.max.x;
    }
}

fn paint_empty(ui: &mut egui::Ui, message: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(message).color(theme::FG_DIM).size(13.0));
    });
}

// -- Time formatting ----------------------------------------------------------

/// Format epoch seconds as human-friendly relative time ("2 min ago", "3d ago").
fn format_relative_time(epoch_secs: i64, now_secs: i64) -> String {
    let delta = now_secs.saturating_sub(epoch_secs);

    if delta < 0 {
        "-".to_string()
    } else if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{} min ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}

// -- Query parsing ------------------------------------------------------------

/// Split `user@filter` into an optional user override and the filter text.
/// If the query has no `@`, the entire string is the filter.
fn parse_user_prefix(query: &str) -> (Option<&str>, &str) {
    if let Some(at_pos) = query.find('@') {
        let user = query[..at_pos].trim();
        let filter = query[at_pos + 1..].trim();
        if user.is_empty() {
            (None, filter)
        } else {
            (Some(user), filter)
        }
    } else {
        (None, query)
    }
}

fn connect_action(host: &RemoteHost, user_override: Option<&str>) -> RemoteHostsOverlayAction {
    let mut connection = host.ssh_connection.clone();
    if let Some(user) = user_override {
        connection.user = Some(user.to_string());
    }
    RemoteHostsOverlayAction::OpenSsh {
        label: host.label.clone(),
        connection,
    }
}

// -- Filtering ----------------------------------------------------------------

fn filtered_indices(hosts: &[RemoteHost], query: &str) -> Vec<usize> {
    let query = query.trim().to_ascii_lowercase();
    hosts
        .iter()
        .enumerate()
        .filter(|(_, host)| query.is_empty() || host_matches(&query, host))
        .map(|(i, _)| i)
        .collect()
}

fn host_matches(query: &str, host: &RemoteHost) -> bool {
    contains_lowercase(host.label.as_bytes(), query.as_bytes())
        || contains_lowercase(host.ssh_connection.host.as_bytes(), query.as_bytes())
        || host
            .hostname
            .as_deref()
            .is_some_and(|h| contains_lowercase(h.as_bytes(), query.as_bytes()))
        || host
            .os
            .as_deref()
            .is_some_and(|os| contains_lowercase(os.as_bytes(), query.as_bytes()))
        || contains_lowercase(host.sources.label().as_bytes(), query.as_bytes())
        || contains_lowercase(host.status.label().as_bytes(), query.as_bytes())
        || host
            .tags
            .iter()
            .chain(host.ips.iter())
            .any(|v| contains_lowercase(v.as_bytes(), query.as_bytes()))
}

/// Case-insensitive substring search without allocation.
/// Assumes `needle` is already ASCII-lowercased.
fn contains_lowercase(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .windows(needle.len())
        .any(|window| window.iter().zip(needle).all(|(h, n)| h.to_ascii_lowercase() == *n))
}

// -- Tag colors ---------------------------------------------------------------

const TAG_COLORS: [Color32; 8] = [
    Color32::from_rgb(249, 226, 175), // yellow
    Color32::from_rgb(137, 180, 250), // blue
    Color32::from_rgb(166, 227, 161), // green
    Color32::from_rgb(243, 139, 168), // pink
    Color32::from_rgb(202, 151, 234), // magenta
    Color32::from_rgb(102, 212, 214), // cyan
    Color32::from_rgb(233, 190, 109), // amber
    Color32::from_rgb(147, 187, 255), // light blue
];

fn tag_color(tag: &str) -> Color32 {
    let hash = tag
        .bytes()
        .fold(0_u32, |acc, b| acc.wrapping_mul(31).wrapping_add(u32::from(b)));
    TAG_COLORS[(hash as usize) % TAG_COLORS.len()]
}
