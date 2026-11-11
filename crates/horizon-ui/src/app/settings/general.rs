use egui::Ui;
use horizon_core::{AppearanceTheme, Config};

use crate::theme;

/// Render the General settings tab: window dimensions, feature toggles,
/// the machine entries and overlay sizes.  Returns `true` when any value was
/// modified.
pub(super) fn render(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    changed |= render_window_section(ui, config);
    changed |= render_appearance_section(ui, config);
    changed |= render_features_section(ui, config);
    changed |= render_machines_section(ui, config);
    changed |= render_remote_hosts_section(ui, config);
    changed |= render_overlays_section(ui, config);
    changed
}

/// Where a panel of the remote hosts chooser lands, and the port of the VNC
/// server that `ssh -W` reaches on that host.
///
/// The per-host ports are rows: a host or an SSH alias, and its port. A row that
/// a person clears goes, and the last row adds another one.
fn render_remote_hosts_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Remote hosts");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "A terminal or a VNC panel that the remote hosts chooser opens lands in this workspace. \
             The VNC server of a host listens on its own loopback interface, and Horizon reaches it \
             with ssh.",
        );
        ui.add_space(8.0);

        egui::Grid::new("settings_remote_hosts_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Workspace").color(theme::FG_SOFT()).size(12.0));
                changed |= super::text_field(
                    ui,
                    &mut config.remote_hosts.default_workspace,
                    "Remote Sessions",
                    "settings_remote_hosts_workspace",
                );
                ui.end_row();

                ui.label(egui::RichText::new("VNC port").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.remote_hosts.vnc_port)
                            .range(1..=u16::MAX)
                            .clamp_existing_to_range(false)
                            .speed(1.0),
                    )
                    .changed();
                ui.end_row();
            });

        ui.add_space(10.0);
        super::dim_label(ui, "A port for one host, by its label or its SSH host name:");
        changed |= render_vnc_ports(ui, config);
        super::validation_notice(ui, config.remote_hosts.validate());
    });
    changed
}

/// The per-host VNC ports, one row each, with an empty row to add one.
fn render_vnc_ports(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    // The keys are read back after the rows are drawn, so a rename of a row is
    // written whole instead of row by row.
    let mut ports: Vec<(String, u16)> = config
        .remote_hosts
        .vnc_ports
        .iter()
        .map(|(host, port)| (host.clone(), *port))
        .collect();
    ports.push((String::new(), config.remote_hosts.vnc_port));

    egui::Grid::new("settings_remote_hosts_ports_grid")
        .num_columns(3)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            let mut remove = None;
            for (index, (host, port)) in ports.iter_mut().enumerate() {
                let host_id = format!("settings_remote_hosts_port_host_{index}");
                changed |= super::text_field(ui, host, "", &host_id);
                changed |= ui
                    .add(
                        egui::DragValue::new(port)
                            .range(1..=u16::MAX)
                            .clamp_existing_to_range(false)
                            .speed(1.0),
                    )
                    .changed();
                // The empty row adds a host; a row with a name can go.
                if host.trim().is_empty() {
                    ui.label("");
                } else if ui.button("Remove").clicked() {
                    remove = Some(index);
                }
                ui.end_row();
            }
            if let Some(index) = remove {
                ports.remove(index);
                changed = true;
            }
        });

    if changed {
        config.remote_hosts.vnc_ports = ports
            .into_iter()
            .filter(|(host, _)| !host.trim().is_empty())
            .map(|(host, port)| (host.trim().to_owned(), port))
            .collect();
    }
    changed
}

/// The two entries that say which machine serves the browser panels and which
/// computer holds the machine list. Both apply at the next start.
fn render_machines_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Browser host");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "Browser panels open in that profile of Chromium on that machine, over SSH. A blank SSH \
             target keeps the browser panels of this computer. A change applies at the next start.",
        );
        ui.add_space(8.0);

        egui::Grid::new("settings_browser_host_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("SSH target").color(theme::FG_SOFT()).size(12.0));
                changed |= super::optional_text_field(
                    ui,
                    &mut config.browser_host.ssh_target,
                    "user@host or an SSH alias",
                    "settings_browser_host_target",
                );
                ui.end_row();

                ui.label(egui::RichText::new("Profile").color(theme::FG_SOFT()).size(12.0));
                changed |= super::optional_text_field(
                    ui,
                    &mut config.browser_host.profile,
                    "profile name on that machine",
                    "settings_browser_host_profile",
                );
                ui.end_row();
            });
        super::validation_notice(ui, config.browser_host.validate());
    });

    super::section_heading(ui, "Server");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "Other computers of yours that name the same server show the same machines, keys and \
             settings. A blank SSH target keeps the machine list on this computer. A change applies \
             at the next start.",
        );
        ui.add_space(8.0);

        egui::Grid::new("settings_server_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("SSH target").color(theme::FG_SOFT()).size(12.0));
                changed |= super::optional_text_field(
                    ui,
                    &mut config.server.ssh_target,
                    "user@host or an SSH alias",
                    "settings_server_target",
                );
                ui.end_row();

                ui.label(egui::RichText::new("Command").color(theme::FG_SOFT()).size(12.0));
                changed |= super::optional_text_field(
                    ui,
                    &mut config.server.command,
                    horizon_core::DEFAULT_LINK_COMMAND,
                    "settings_server_command",
                );
                ui.end_row();
            });
        super::validation_notice(ui, config.server.validate());
    });
    changed
}

