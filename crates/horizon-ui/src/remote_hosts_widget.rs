use std::time::{Duration, Instant};

use egui::{Align, Button, Color32, ComboBox, CornerRadius, FontId, Layout, RichText, ScrollArea, Sense, Stroke, Vec2};
use horizon_core::{Panel, RemoteHost, RemoteHostStatus};

use crate::theme;

const HEADER_HEIGHT: f32 = 68.0;
const SEARCH_HEIGHT: f32 = 54.0;
const ERROR_HEIGHT: f32 = 42.0;
const ROW_HEIGHT: f32 = 64.0;
const FOOTER_HEIGHT: f32 = 96.0;
const BADGE_CORNER: u8 = 7;
const REFRESH_OPTIONS: [(&str, Option<Duration>); 5] = [
    ("15s", Some(Duration::from_secs(15))),
    ("30s", Some(Duration::from_secs(30))),
    ("1m", Some(Duration::from_secs(60))),
    ("5m", Some(Duration::from_secs(5 * 60))),
    ("Off", None),
];

pub struct RemoteHostsView<'a> {
    panel: &'a mut Panel,
}

impl<'a> RemoteHostsView<'a> {
    pub fn new(panel: &'a mut Panel) -> Self {
        Self { panel }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, is_focused: bool) -> bool {
        let clicked = ui.rect_contains_pointer(ui.max_rect());
        let panel_id = self.panel.id.0;
        let Some(remote_hosts) = self.panel.remote_hosts_mut() else {
            return clicked;
        };

        ui.ctx().request_repaint_after(next_repaint_delay(remote_hosts));

        let filtered = filtered_indices(remote_hosts);
        clamp_selected(remote_hosts, filtered.len());
        handle_keyboard(ui.ctx(), remote_hosts, is_focused, &filtered);

        render_header(ui, remote_hosts, panel_id, filtered.len());
        ui.add_space(10.0);
        render_search(ui, remote_hosts);
        ui.add_space(10.0);

        if let Some(error) = &remote_hosts.last_error {
            render_error_banner(ui, error);
            ui.add_space(10.0);
        }

        let selected_host = selected_host(remote_hosts, &filtered);
        let footer_height = if selected_host.is_some() {
            FOOTER_HEIGHT + 10.0
        } else {
            0.0
        };

        render_hosts_card(ui, remote_hosts, panel_id, &filtered, footer_height);

        if let Some(host) = selected_host {
            ui.add_space(10.0);
            render_selected_host_bar(ui, remote_hosts, &host);
        }

        clicked
    }
}

fn render_header(
    ui: &mut egui::Ui,
    remote_hosts: &mut horizon_core::RemoteHostsPanel,
    panel_id: u64,
    filtered_count: usize,
) {
    egui::Frame::new()
        .fill(theme::alpha(theme::BG_ELEVATED, 190))
        .corner_radius(CornerRadius::same(14))
        .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 210)))
        .show(ui, |ui| {
            ui.set_min_height(HEADER_HEIGHT);

            let total_count = remote_hosts.catalog.hosts.len();
            let online_count = remote_hosts
                .catalog
                .hosts
                .iter()
                .filter(|host| host.status == RemoteHostStatus::Online)
                .count();
            let offline_count = remote_hosts
                .catalog
                .hosts
                .iter()
                .filter(|host| host.status == RemoteHostStatus::Offline)
                .count();

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Remote Hosts").size(15.0).color(theme::FG).strong());
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new("SSH config and Tailscale discovery in one host browser")
                            .size(10.5)
                            .color(theme::FG_DIM),
                    );
                });

                ui.add_space(14.0);
                render_header_badge(
                    ui,
                    &format!("{filtered_count}/{total_count} visible"),
                    theme::alpha(theme::FG_DIM, 180),
                );
                render_header_badge(ui, &format!("{online_count} online"), theme::PALETTE_GREEN);
                if offline_count > 0 {
                    render_header_badge(ui, &format!("{offline_count} offline"), theme::PALETTE_RED);
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let refresh_button = Button::new(
                        RichText::new(if remote_hosts.refresh_in_flight {
                            "Refreshing…"
                        } else {
                            "Refresh"
                        })
                        .size(11.0),
                    )
                    .fill(theme::PANEL_BG_ALT)
                    .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 220)))
                    .corner_radius(10);
                    if ui
                        .add_enabled(!remote_hosts.refresh_in_flight, refresh_button)
                        .clicked()
                    {
                        remote_hosts.request_refresh();
                    }

                    ui.add_space(10.0);
                    ComboBox::from_id_salt(("remote_hosts_refresh_interval", panel_id))
                        .selected_text(refresh_interval_label(remote_hosts.auto_refresh_interval()))
                        .width(78.0)
                        .show_ui(ui, |ui| {
                            for (label, interval) in REFRESH_OPTIONS {
                                if ui
                                    .selectable_label(remote_hosts.auto_refresh_interval() == interval, label)
                                    .clicked()
                                {
                                    remote_hosts.set_auto_refresh_interval(interval);
                                }
                            }
                        });

                    ui.add_space(8.0);
                    ui.label(RichText::new("Auto").size(10.5).color(theme::FG_DIM).strong());
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new(refresh_status_label(remote_hosts))
                            .size(10.5)
                            .color(theme::FG_DIM),
                    );
                });
            });
        });
}

