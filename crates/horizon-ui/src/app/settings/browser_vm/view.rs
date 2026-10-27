//! The cards of the Browser VM section: the machine, the rent form and the sign-in step.
//!
//! Every card draws the decisions of [`super::Offered`], so a test of those decisions is a
//! test of what the person sees, and a press only records an action for the window.
use egui::Ui;
use horizon_core::Config;
use horizon_core::browser::manifest::provider_usage::BrowserVmAction;
use horizon_core::cloud_runtime::browser_vm::Phase;

use super::super::{dim_label, section_card, section_heading};
use super::{Offered, Panel, hourly_line, phase_label};
use crate::theme;

/// Draws the section and records the presses. Returns whether the settings entry changed,
/// which the caller writes into the YAML buffer and the settings bar saves.
pub(in crate::app) fn render(ui: &mut Ui, config: &mut Config, panel: &mut Panel) -> bool {
    let offered = panel.offered();
    machine_card(ui, panel, &offered);
    rent_card(ui, panel, &offered);
    let mut changed = false;
    if offered.sign_in {
        changed = sign_in_card(ui, config, panel, &offered);
    }
    changed
}

/// The phase, the machine and its hourly price, with the run buttons.
fn machine_card(ui: &mut Ui, panel: &mut Panel, offered: &Offered) {
    section_heading(ui, "Browser VM");
    section_card(ui, |ui| {
        dim_label(
            ui,
            "One browser machine for this Horizon. Its state volume at /state keeps the sites that a \
             person signs in to, across a stop and a resume.",
        );
        ui.add_space(8.0);
        machine_fields(ui, panel);
        dim_label(
            ui,
            "The Hetzner price is net of VAT and covers the server type only. The state volume, the \
             primary IPv4 address and traffic over the included amount bill separately.",
        );
        if let Some(error) = &panel.error {
            ui.add_space(8.0);
            ui.add(egui::Label::new(egui::RichText::new(error).color(theme::PALETTE_RED()).size(11.0)).wrap());
        }
        if let Some(reason) = panel.failure() {
            ui.add_space(8.0);
            ui.add(egui::Label::new(egui::RichText::new(reason).color(theme::PALETTE_RED()).size(11.0)).wrap());
        }
        if let Some(note) = panel.ssh_note() {
            ui.add_space(8.0);
            dim_label(ui, note);
        }
        if panel.busy {
            ui.add_space(8.0);
            dim_label(
                ui,
                "A browser VM run is in progress. Start, Stop, Resume and Delete wait for it.",
            );
        }
        ui.add_space(10.0);
        run_buttons(ui, panel, offered);
        delete_confirmation(ui, panel, offered);
    });
}

fn machine_fields(ui: &mut Ui, panel: &Panel) {
    let status = panel.status.as_ref();
    let phase = status.map_or("unknown", |status| phase_label(status.phase));
    let server_type = status
        .and_then(|status| status.server_type.as_deref())
        .unwrap_or("none");
    let location = status.and_then(|status| status.location.as_deref()).unwrap_or("none");
    let volume = status
        .and_then(|status| status.volume_gb)
        .map_or_else(|| "none".to_string(), |gb| format!("{gb} GB"));
    let hourly = status
        .and_then(|status| status.hourly_net)
        .map_or_else(|| "not known yet".to_string(), hourly_line);
    let fields = [
        ("Phase", phase.to_string()),
        ("Machine size", server_type.to_string()),
        ("Location", location.to_string()),
        ("SSH target", panel.ssh_target().unwrap_or("none yet").to_string()),
        ("State volume", volume),
        ("Hourly cost", hourly),
    ];
    egui::Grid::new("settings_browser_vm_machine")
        .num_columns(2)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            for (label, value) in fields {
                ui.label(egui::RichText::new(label).color(theme::FG_SOFT()).size(12.0));
                ui.label(egui::RichText::new(value).color(theme::FG()).size(12.0).monospace());
                ui.end_row();
            }
        });
    ui.add_space(6.0);
}

fn run_buttons(ui: &mut Ui, panel: &mut Panel, offered: &Offered) {
    ui.horizontal_wrapped(|ui| {
        for (action, label, enabled) in [
            (BrowserVmAction::Status, "Refresh", offered.refresh),
            (BrowserVmAction::Stop, "Stop", offered.stop),
            (BrowserVmAction::Resume, "Resume", offered.resume),
        ] {
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                panel.ask(action);
            }
        }
        if offered.delete && !panel.confirms_delete && ui.add(egui::Button::new("Delete")).clicked() {
            panel.arm_delete();
        }
    });
}