/// One field that a person may leave blank, and writes back only when the text
/// One field of an entry that a person may leave blank. The box edits a copy,
/// so a blank box means the entry is not set; an entry that is not set gives an
/// empty copy, so the common case allocates nothing. The value is written back
fn render_window_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Window");
    super::section_card(ui, |ui| {
        super::dim_label(ui, "Default window size and position on launch.");
        ui.add_space(8.0);

        egui::Grid::new("settings_window_grid")
            .num_columns(4)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Width").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.window.width)
                            .range(400.0..=8000.0)
                            .speed(2.0)
                            .suffix(" px"),
                    )
                    .changed();

                ui.label(egui::RichText::new("Height").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.window.height)
                            .range(300.0..=5000.0)
                            .speed(2.0)
                            .suffix(" px"),
                    )
                    .changed();
                ui.end_row();

                ui.label(egui::RichText::new("X").color(theme::FG_SOFT()).size(12.0));
                changed |= optional_f32_drag(ui, &mut config.window.x, "px");

                ui.label(egui::RichText::new("Y").color(theme::FG_SOFT()).size(12.0));
                changed |= optional_f32_drag(ui, &mut config.window.y, "px");
                ui.end_row();
            });
    });
    changed
}

fn render_appearance_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Appearance");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "Follow the system theme by default, or force a light or dark override.",
        );
        ui.add_space(8.0);

        egui::ComboBox::from_id_salt("settings_appearance_theme")
            .selected_text(theme_label(config.appearance.theme))
            .show_ui(ui, |ui| {
                changed |= ui
                    .selectable_value(
                        &mut config.appearance.theme,
                        AppearanceTheme::Auto,
                        theme_label(AppearanceTheme::Auto),
                    )
                    .changed();
                changed |= ui
                    .selectable_value(
                        &mut config.appearance.theme,
                        AppearanceTheme::Dark,
                        theme_label(AppearanceTheme::Dark),
                    )
                    .changed();
                changed |= ui
                    .selectable_value(
                        &mut config.appearance.theme,
                        AppearanceTheme::Light,
                        theme_label(AppearanceTheme::Light),
                    )
                    .changed();
            });
    });
    changed
}

fn render_features_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Features");
    super::section_card(ui, |ui| {
        changed |= ui
            .checkbox(
                &mut config.features.organize_workspaces_on_session_load,
                egui::RichText::new("Organize Workspaces on Session Load")
                    .color(theme::FG())
                    .size(12.0),
            )
            .changed();
        super::dim_label(
            ui,
            "Align attached workspaces horizontally in sidebar order whenever a session loads, including at startup. Detached workspaces are unchanged.",
        );

        ui.add_space(10.0);
        changed |= ui
            .checkbox(
                &mut config.features.sidebar_accordion,
                egui::RichText::new("Accordion Sidebar").color(theme::FG()).size(12.0),
            )
            .changed();
        super::dim_label(
            ui,
            "Only the active workspace lists panels in the sidebar. Other workspaces collapse to a name and count.",
        );
    });
    changed
}

fn render_overlays_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Overlays");
    super::section_card(ui, |ui| {
        super::dim_label(ui, "Dimensions of overlay widgets on the canvas.");
        ui.add_space(8.0);

        egui::Grid::new("settings_overlays_grid")
            .num_columns(4)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Map Width").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.overlays.minimap_width)
                            .range(80.0..=600.0)
                            .speed(1.0)
                            .suffix(" px"),
                    )
                    .changed();

                ui.label(egui::RichText::new("Map Height").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.overlays.minimap_height)
                            .range(60.0..=500.0)
                            .speed(1.0)
                            .suffix(" px"),
                    )
                    .changed();
                ui.end_row();
            });
    });
    changed
}

/// Render a `DragValue` for an `Option<f32>`.  An unchecked checkbox
/// clears the value to `None`.
fn optional_f32_drag(ui: &mut Ui, value: &mut Option<f32>, suffix: &str) -> bool {
    let mut changed = false;
    let mut enabled = value.is_some();

    if ui.checkbox(&mut enabled, "").changed() {
        *value = if enabled { Some(0.0) } else { None };
        changed = true;
    }

    if let Some(v) = value.as_mut() {
        changed |= ui
            .add(
                egui::DragValue::new(v)
                    .range(-10000.0..=10000.0)
                    .speed(1.0)
                    .suffix(format!(" {suffix}")),
            )
            .changed();
    }

    changed
}

fn theme_label(theme: AppearanceTheme) -> &'static str {
    match theme {
        AppearanceTheme::Auto => "Auto (system)",
        AppearanceTheme::Dark => "Dark",
        AppearanceTheme::Light => "Light",
    }
}
