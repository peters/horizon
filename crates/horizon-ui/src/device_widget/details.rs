//! Compact, selectable connection facts kept separate from image/view controls.
use egui::{Label, RichText, Ui};
use horizon_core::{DevicePanelState, browser::manifest::device::DeviceServerDetails};

pub(super) fn show(ui: &mut Ui, device: &DevicePanelState, server: &DeviceServerDetails, connected: bool) {
    ui.collapsing("Connection details", |ui| {
        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
            row(ui, "Local endpoint", &device.target.address().to_string());
            if let Some(identity) = &device.identity {
                ui.weak("Supplied by session creator");
                for (label, value) in [
                    ("Machine", identity.machine_name.as_deref()),
                    ("Hostname", identity.hostname.as_deref()),
                    ("Tailscale", identity.tailscale_name.as_deref()),
                ] {
                    if let Some(value) = value {
                        row(ui, label, value);
                    }
                }
                for address in &identity.ip_addresses {
                    row(ui, "Remote IP", &address.to_string());
                }
            }
            if server.name.is_some() || server.desktop_size.is_some() {
                ui.weak(if connected {
                    "Server-reported"
                } else {
                    "Server-reported (disconnected)"
                });
                if let Some(name) = &server.name {
                    row(ui, "VNC name", name);
                }
                if let Some([width, height]) = server.desktop_size {
                    row(ui, "Desktop", &format!("{width} × {height}"));
                }
            }
            if device.display_name(server.name.as_deref()).is_none() {
                ui.weak("Machine name unavailable");
            }
        });
    });
}

fn row(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        ui.add(Label::new(RichText::new(value).monospace()).wrap().selectable(true));
    });
}

/// Collect the reconnect action without mutating the connection during rendering.
pub(super) fn header(
    ui: &mut Ui,
    device: &DevicePanelState,
    server: &DeviceServerDetails,
    status: &super::session::Status,
    interactive: bool,
) -> bool {
    use super::session::Status;
    ui.horizontal_wrapped(|ui| {
        if let Some(name) = device.display_name(server.name.as_deref()) {
            let source = if device.display_name(None).is_some() {
                "Supplied name"
            } else {
                "VNC name"
            };
            ui.add(egui::Label::new(egui::RichText::new(name).strong()).truncate())
                .on_hover_text(name);
            ui.weak(source);
        } else {
            ui.monospace(device.target.address().to_string());
        }
        ui.label("Read-only");
        let reconnect = ui.add_enabled(interactive, egui::Button::new("Reconnect")).clicked();
        match status {
            Status::Stopped => {
                ui.label("Stopped — select Reconnect");
            }
            Status::Connecting => {
                ui.spinner();
                ui.label("Connecting…");
            }
            Status::Connected => {
                ui.label("Connected");
            }
            Status::Disconnected(error) => {
                ui.colored_label(ui.visuals().error_fg_color, format!("Disconnected: {error}"));
            }
        }
        reconnect
    })
    .inner
}