/// The second press of a delete, inside the section and never in a modal window.
fn delete_confirmation(ui: &mut Ui, panel: &mut Panel, offered: &Offered) {
    // A run in flight takes every provider action away, including a delete that was armed
    // before it started.
    if !panel.confirms_delete || !offered.delete {
        return;
    }
    ui.add_space(8.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new("Delete removes the server and the state volume. Every saved sign-in is lost.")
                .color(theme::PALETTE_RED())
                .size(11.0),
        )
        .wrap(),
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Delete the machine and its volume").clicked() {
            panel.ask(BrowserVmAction::Delete);
        }
        if ui.button("Keep the machine").clicked() {
            panel.cancel_delete();
        }
    });
}

/// The fields of a rent, and the start that sends them.
fn rent_card(ui: &mut Ui, panel: &mut Panel, offered: &Offered) {
    section_heading(ui, "Rent a machine");
    section_card(ui, |ui| {
        dim_label(
            ui,
            "A start rents a dedicated ccx server and its state volume. The image must name its \
             digest, so a machine cannot change what it runs between two rents.",
        );
        ui.add_space(8.0);
        let edited = rent_fields(ui, panel);
        panel.edited |= edited;
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            let start = egui::Button::new("Start").min_size(egui::Vec2::new(80.0, 0.0));
            if ui.add_enabled(offered.start, start).clicked() {
                panel.ask(BrowserVmAction::Deploy);
            }
            if !offered.start {
                dim_label(ui, start_wait(panel));
            }
        });
    });
}

fn rent_fields(ui: &mut Ui, panel: &mut Panel) -> bool {
    let mut edited = false;
    egui::Grid::new("settings_browser_vm_rent")
        .num_columns(2)
        .spacing([14.0, 8.0])
        .show(ui, |ui| {
            edited |= field(ui, "Image", &mut panel.image, "registry/name@sha256:...");
            edited |= field(ui, "Server type", &mut panel.server_type, "ccx13");
            edited |= field(ui, "Location", &mut panel.location, "the first allowed location");
            ui.label(egui::RichText::new("State volume").color(theme::FG_SOFT()).size(12.0));
            edited |= ui
                .add(egui::DragValue::new(&mut panel.volume_gb).suffix(" GB").speed(1.0))
                .changed();
            ui.end_row();
        });
    edited
}

/// One text field of the rent form, with the entry that an empty field means.
fn field(ui: &mut Ui, label: &str, value: &mut String, hint: &str) -> bool {
    ui.label(egui::RichText::new(label).color(theme::FG_SOFT()).size(12.0));
    let changed = ui
        .add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(ui.available_width().max(160.0))
                .font(egui::TextStyle::Monospace),
        )
        .changed();
    ui.end_row();
    changed
}

/// Why the start waits, in the words of the phase the record names.
fn start_wait(panel: &Panel) -> &'static str {
    if panel.busy {
        return "A browser VM run is in progress; the start waits for it.";
    }
    match panel.phase() {
        Some(Phase::Ready) => "The machine runs. Stop it before you rent another one.",
        Some(Phase::Stopped) => "The machine is stopped. Resume it, so its sign-ins stay.",
        Some(Phase::Stopping | Phase::Resuming | Phase::Deleting) => {
            "A run is recorded that no run of this window performs. Delete removes the machine that it names."
        }
        None | Some(Phase::Idle | Phase::Deleted | Phase::Failed | Phase::Deploying) => "The start is available.",
    }
}

/// The sign-in step of a ready machine, with the Device panel and the settings write.
fn sign_in_card(ui: &mut Ui, config: &mut Config, panel: &mut Panel, offered: &Offered) -> bool {
    let mut changed = false;
    section_heading(ui, "Sign in on the display");
    section_card(ui, |ui| {
        dim_label(
            ui,
            "A person must sign in one time. Open the Device panel of the machine and sign in to the \
             sites on its display. The state volume keeps those sign-ins. An agent cannot do this step.",
        );
        ui.add_space(8.0);
        let mut notice = None;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(offered.sign_in, egui::Button::new("Open Device panel"))
                .clicked()
            {
                panel.open_device_panel();
            }
            let is_new = panel.ssh_target() != config.browser_host.ssh_target();
            if ui
                .add_enabled(
                    offered.sign_in && is_new,
                    egui::Button::new("Use this host for new panels"),
                )
                .clicked()
            {
                match panel.use_host(config) {
                    Ok(applied) => changed |= applied,
                    Err(text) => notice = Some(text),
                }
            }
            if !is_new {
                dim_label(ui, "New browser panels already use this machine.");
            }
        });
        if let Some(notice) = notice {
            dim_label(ui, notice);
        }
        dim_label(ui, "The entry applies at the next start of Horizon.");
    });
    changed
}
