//! The content of the hub pages. Quick nav, hosts and sessions show what Horizon really
//! has; cloud shows sample environments, because real ones need an account.

use egui::{
    Align, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextEdit, Ui, vec2,
};
use horizon_core::RemoteHostStatus;

use super::super::super::{blocks, command_bar};
use super::super::HorizonApp;
use super::super::desk_bar::paint::{elide, section_label};
use super::status_color;
use crate::theme;

/// Environments shown on the cloud page. Sample data: real ones need a provider account.
const CLOUD: [(&str, &str, &str, &str); 4] = [
    ("hetzner-cx42", "Hetzner, fsn1", "Running", "0.03 EUR/h"),
    ("build-farm", "Hetzner, nbg1", "Running", "0.12 EUR/h"),
    ("gpu-sandbox", "Scaleway, par1", "Stopped", "-"),
    ("preview-eu", "Hetzner, hel1", "Idle", "0.01 EUR/h"),
];

impl HorizonApp {
    // ---- quick nav -----------------------------------------------------------

    pub(super) fn page_nav(&mut self, ui: &mut Ui) {
        let mut query = std::mem::take(&mut self.assistant.summon.hub.query);
        Frame::new()
            .fill(theme::BG())
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(CornerRadius::same(11))
            .inner_margin(Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(
                    TextEdit::singleline(&mut query)
                        .frame(Frame::NONE)
                        .margin(Margin::ZERO)
                        .desired_width(f32::INFINITY)
                        .hint_text(RichText::new("Jump to a workspace or a panel").color(theme::FG_DIM()))
                        .text_color(theme::FG()),
                );
            });
        ui.add_space(10.0);
        let needle = query.to_lowercase();
        self.assistant.summon.hub.query = query;