fn render_header_badge(ui: &mut egui::Ui, label: &str, color: Color32) {
    egui::Frame::new()
        .fill(theme::alpha(color, 18))
        .corner_radius(CornerRadius::same(BADGE_CORNER))
        .stroke(Stroke::new(1.0, theme::alpha(color, 70)))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(10.5).color(color).strong());
        });
}

fn render_search(ui: &mut egui::Ui, remote_hosts: &mut horizon_core::RemoteHostsPanel) {
    egui::Frame::new()
        .fill(theme::alpha(theme::PANEL_BG_ALT, 240))
        .corner_radius(CornerRadius::same(12))
        .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 200)))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_height(SEARCH_HEIGHT);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Search").size(10.5).color(theme::FG_DIM).strong());
                ui.add_space(10.0);
                let response = ui.add_sized(
                    [ui.available_width().max(180.0), 30.0],
                    egui::TextEdit::singleline(&mut remote_hosts.query)
                        .hint_text("alias, node, IP, OS, tag, or source")
                        .font(FontId::proportional(12.5)),
                );
                if response.changed() {
                    remote_hosts.selected = 0;
                }
            });
        });
}

fn render_error_banner(ui: &mut egui::Ui, error: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ERROR_HEIGHT), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(8), theme::alpha(theme::PALETTE_RED, 18));
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0, theme::alpha(theme::PALETTE_RED, 72)),
        egui::StrokeKind::Outside,
    );
    ui.painter().text(
        rect.left_center() + egui::vec2(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        error,
        FontId::proportional(11.0),
        theme::PALETTE_RED,
    );
}

fn render_hosts_card(
    ui: &mut egui::Ui,
    remote_hosts: &mut horizon_core::RemoteHostsPanel,
    panel_id: u64,
    filtered: &[usize],
    footer_height: f32,
) {
    egui::Frame::new()
        .fill(theme::alpha(theme::BG_ELEVATED, 178))
        .corner_radius(CornerRadius::same(16))
        .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 210)))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            let list_height = (ui.available_height() - footer_height).max(120.0);
            ScrollArea::vertical()
                .id_salt(("remote_hosts_rows", panel_id))
                .max_height(list_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if remote_hosts.catalog.hosts.is_empty() && remote_hosts.refresh_in_flight {
                        render_loading_state(ui);
                        return;
                    }

                    if filtered.is_empty() {
                        render_empty_state(ui, remote_hosts);
                        return;
                    }

                    for (filtered_index, host_index) in filtered.iter().copied().enumerate() {
                        let Some(host) = remote_hosts.catalog.hosts.get(host_index).cloned() else {
                            continue;
                        };
                        let selected = remote_hosts.selected == filtered_index;
                        render_host_row(ui, remote_hosts, filtered_index, &host, selected);
                        ui.add_space(6.0);
                    }
                });
        });
}

fn render_loading_state(ui: &mut egui::Ui) {
    ui.add_space(26.0);
    ui.vertical_centered(|ui| {
        ui.spinner();
        ui.add_space(8.0);
        ui.label(
            RichText::new("Refreshing remote hosts…")
                .size(12.0)
                .color(theme::FG_DIM),
        );
    });
}

fn render_empty_state(ui: &mut egui::Ui, remote_hosts: &horizon_core::RemoteHostsPanel) {
    ui.add_space(26.0);
    ui.vertical_centered(|ui| {
        let message = if remote_hosts.catalog.hosts.is_empty() {
            "No remote hosts discovered"
        } else {
            "No hosts match the current search"
        };
        ui.label(RichText::new(message).size(13.0).color(theme::FG_SOFT));
        ui.add_space(6.0);
        ui.label(
            RichText::new("Try a different search or refresh the discovery sources.")
                .size(10.5)
                .color(theme::FG_DIM),
        );
    });
}

