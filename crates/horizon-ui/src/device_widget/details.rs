//! Machine identity and selectable connection facts, separate from image controls.
use egui::{Align, Label, Layout, RichText, Stroke, Ui, vec2};
use horizon_core::{
    DevicePanelState,
    browser::manifest::device::{DeviceServerDetails, NativeSessionMetadata},
};

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

pub(super) fn show(
    ui: &mut Ui,
    device: &DevicePanelState,
    server: &DeviceServerDetails,
    connected: bool,
    stream: Option<horizon_frame_source::Stats>,
    input_error: Option<&str>,
) {
    ui.collapsing("Connection details", |ui| {
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
                                connection(&mut columns[1], device, server, connected, stream, input_error);
                            });
                        } else {
                            if device.identity.is_some() {
                                machine(ui, device);
                                ui.add_space(10.0);
                                ui.separator();
                                ui.add_space(8.0);
                            }
                            connection(ui, device, server, connected, stream, input_error);
                        }
                    });
            });
        ui.add_space(4.0);
    });
}

fn section(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).size(13.0).strong().color(theme::FG()));
    ui.add_space(5.0);
}

fn caption(ui: &mut Ui, text: &str) {
    ui.add(Label::new(RichText::new(text).size(12.0).color(theme::FG_SOFT())).wrap());
    ui.add_space(6.0);
}

/// The address of the VNC desktop of this viewer, for the row of its connection.
/// The kind of source that one viewer shows, for the header of the panel.
fn transport_label(device: &DevicePanelState) -> &'static str {
    if device.target.is_frame_source() {
        "Frame source program"
    } else if device.ssh_tunnel.is_some() {
        "VNC desktop over SSH"
    } else {
        "VNC desktop"
    }
}

/// The word for the name or the text that the source reported: a VNC server names
/// its desktop, and a frame source program names itself.
fn reported_name_label(frame_source: bool) -> &'static str {
    if frame_source { "Source name" } else { "VNC name" }
}

/// The mode of the input of this viewer: a person's mouse and keyboard go to the
/// program of a frame source viewer, and to the desktop of a VNC viewer.
fn mode_line(frame_source: bool, interact: bool) -> &'static str {
    if !interact {
        return "Read-only";
    }
    if frame_source {
        "Interactive: your mouse and keyboard go to the program"
    } else {
        "Interactive: your mouse and keyboard go to the desktop"
    }
}

fn endpoint_address(device: &DevicePanelState) -> String {
    device
        .target
        .vnc_address()
        .map(|address| address.to_string())
        .unwrap_or_default()
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

fn connection(
    ui: &mut Ui,
    device: &DevicePanelState,
    server: &DeviceServerDetails,
    connected: bool,
    stream: Option<horizon_frame_source::Stats>,
    input_error: Option<&str>,
) {
    section(ui, "Connection");
    // A frame source viewer names the program of the settings and not an address,
    // and it has no tunnel.
    if let Some(reference) = device.target.frame_source_ref() {
        row(ui, "Frame source", &reference.name);
        if let Some(target) = &reference.target {
            row(ui, "Target", target);
        }
    } else {
        match &device.ssh_tunnel {
            Some(tunnel) => {
                row(ui, "SSH host", &tunnel.display_label());
                if let Some(port) = tunnel.port {
                    row(ui, "SSH port", &port.to_string());
                }
                row(ui, "Remote endpoint", &endpoint_address(device));
            }
            None => row(ui, "Local endpoint", &endpoint_address(device)),
        }
    }
    if let Some(stream) = stream {
        row(ui, "Dropped pictures", &stream.frames_dropped.to_string());
        row(ui, "Age of the newest picture", &format!("{} ms", stream.age_ms));
    }
    if let Some(error) = input_error {
        row(ui, "Last input refusal", error);
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
            row(ui, reported_name_label(device.target.is_frame_source()), name);
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
            ui.label(RichText::new(label).size(12.0).color(theme::FG_SOFT()));
        });
        ui.add(
            Label::new(RichText::new(value).size(13.0).monospace())
                .wrap()
                .selectable(true),
        );
    });
    ui.add_space(3.0);
}