        let desk = self
            .assistant
            .desk
            .as_ref()
            .map(crate::app::desk::Desk::snapshot)
            .unwrap_or_default();
        let mut go: Option<(usize, Option<horizon_core::PanelId>)> = None;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section_label(ui, "Workspaces");
            ui.add_space(6.0);
            for (index, workspace) in self.board.workspaces.iter().enumerate() {
                if !needle.is_empty() && !workspace.name.to_lowercase().contains(&needle) {
                    continue;
                }
                if workspace.panels.is_empty() && needle.is_empty() && index > 5 {
                    continue;
                }
                let live = desk.active == index;
                let summary = match workspace.panels.len() {
                    1 => "1 panel".to_string(),
                    count => format!("{count} panels"),
                };
                if list_row(
                    ui,
                    &format!("{}", index + 1),
                    &workspace.name,
                    &summary,
                    live.then_some("Here"),
                )
                .clicked()
                {
                    go = Some((index, None));
                }
            }
            ui.add_space(10.0);
            section_label(ui, "Panels");
            ui.add_space(6.0);
            let mut shown = 0;
            for panel in &self.board.panels {
                if panel.is_assistant() || shown >= 8 {
                    continue;
                }
                let title = panel.display_title().into_owned();
                if !needle.is_empty() && !title.to_lowercase().contains(&needle) {
                    continue;
                }
                let Some(index) = self
                    .board
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.id == panel.workspace_id)
                else {
                    continue;
                };
                shown += 1;
                let kind = format!("{:?}", panel.kind).to_lowercase();
                if list_row(
                    ui,
                    &format!("{}", index + 1),
                    &title,
                    &format!("{kind} in {}", self.board.workspaces[index].name),
                    None,
                )
                .clicked()
                {
                    go = Some((index, Some(panel.id)));
                }
            }
        });
        if let Some((index, panel)) = go {
            if let Some(desk) = self.assistant.desk.as_ref() {
                desk.switch(index);
            }
            if let Some(id) = panel {
                self.board.focus(id);
            }
        }
    }

    // ---- remote hosts --------------------------------------------------------

    pub(super) fn page_hosts(&mut self, ui: &mut Ui) {
        // A scripted demo shows only the SSH config's hosts: the tailnet's belong to whoever runs it.
        let scripted = self.assistant.demo.is_some();
        let hosts: Vec<_> = self
            .remote_hosts_catalog
            .hosts
            .iter()
            .filter(|host| !scripted || host.sources.ssh_config)
            .map(|host| {
                (
                    host.label.clone(),
                    host.display_target(),
                    host.os.clone().unwrap_or_default(),
                    host.status,
                    host.tags.clone(),
                )
            })
            .collect();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} hosts", hosts.len()))
                    .size(12.5)
                    .color(theme::FG_DIM()),
            );
        });
        ui.add_space(8.0);
        let mut open: Option<(String, bool)> = None;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if hosts.is_empty() {
                ui.label(
                    RichText::new("No hosts found yet. Add some to ~/.ssh/config.")
                        .size(13.0)
                        .color(theme::FG_DIM()),
                );
            }
            for (label, target, os, status, tags) in &hosts {
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let word = match status {
                                RemoteHostStatus::Online => "Online",
                                RemoteHostStatus::Offline => "Offline",
                                RemoteHostStatus::Unknown => "Unknown",
                            };
                            let (dot, _) = ui.allocate_exact_size(vec2(14.0, 20.0), Sense::hover());
                            ui.painter().circle_filled(dot.center(), 4.5, status_color(word));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(label).size(14.0).strong().color(theme::FG()));
                                let detail = if os.is_empty() {
                                    target.clone()
                                } else {
                                    format!("{target}  -  {os}")
                                };
                                ui.label(RichText::new(elide(&detail, 60)).size(11.5).color(theme::FG_DIM()));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if blocks::ghost(ui, "VNC").clicked() {
                                    open = Some((label.clone(), true));
                                }
                                if blocks::primary(ui, "SSH").clicked() {
                                    open = Some((label.clone(), false));
                                }
                                for tag in tags.iter().take(2) {
                                    chip(ui, tag);
                                }
                            });
                        });
                    });
                ui.add_space(8.0);
            }
        });
        if let Some((label, vnc)) = open {
            let ctx = ui.ctx().clone();
            self.open_remote_host_by_label(&ctx, &label, vnc);
            self.assistant.summon.hub.close();
        }
    }

    // ---- cloud ----------------------------------------------------------------

    pub(super) fn page_cloud(&mut self, ui: &mut Ui) {
        let stopped = self.assistant.summon.hub.stopped;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Sample environments. Real ones need a provider account.")
                    .size(12.5)
                    .color(theme::FG_DIM()),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let _ = blocks::primary(ui, "New environment");
            });
        });
        ui.add_space(8.0);
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (name, place, state, cost) in CLOUD {
                let state = if stopped && name == "hetzner-cx42" {
                    "Stopped"
                } else {
                    state
                };
                let cost = if stopped && name == "hetzner-cx42" { "-" } else { cost };
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (dot, _) = ui.allocate_exact_size(vec2(14.0, 20.0), Sense::hover());
                            ui.painter().circle_filled(dot.center(), 4.5, status_color(state));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(name).size(14.0).strong().color(theme::FG()));
                                ui.label(
                                    RichText::new(format!("{place}  -  {cost}"))
                                        .size(11.5)
                                        .color(theme::FG_DIM()),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let _ = blocks::ghost(ui, if state == "Stopped" { "Start" } else { "Stop" });
                                let _ = blocks::ghost(ui, "Open");
                                chip(ui, state);
                            });
                        });
                    });
                ui.add_space(8.0);
            }
        });
    }

    // ---- sessions -------------------------------------------------------------

    pub(super) fn page_sessions(&mut self, ui: &mut Ui) {
        let sessions = self.session_store.list_profile_sessions().unwrap_or_default();
        let this_panels = self.board.panels.iter().filter(|panel| !panel.is_assistant()).count();
        let this_workspaces = self.board.workspaces.len();
        ui.label(
            RichText::new(format!(
                "{} saved, this one is running",
                sessions.iter().filter(|s| !s.is_live).count()
            ))
            .size(12.5)
            .color(theme::FG_DIM()),
        );
        ui.add_space(8.0);
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // This session first, whether or not it is on disk (an ephemeral one is not).
            let _ = list_row(
                ui,
                "*",
                "This session",
                &format!("{this_workspaces} workspaces, {this_panels} panels"),
                Some("Current"),
            );
            for session in sessions.iter().filter(|session| !session.is_live) {
                let detail = format!("{} workspaces, {} panels", session.workspace_count, session.panel_count);
                let _ = list_row(ui, "-", &session.label, &detail, None);
            }
        });
    }

    // ---- settings -------------------------------------------------------------

    pub(super) fn page_settings(&mut self, ui: &mut Ui) {
        let ask = self.assistant.settings.ask_before_send;
        let mut toggle = false;
        let rows: [(&str, &str, bool, bool); 4] = [
            (
                "Ask before the assistant types",
                "Show a card for each message first",
                ask,
                true,
            ),
            (
                "Mark Horizon windows",
                "A tag and an outline on every panel window",
                true,
                false,
            ),
            ("Speak replies", "The assistant answers aloud", true, false),
            (
                "Keep the command bar on every desktop",
                "Always on top, above the dock",
                true,
                false,
            ),
        ];
        for (title, detail, on, wired) in rows {
            Frame::new()
                .fill(theme::BG_ELEVATED())
                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(title).size(13.5).color(theme::FG()));
                            ui.label(RichText::new(detail).size(11.5).color(theme::FG_DIM()));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if switch(ui, on).clicked() && wired {
                                toggle = true;
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }
        if toggle {
            self.run_local_command(command_bar::LocalCommand::ToggleAsk);
        }
    }
}