fn render_host_row(
    ui: &mut egui::Ui,
    remote_hosts: &mut horizon_core::RemoteHostsPanel,
    filtered_index: usize,
    host: &RemoteHost,
    selected: bool,
) {
    let (row_rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
    if response.clicked() {
        remote_hosts.selected = filtered_index;
    }
    if response.double_clicked() {
        remote_hosts.queue_open_ssh(host);
    }

    let fill = if selected {
        theme::alpha(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.18), 255)
    } else if response.hovered() {
        theme::alpha(theme::PANEL_BG_ALT, 255)
    } else {
        theme::alpha(theme::PANEL_BG, 235)
    };
    let stroke = if selected {
        Stroke::new(1.0, theme::alpha(theme::ACCENT, 180))
    } else {
        Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 160))
    };

    ui.painter().rect_filled(row_rect, CornerRadius::same(12), fill);
    ui.painter()
        .rect_stroke(row_rect, CornerRadius::same(12), stroke, egui::StrokeKind::Outside);

    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(row_rect.shrink2(Vec2::new(12.0, 10.0)))
            .layout(Layout::left_to_right(Align::TOP)),
        |ui| {
            let side_width = 230.0_f32.min(ui.available_width() * 0.42);
            let info_width = (ui.available_width() - side_width).max(180.0);

            ui.allocate_ui_with_layout(
                Vec2::new(info_width, ROW_HEIGHT - 20.0),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.label(RichText::new(&host.label).size(13.0).color(theme::FG).strong());
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(host.target())
                            .font(FontId::monospace(10.5))
                            .color(theme::FG_SOFT),
                    );
                    ui.add_space(4.0);
                    ui.add_sized(
                        [ui.available_width(), 14.0],
                        egui::Label::new(
                            RichText::new(host_detail(host))
                                .font(FontId::monospace(10.0))
                                .color(theme::FG_DIM),
                        )
                        .truncate(),
                    );
                },
            );

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(os) = host.os.as_deref() {
                    ui.label(RichText::new(os).size(10.5).color(theme::FG_DIM).monospace());
                    ui.add_space(10.0);
                }
                render_badge(ui, host.status.label(), status_badge_color(host.status));
                ui.add_space(6.0);
                render_badge(ui, host.sources.label(), source_badge_color(host));
            });
        },
    );
}

fn render_selected_host_bar(ui: &mut egui::Ui, remote_hosts: &mut horizon_core::RemoteHostsPanel, host: &RemoteHost) {
    egui::Frame::new()
        .fill(theme::alpha(
            theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.14),
            255,
        ))
        .corner_radius(CornerRadius::same(16))
        .stroke(Stroke::new(1.0, theme::alpha(theme::ACCENT, 170)))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_height(FOOTER_HEIGHT);

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Selected host").size(10.5).color(theme::FG_DIM).strong());
                    ui.add_space(2.0);
                    ui.label(RichText::new(&host.label).size(14.0).color(theme::FG).strong());
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(host.target())
                            .font(FontId::monospace(10.5))
                            .color(theme::FG_SOFT),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        render_badge(ui, host.sources.label(), source_badge_color(host));
                        ui.add_space(6.0);
                        render_badge(ui, host.status.label(), status_badge_color(host.status));
                    });
                });

                ui.add_space(18.0);
                ui.separator();
                ui.add_space(18.0);

                ui.vertical(|ui| {
                    ui.label(RichText::new("SSH user").size(10.5).color(theme::FG_DIM).strong());
                    ui.add_space(4.0);
                    {
                        let user_draft = remote_hosts.user_draft_for_host_mut(host);
                        ui.add_sized(
                            [150.0, 30.0],
                            egui::TextEdit::singleline(user_draft)
                                .hint_text("optional")
                                .font(FontId::proportional(12.0)),
                        );
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!(
                            "Connects to {}",
                            remote_hosts.effective_ssh_connection(host).display_label()
                        ))
                        .size(10.5)
                        .color(theme::FG_SOFT),
                    );
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let connect_button = Button::new(RichText::new("Connect").size(11.5).color(theme::FG).strong())
                        .fill(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.32))
                        .stroke(Stroke::new(
                            1.0,
                            theme::blend(theme::BORDER_STRONG, theme::ACCENT, 0.72),
                        ))
                        .corner_radius(12);
                    if ui.add_sized([92.0, 34.0], connect_button).clicked() {
                        remote_hosts.queue_open_ssh(host);
                    }
                    ui.add_space(12.0);
                    ui.label(RichText::new("Enter connects").size(10.5).color(theme::FG_DIM));
                });
            });
        });
}

fn render_badge(ui: &mut egui::Ui, label: &str, color: Color32) {
    egui::Frame::new()
        .fill(theme::alpha(color, 16))
        .corner_radius(CornerRadius::same(BADGE_CORNER))
        .stroke(Stroke::new(1.0, theme::alpha(color, 60)))
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(10.0).color(color).strong());
        });
}

fn selected_host(remote_hosts: &horizon_core::RemoteHostsPanel, filtered: &[usize]) -> Option<RemoteHost> {
    filtered
        .get(remote_hosts.selected)
        .and_then(|host_index| remote_hosts.catalog.hosts.get(*host_index))
        .cloned()
}