/// Collect the reconnect action without mutating the connection during rendering.
pub(super) fn header(
    ui: &mut Ui,
    device: &DevicePanelState,
    server: &DeviceServerDetails,
    native_labels: &NativeLabels,
    status: &super::session::Status,
    interactive: bool,
    interact: bool,
) -> bool {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.add_space(8.0);
        let endpoint = device.endpoint_label();
        let frame_source = device.target.is_frame_source();
        let transport = transport_label(device);
        // A frame source viewer is named by its program in the settings, and the
        // text that the program gives itself is a row of the connection details.
        let name = if server.native_session.is_some() {
            Some(native_labels.device.as_str())
        } else if frame_source {
            device.display_name(None)
        } else {
            device.display_name(server.name.as_deref())
        };
        let reconnect = ui
            .horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                if ui.available_width() >= 300.0 {
                    desktop_icon(ui);
                }
                let width = (ui.available_width() - 100.0).max(24.0);
                ui.allocate_ui_with_layout(vec2(width, 40.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_min_width(width);
                    ui.add(Label::new(RichText::new(name.unwrap_or(&endpoint)).size(20.0).strong()).truncate())
                        .on_hover_text(name.unwrap_or(&endpoint));
                    ui.add(
                        Label::new(
                            RichText::new(if server.native_session.is_some() {
                                &native_labels.lane
                            } else if name.is_some() {
                                &endpoint
                            } else {
                                transport
                            })
                            .size(12.0)
                            .color(theme::FG_SOFT()),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&endpoint);
                });
                ui.allocate_ui_with_layout(vec2(88.0, 40.0), Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled(interactive, egui::Button::new("Reconnect").min_size(vec2(84.0, 28.0)))
                        .clicked()
                })
                .inner
            })
            .inner;
        if let Some(native) = &server.native_session {
            ui.add(Label::new(&native_labels.app).wrap())
                .on_hover_text(&native.build_sha256);
            if !native_labels.progress.is_empty() {
                ui.add(Label::new(&native_labels.progress).wrap());
            }
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            connection_status(ui, status);
            let mode = mode_line(frame_source, interact);
            ui.label(RichText::new(mode).size(12.0).color(theme::FG_SOFT()));
            if name.is_some() {
                let source = if server.native_session.is_some() {
                    "Native host"
                } else if device.display_name(None).is_some() {
                    "Supplied name"
                } else {
                    "VNC name"
                };
                ui.label(RichText::new(source).size(12.0).color(theme::FG_SOFT()));
            }
        });
        match status {
            Status::Disconnected { message, .. } => {
                ui.add_space(3.0);
                ui.add(Label::new(RichText::new(message).size(12.0).color(theme::PALETTE_RED())).wrap());
            }
            Status::Stopped => {
                ui.add_space(3.0);
                caption(ui, "Select Reconnect to view this desktop.");
            }
            Status::Connecting | Status::Connected => {}
        }
        ui.add_space(8.0);
        reconnect
    })
    .inner
}

fn connection_status(ui: &mut Ui, status: &super::session::Status) {
    let (label, color) = match status {
        Status::Stopped => ("Stopped", theme::FG_SOFT()),
        Status::Connecting => ("Connecting…", theme::PALETTE_YELLOW()),
        Status::Connected => ("Connected", theme::PALETTE_GREEN()),
        Status::Disconnected { .. } => ("Disconnected", theme::PALETTE_RED()),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(vec2(8.0, 12.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.0, color);
        ui.label(RichText::new(label).size(12.0).color(color));
    });
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

    fn frame_source_device() -> DevicePanelState {
        DevicePanelState {
            target: horizon_core::DeviceViewTarget::frame_source(horizon_core::FrameSourceRef {
                name: "standin".into(),
                target: None,
            }),
            input: horizon_core::DeviceInput::Accepted,
            identity: None,
            connect_on_start: false,
            ssh_tunnel: None,
        }
    }

    #[test]
    fn a_frame_source_is_named_as_a_program_and_not_as_a_vnc_desktop() {
        assert_eq!(transport_label(&frame_source_device()), "Frame source program");
        assert_eq!(
            transport_label(&DevicePanelState {
                target: horizon_core::DeviceViewTarget::parse("127.0.0.1:5900").unwrap(),
                ..frame_source_device()
            }),
            "VNC desktop"
        );
        assert_eq!(reported_name_label(true), "Source name");
        assert_eq!(reported_name_label(false), "VNC name");
    }
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