/// One row of a list: a number or mark, a title, a detail line, and an optional pill.
fn list_row(ui: &mut Ui, mark: &str, title: &str, detail: &str, pill: Option<&str>) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(10), theme::ACCENT().gamma_multiply(0.1));
    }
    painter.text(
        rect.left_center() + vec2(18.0, 0.0),
        egui::Align2::CENTER_CENTER,
        mark,
        egui::FontId::monospace(12.5),
        theme::ACCENT(),
    );
    painter.text(
        rect.left_center() + vec2(42.0, -8.0),
        egui::Align2::LEFT_CENTER,
        elide(title, 46),
        egui::FontId::proportional(13.5),
        theme::FG(),
    );
    painter.text(
        rect.left_center() + vec2(42.0, 9.0),
        egui::Align2::LEFT_CENTER,
        elide(detail, 70),
        egui::FontId::proportional(11.5),
        theme::FG_DIM(),
    );
    if let Some(word) = pill {
        let color = status_color(word);
        let width = painter
            .layout_no_wrap(word.to_string(), egui::FontId::proportional(11.0), color)
            .size()
            .x
            + 20.0;
        let chip = egui::Rect::from_center_size(rect.right_center() - vec2(width / 2.0 + 10.0, 0.0), vec2(width, 22.0));
        painter.rect(
            chip,
            CornerRadius::same(99),
            color.gamma_multiply(0.15),
            Stroke::new(1.0, color.gamma_multiply(0.45)),
            StrokeKind::Inside,
        );
        painter.text(
            chip.center(),
            egui::Align2::CENTER_CENTER,
            word,
            egui::FontId::proportional(11.0),
            color,
        );
    }
    response
}

fn chip(ui: &mut Ui, text: &str) {
    let color = status_color(text);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), egui::FontId::proportional(11.0), color);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(18.0, 8.0), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(99),
        color.gamma_multiply(0.13),
        Stroke::new(1.0, color.gamma_multiply(0.4)),
        StrokeKind::Inside,
    );
    ui.painter().galley(rect.left_top() + vec2(9.0, 4.0), galley, color);
}

/// A small on/off switch.
fn switch(ui: &mut Ui, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(40.0, 22.0), Sense::click());
    let track = if on { theme::ACCENT() } else { theme::BORDER_STRONG() };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(99), track.gamma_multiply(0.55));
    let knob = rect.left_center() + vec2(if on { 29.0 } else { 11.0 }, 0.0);
    ui.painter().circle_filled(knob, 8.0, theme::FG());
    response
}
