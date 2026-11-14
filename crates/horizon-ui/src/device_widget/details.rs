//! Machine identity and selectable connection facts, separate from image controls.
use egui::{Align, Label, Layout, Stroke, Ui, vec2};
use horizon_core::{
    DevicePanelState,
    browser::manifest::device::{DeviceServerDetails, NativeSessionMetadata},
};

use super::panel_text;
use super::session::Status;
use crate::theme;

/// Rebuild caption strings only when metadata changes, not on pointer frames.
#[derive(Default)]
pub(super) struct NativeLabels {
    device: String,
    lane: String,
    app: String,
    progress: String,
}
impl NativeLabels {
    pub(super) fn new(native: &NativeSessionMetadata) -> Self {
        let result = native
            .recipes
            .iter()
            .rev()
            .find(|result| Some(&result.recipe) == native.recipe.as_ref());
        Self {
            device: format!("{} · {} · {}", native.model, native.os, native.form),
            lane: format!("{} · lane {} of {}", native.provider, native.lane, native.lanes),
            app: format!(
                "{} · {}",
                native.app,
                native.build_sha256.get(..12).unwrap_or(&native.build_sha256)
            ),
            progress: match (&native.recipe, &native.step, result) {
                (Some(recipe), Some(step), _) => format!("{recipe} · {step}"),
                (Some(recipe), None, Some(result)) => {
                    format!("{recipe}: {}", result.status_label())
                }
                (Some(recipe), None, _) => recipe.clone(),
                _ => String::new(),
            },
        }
    }
}

pub(super) fn show(ui: &mut Ui, device: &DevicePanelState, server: &DeviceServerDetails, connected: bool) {
    let heading = panel_text::heading(ui, "Connection details");
    ui.collapsing(heading, |ui| {
        let height = (ui.available_height() * 0.6).clamp(96.0, 300.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .max_height(height)
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL_BG_ALT())
                    .corner_radius(8)
                    .inner_margin(12)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 4.0;
                        if device.identity.is_some() && ui.available_width() >= 620.0 {
                            ui.columns(2, |columns| {
                                machine(&mut columns[0], device);
                                connection(&mut columns[1], device, server, connected);
                            });
                        } else {
                            if device.identity.is_some() {
                                machine(ui, device);
                                ui.add_space(10.0);
                                ui.separator();
                                ui.add_space(8.0);
                            }
                            connection(ui, device, server, connected);
                        }
                    });
            });
        ui.add_space(4.0);
    });
}

fn section(ui: &mut Ui, title: &str) {
    let title = panel_text::word(ui, title, panel_text::VALUE, theme::FG());
    ui.add(Label::new(title));
    ui.add_space(5.0);
}

pub(super) fn caption(ui: &mut Ui, text: &str) {
    let text = panel_text::wrapped(ui, text, panel_text::SMALL, theme::FG_SOFT(), ui.available_width());
    ui.add(Label::new(text));
    ui.add_space(6.0);
}

fn machine(ui: &mut Ui, device: &DevicePanelState) {
    let Some(identity) = &device.identity else {
        return;
    };
    section(ui, "Machine");
    caption(ui, "Supplied by session creator");
    for (label, value) in [
        ("Name", identity.machine_name.as_deref()),
        ("Hostname", identity.hostname.as_deref()),
        ("Tailscale", identity.tailscale_name.as_deref()),
    ] {
        if let Some(value) = value {
            row(ui, label, value);
        }
    }
    for address in &identity.ip_addresses {
        row(ui, "IP address", &address.to_string());
    }
}

fn connection(ui: &mut Ui, device: &DevicePanelState, server: &DeviceServerDetails, connected: bool) {
    section(ui, "Connection");
    match &device.ssh_tunnel {
        Some(tunnel) => {
            row(ui, "SSH host", &tunnel.display_label());
            if let Some(port) = tunnel.port {
                row(ui, "SSH port", &port.to_string());
            }
            row(ui, "Remote endpoint", &device.target.address().to_string());
        }
        None => row(ui, "Local endpoint", &device.target.address().to_string()),
    }
    if server.name.is_some() || server.desktop_size.is_some() {
        ui.add_space(6.0);
        caption(
            ui,
            if connected {
                "Server-reported"
            } else {
                "Server-reported (disconnected)"
            },
        );
        if let Some(name) = &server.name {
            row(ui, "VNC name", name);
        }
        if let Some([width, height]) = server.desktop_size {
            row(ui, "Resolution", &format!("{width} × {height}"));
        }
    }
    if let Some(native) = &server.native_session {
        ui.add_space(6.0);
        section(ui, "Native test (host-reported)");
        row(ui, "Session", &native.session_id);
        if let Some(run) = &native.run_id {
            row(ui, "Run", run);
        }
        row(ui, "Build SHA256", &native.build_sha256);
        for result in &native.recipes {
            row(ui, &result.recipe, result.status_label());
        }
    }
    if device.display_name(server.name.as_deref()).is_none() {
        ui.add_space(6.0);
        caption(ui, "Machine name unavailable");
    }
}