fn host_detail(host: &RemoteHost) -> String {
    let mut parts = Vec::new();
    if !host.tags.is_empty() {
        parts.push(host.tags.join(" · "));
    }
    if !host.ips.is_empty() {
        parts.push(host.ips.join(", "));
    }
    if let Some(last_seen) = &host.last_seen {
        parts.push(format!("last seen {last_seen}"));
    }

    if parts.is_empty() {
        "No extra metadata".to_string()
    } else {
        parts.join("  •  ")
    }
}

fn filtered_indices(remote_hosts: &horizon_core::RemoteHostsPanel) -> Vec<usize> {
    let query = remote_hosts.query.trim().to_ascii_lowercase();
    remote_hosts
        .catalog
        .hosts
        .iter()
        .enumerate()
        .filter(|(_, host)| query.is_empty() || remote_host_matches_query(host, &query))
        .map(|(index, _)| index)
        .collect()
}

fn remote_host_matches_query(host: &RemoteHost, query: &str) -> bool {
    if host.label.to_ascii_lowercase().contains(query) {
        return true;
    }
    if host.target().to_ascii_lowercase().contains(query) {
        return true;
    }
    if host.sources.label().to_ascii_lowercase().contains(query) {
        return true;
    }
    if host.status.label().to_ascii_lowercase().contains(query) {
        return true;
    }
    if host
        .os
        .as_deref()
        .is_some_and(|os| os.to_ascii_lowercase().contains(query))
    {
        return true;
    }
    host.tags
        .iter()
        .chain(host.ips.iter())
        .any(|value| value.to_ascii_lowercase().contains(query))
}

fn clamp_selected(remote_hosts: &mut horizon_core::RemoteHostsPanel, count: usize) {
    if count == 0 {
        remote_hosts.selected = 0;
    } else if remote_hosts.selected >= count {
        remote_hosts.selected = count - 1;
    }
}

fn handle_keyboard(
    ctx: &egui::Context,
    remote_hosts: &mut horizon_core::RemoteHostsPanel,
    is_focused: bool,
    filtered: &[usize],
) {
    if !is_focused || ctx.wants_keyboard_input() || filtered.is_empty() {
        return;
    }

    if ctx.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
        remote_hosts.selected = (remote_hosts.selected + 1).min(filtered.len() - 1);
    }
    if ctx.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
        remote_hosts.selected = remote_hosts.selected.saturating_sub(1);
    }
    if ctx.input(|input| input.key_pressed(egui::Key::Enter))
        && let Some(host_index) = filtered.get(remote_hosts.selected)
        && let Some(host) = remote_hosts.catalog.hosts.get(*host_index).cloned()
    {
        remote_hosts.queue_open_ssh(&host);
    }
}

fn refresh_status_label(remote_hosts: &horizon_core::RemoteHostsPanel) -> String {
    if remote_hosts.refresh_in_flight {
        return "updating now".to_string();
    }

    remote_hosts
        .last_refresh_completed_at()
        .map_or_else(|| "waiting for first refresh".to_string(), age_label)
}

fn age_label(last_refresh: Instant) -> String {
    let age = last_refresh.elapsed();
    if age < Duration::from_secs(2) {
        "updated just now".to_string()
    } else if age < Duration::from_secs(60) {
        format!("updated {}s ago", age.as_secs())
    } else {
        format!("updated {}m ago", age.as_secs() / 60)
    }
}

fn next_repaint_delay(remote_hosts: &horizon_core::RemoteHostsPanel) -> Duration {
    if remote_hosts.refresh_in_flight {
        return Duration::from_millis(250);
    }

    if let Some(last_refresh) = remote_hosts.last_refresh_completed_at()
        && let Some(interval) = remote_hosts.auto_refresh_interval()
    {
        return interval
            .saturating_sub(last_refresh.elapsed())
            .min(Duration::from_secs(1));
    }

    Duration::from_secs(1)
}

fn refresh_interval_label(interval: Option<Duration>) -> &'static str {
    REFRESH_OPTIONS
        .iter()
        .find(|(_, candidate)| *candidate == interval)
        .map_or("Custom", |(label, _)| *label)
}

fn source_badge_color(host: &RemoteHost) -> Color32 {
    match (host.sources.ssh_config, host.sources.tailscale) {
        (true, true) => Color32::from_rgb(166, 227, 161),
        (true, false) => Color32::from_rgb(137, 180, 250),
        (false, true) => Color32::from_rgb(249, 226, 175),
        (false, false) => theme::FG_DIM,
    }
}

fn status_badge_color(status: RemoteHostStatus) -> Color32 {
    match status {
        RemoteHostStatus::Online => theme::PALETTE_GREEN,
        RemoteHostStatus::Offline => theme::PALETTE_RED,
        RemoteHostStatus::Unknown => theme::FG_DIM,
    }
}