fn row(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.allocate_ui_with_layout(vec2(86.0, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_min_width(86.0);
            let label = panel_text::word(ui, label, panel_text::SMALL, theme::FG_SOFT());
            ui.add(Label::new(label));
        });
        let value = panel_text::mono_wrapped(ui, value, panel_text::VALUE, panel_text::fg(), ui.available_width());
        ui.add(Label::new(value).selectable(true));
    });
    ui.add_space(3.0);
}

/// What the person asked of the connection while the header was drawn. The
/// action runs after the frame, so no rendering step changes the connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum HeaderAction {
    #[default]
    None,
    /// Connect again now, and forget the last image.
    Reconnect,
    /// End the attempts and stay stopped until a person connects again.
    Stop,
}

/// Collect the reconnect and stop actions without mutating the connection
/// during rendering.
pub(super) fn header(
    ui: &mut Ui,
    device: &DevicePanelState,
    server: &DeviceServerDetails,
    native_labels: &NativeLabels,
    status: &super::session::Status,
    interactive: bool,
    interact: bool,
) -> HeaderAction {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.add_space(8.0);
        let name = if server.native_session.is_some() {
            Some(native_labels.device.as_str())
        } else {
            device.display_name(server.name.as_deref())
        };
        // Stopping only means something while something runs or waits, and a
        // stopped viewer keeps the two words that bring it back.
        let can_stop = !matches!(status, Status::Stopped);
        let requested = header_row(ui, device, server, native_labels, name, can_stop, interactive);
        native_captions(ui, server, native_labels);
        ui.add_space(6.0);
        header_facts(ui, device, server, name, status, interact);
        status_message(ui, status);
        ui.add_space(8.0);
        requested
    })
    .inner
}

/// The icon, the name of the desktop and its two buttons.
fn header_row(
    ui: &mut Ui,
    device: &DevicePanelState,
    server: &DeviceServerDetails,
    native_labels: &NativeLabels,
    name: Option<&str>,
    can_stop: bool,
    interactive: bool,
) -> HeaderAction {
    let endpoint = device.endpoint_label();
    let transport = if device.ssh_tunnel.is_some() {
        "VNC desktop over SSH"
    } else {
        "VNC desktop"
    };
    let buttons = if can_stop { 176.0 } else { 88.0 };
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        if ui.available_width() >= 300.0 {
            desktop_icon(ui);
        }
        let width = (ui.available_width() - buttons - 12.0).max(24.0);
        ui.allocate_ui_with_layout(vec2(width, 40.0), Layout::top_down(Align::Min), |ui| {
            ui.set_min_width(width);
            let title = panel_text::line(ui, name.unwrap_or(&endpoint), panel_text::NAME, panel_text::fg(), width);
            ui.add(Label::new(title).truncate())
                .on_hover_text(name.unwrap_or(&endpoint));
            let subtitle = if server.native_session.is_some() {
                &native_labels.lane
            } else if name.is_some() {
                &endpoint
            } else {
                transport
            };
            let subtitle = panel_text::line(ui, subtitle, panel_text::SMALL, theme::FG_SOFT(), width);
            ui.add(Label::new(subtitle).truncate()).on_hover_text(&endpoint);
        });
        header_actions(ui, buttons, can_stop, interactive)
    })
    .inner
}

/// The two words that change the connection, at the right of the header row.
fn header_actions(ui: &mut Ui, buttons: f32, can_stop: bool, interactive: bool) -> HeaderAction {
    ui.allocate_ui_with_layout(vec2(buttons, 40.0), Layout::right_to_left(Align::Center), |ui| {
        let mut action = HeaderAction::None;
        let stop = panel_text::word(ui, "Stop", panel_text::SMALL, theme::FG());
        if can_stop
            && ui
                .add_enabled(interactive, egui::Button::new(stop).min_size(vec2(72.0, 28.0)))
                .on_hover_text("End the connection and its automatic attempts")
                .clicked()
        {
            action = HeaderAction::Stop;
        }
        let reconnect = panel_text::word(ui, "Reconnect", panel_text::SMALL, theme::FG());
        if ui
            .add_enabled(interactive, egui::Button::new(reconnect).min_size(vec2(84.0, 28.0)))
            .clicked()
        {
            action = HeaderAction::Reconnect;
        }
        action
    })
    .inner
}

/// The two lines that a native host reports, under the header row.
fn native_captions(ui: &mut Ui, server: &DeviceServerDetails, native_labels: &NativeLabels) {
    let Some(native) = &server.native_session else {
        return;
    };
    let width = ui.available_width();
    let font = panel_text::body_font(ui);
    let app =
        crate::zoom_text::wrapped_widget_text(ui.painter(), &native_labels.app, font.clone(), panel_text::fg(), width);
    ui.add(Label::new(app)).on_hover_text(&native.build_sha256);
    if !native_labels.progress.is_empty() {
        let progress =
            crate::zoom_text::wrapped_widget_text(ui.painter(), &native_labels.progress, font, panel_text::fg(), width);
        ui.add(Label::new(progress));
    }
}

/// The state of the connection, the input mode and the source of the name.
fn header_facts(
    ui: &mut Ui,
    device: &DevicePanelState,
    server: &DeviceServerDetails,
    name: Option<&str>,
    status: &super::session::Status,
    interact: bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        connection_status(ui, status);
        let mode = if interact {
            "Interactive: your mouse and keyboard go to the desktop"
        } else {
            "Read-only"
        };
        let mode = panel_text::word(ui, mode, panel_text::SMALL, theme::FG_SOFT());
        ui.add(Label::new(mode));
        if name.is_some() {
            let source = if server.native_session.is_some() {
                "Native host"
            } else if device.display_name(None).is_some() {
                "Supplied name"
            } else {
                "VNC name"
            };
            let source = panel_text::word(ui, source, panel_text::SMALL, theme::FG_SOFT());
            ui.add(Label::new(source));
        }
    });
}

/// What the state of the connection adds below its badge.
fn status_message(ui: &mut Ui, status: &super::session::Status) {
    match status {
        Status::Disconnected { message, .. } => {
            ui.add_space(3.0);
            let message = panel_text::wrapped(
                ui,
                message,
                panel_text::SMALL,
                theme::PALETTE_RED(),
                ui.available_width(),
            );
            ui.add(Label::new(message));
        }
        Status::Reconnecting { message, .. } => {
            ui.add_space(3.0);
            caption(ui, "The link to the desktop is lost. Horizon tries again by itself.");
            let message = panel_text::wrapped(ui, message, panel_text::SMALL, theme::FG_SOFT(), ui.available_width());
            ui.add(Label::new(message));
        }
        Status::Stopped => {
            ui.add_space(3.0);
            caption(ui, "Select Reconnect to view this desktop.");
        }
        Status::Connecting | Status::Connected => {}
    }
}

fn connection_status(ui: &mut Ui, status: &super::session::Status) {
    let (label, color) = match status {
        Status::Stopped => ("Stopped".to_string(), theme::FG_SOFT()),
        Status::Connecting => ("Connecting…".to_string(), theme::PALETTE_YELLOW()),
        Status::Connected => ("Connected".to_string(), theme::PALETTE_GREEN()),
        Status::Disconnected { .. } => ("Disconnected".to_string(), theme::PALETTE_RED()),
        Status::Reconnecting { retry_at, .. } => (
            match whole_seconds_until(*retry_at) {
                Some(seconds) => format!("Reconnecting · next attempt in {seconds} s"),
                // The attempt starts within this frame, so no wait is left to
                // count.
                None => "Reconnecting…".to_string(),
            },
            theme::PALETTE_YELLOW(),
        ),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(vec2(8.0, 12.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.0, color);
        let label = panel_text::word(ui, &label, panel_text::SMALL, color);
        ui.add(Label::new(label));
    });
}

/// The whole seconds that a person waits for the next attempt, rounded up so
/// that a countdown never shows `0 s` while it still waits. `None` when no whole
/// second is left.
pub(super) fn whole_seconds_until(at: std::time::Instant) -> Option<u64> {
    let remaining = at.saturating_duration_since(std::time::Instant::now());
    if remaining.is_zero() {
        return None;
    }
    let round_up = u64::from(remaining.subsec_nanos() > 0);
    Some(remaining.as_secs() + round_up)
}

fn desktop_icon(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
    let painter = ui.painter();
    let color = theme::ACCENT();
    painter.rect_filled(rect, 10, theme::alpha(color, 22));
    let screen = egui::Rect::from_min_size(rect.min + vec2(9.0, 10.0), vec2(22.0, 15.0));
    painter.rect_stroke(screen, 2, Stroke::new(1.5, color), egui::StrokeKind::Inside);
    painter.line_segment(
        [rect.min + vec2(20.0, 25.0), rect.min + vec2(20.0, 30.0)],
        Stroke::new(1.5, color),
    );
    painter.line_segment(
        [rect.min + vec2(14.0, 30.0), rect.min + vec2(26.0, 30.0)],
        Stroke::new(1.5, color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_recipe_ids_show_active_step_then_latest_result() {
        let mut native = super::super::tests::native_metadata();
        native.recipe = Some("login".into());
        native.recipes = vec![horizon_core::browser::manifest::device::NativeRecipeResult {
            recipe: "login".into(),
            passed: true,
            blocked: false,
        }];
        native.step = Some("retry".into());
        assert_eq!(NativeLabels::new(&native).progress, "login · retry");
        native.step = None;
        native
            .recipes
            .push(horizon_core::browser::manifest::device::NativeRecipeResult {
                recipe: "login".into(),
                passed: false,
                blocked: false,
            });
        assert_eq!(NativeLabels::new(&native).progress, "login: FAIL");
        native.recipes.last_mut().unwrap().blocked = true;
        assert_eq!(NativeLabels::new(&native).progress, "login: BLOCKED");
    }
}
