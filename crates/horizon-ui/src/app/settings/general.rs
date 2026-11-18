use egui::Ui;
use horizon_core::notify::NotificationSetting;
use horizon_core::{AppearanceTheme, BrowserHostEngine, Config, WindowCloseSetting};

use crate::theme;

/// What the General tab reads of this run beside the values of the settings.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct GeneralView<'a> {
    /// The sentence of the row of desktop notifications while this computer has
    /// no program that shows one, and `None` while it has that program.
    pub(super) missing_notification_program: Option<&'a str>,
}

/// Render the General settings tab: window dimensions, feature toggles,
/// the machine entries and overlay sizes.  Returns `true` when any value was
/// modified.
pub(super) fn render(ui: &mut Ui, config: &mut Config, view: GeneralView<'_>) -> bool {
    let mut changed = false;
    changed |= render_window_section(ui, config);
    changed |= render_appearance_section(ui, config);
    changed |= render_features_section(ui, config, view);
    changed |= render_machines_section(ui, config);
    changed |= render_remote_hosts_section(ui, config);
    changed |= render_canvas_section(ui, config);
    changed |= render_overlays_section(ui, config);
    changed
}

/// The floor of a Fit: the least zoom that fitting a workspace chooses, so the
/// text of its cards and its terminals stays readable. A workspace that does not
/// fit at this zoom is shown from its top left.
fn render_canvas_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Canvas");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "The least zoom that Fit of a workspace chooses. A workspace that does not fit at this \
             zoom keeps it and is shown from its top left, and the minimap shows where the rest of it is.",
        );
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Fit floor").color(theme::FG_SOFT()).size(12.0));
            changed |= ui
                .add(
                    egui::DragValue::new(&mut config.canvas.fit_floor_zoom)
                        .range(0.2..=1.0)
                        .speed(0.01)
                        .max_decimals(2),
                )
                .changed();
            super::dim_label(ui, "of the zoom of this canvas");
        });
        super::dim_label(
            ui,
            "0.85 keeps the smallest text of a card at about 9 screen points and the text of a \
             terminal at about 11.",
        );
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        changed |= ui
            .checkbox(&mut config.canvas.agent_lines, "Lines between an agent and its panels")
            .changed();
        super::dim_label(
            ui,
            "One thin curve from an agent panel to each card, page, terminal or panel that it \
             controls. The lines of the agent in focus are clear, the lines of every other agent \
             are faint, and at a zoom below 0.6 all of them are alike.",
        );
    });
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

/// The width of the port box of one "port for one host" row.
const PORT_FIELD_WIDTH: f32 = 64.0;
/// The width of the button that removes a host from its row.
const REMOVE_BUTTON_WIDTH: f32 = 74.0;
/// The space between two widgets of a row.
const ROW_GAP: f32 = 8.0;
/// The narrowest host box that still shows a host name of a few characters.
const HOST_FIELD_MIN_WIDTH: f32 = 140.0;
/// The widest host box, so the port and the button stay near the host.
const HOST_FIELD_MAX_WIDTH: f32 = 260.0;

/// The width of the host box of one "port for one host" row, from the width that
/// the settings panel gives the section: a host name needs more room than the
/// port and the button of its row.
pub(super) fn host_field_width(panel_width: f32) -> f32 {
    (panel_width - PORT_FIELD_WIDTH - REMOVE_BUTTON_WIDTH - 2.0 * ROW_GAP)
        .clamp(HOST_FIELD_MIN_WIDTH, HOST_FIELD_MAX_WIDTH)
}

/// One host box of a "port for one host" row. It is wide enough for an SSH host
/// name or a label, and its hint says what belongs in it.
fn host_field(ui: &mut Ui, host: &mut String, width: f32, id: &str) -> bool {
    let mut text = host.clone();
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text("Host label or SSH host name")
            .desired_width(width)
            .id_salt(id),
    );
    if !response.changed() {
        return false;
    }
    *host = text;
    true
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
    let host_width = host_field_width(ui.available_width());

    egui::Grid::new("settings_remote_hosts_ports_grid")
        .num_columns(3)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            let mut remove = None;
            for (index, (host, port)) in ports.iter_mut().enumerate() {
                let host_id = format!("settings_remote_hosts_port_host_{index}");
                changed |= host_field(ui, host, host_width, &host_id);
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

/// The entry that says which machine serves the browser panels, and the pointer to
/// the tab that holds the server of the machine list. The browser host serves the
/// panels that a create makes after the change.
fn render_machines_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = render_watch_section(ui, config);
    super::section_heading(ui, "Browser host");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "Browser panels open in that profile of that machine over SSH. A blank SSH target and no \
             machine keep the browser panels of this computer. A change serves each browser panel that \
             you open after it; a panel that is open now keeps its browser until you close it.",
        );
        ui.add_space(8.0);

        egui::Grid::new("settings_browser_host_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                // A machine of the list is named by that machine, and the connect uses
                // the login of a terminal of it. The person leaves that route here to
                // name an SSH target instead.
                if let Some(machine) = config.browser_host.machine().map(str::to_owned) {
                    ui.label(egui::RichText::new("Machine").color(theme::FG_SOFT()).size(12.0));
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&machine).color(theme::FG()).size(12.0));
                        super::dim_label(
                            ui,
                            "The connect uses the address, the user and the machine key that a terminal of \
                             that machine uses.",
                        );
                        if ui
                            .add(crate::app::util::chrome_button("Use an SSH target instead"))
                            .clicked()
                        {
                            config.browser_host.machine = None;
                            changed = true;
                        }
                    });
                    ui.end_row();
                }

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

                ui.label(egui::RichText::new("Engine").color(theme::FG_SOFT()).size(12.0));
                egui::ComboBox::from_id_salt("settings_browser_host_engine")
                    .selected_text(browser_host_engine_label(config.browser_host.engine))
                    .show_ui(ui, |ui| {
                        for engine in [BrowserHostEngine::Chromium, BrowserHostEngine::Firefox] {
                            changed |= ui
                                .selectable_value(
                                    &mut config.browser_host.engine,
                                    engine,
                                    browser_host_engine_label(engine),
                                )
                                .changed();
                        }
                    });
                ui.end_row();

                ui.label(egui::RichText::new("Driver program").color(theme::FG_SOFT()).size(12.0));
                changed |= super::optional_text_field(
                    ui,
                    &mut config.browser_host.command,
                    "horizon --browser-host-driver",
                    "settings_browser_host_command",
                );
                ui.end_row();
            });
        ui.add_space(6.0);
        super::dim_label(
            ui,
            "A Firefox host runs Firefox and geckodriver and needs a horizon binary of the same version \
             on that machine. Chromium needs a Chromium program there and no horizon.",
        );
        super::validation_notice(ui, config.browser_host.validate());
    });

    super::section_heading(ui, "Server");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "The machine list, the keys and the values of this computer follow a server. The tab \
             Server holds that entry, the join of this computer and the computers of the server.",
        );
    });
    changed
}

/// The entry that shows the agents of the machines of the machine list in the
/// sidebar, with no panel open.
fn render_watch_section(ui: &mut Ui, config: &mut Config) -> bool {
    super::section_heading(ui, "Machines");
    super::section_card(ui, |ui| {
        let changed = ui
            .checkbox(
                &mut config.machines.watch_agents,
                egui::RichText::new("Show the agents of each machine")
                    .color(theme::FG())
                    .size(12.0),
            )
            .changed();
        super::dim_label(
            ui,
            "Horizon asks each machine of the list for the agents that run there in tmux, and it \
             shows their state in the sidebar with no panel open: a machine whose agent waits for \
             you says so, and its row opens the terminal of that session. The machines of the list \
             and their keys are on the tab Server.",
        );
        changed
    })
}

/// The name of one engine of a browser host, as the settings window writes it.
fn browser_host_engine_label(engine: BrowserHostEngine) -> &'static str {
    match engine {
        BrowserHostEngine::Chromium => "Chromium",
        BrowserHostEngine::Firefox => "Firefox, with Horizon on that machine",
    }
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

fn render_features_section(ui: &mut Ui, config: &mut Config, view: GeneralView<'_>) -> bool {
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

        ui.add_space(10.0);
        changed |= ui
            .checkbox(
                &mut config.features.keep_agents_through_restart,
                egui::RichText::new("Keep agent programs running when Horizon restarts")
                    .color(theme::FG())
                    .size(12.0),
            )
            .changed();
        super::dim_label(
            ui,
            "The program of an agent panel runs in a small holder process of Horizon. Close the panel to end the program.",
        );

        ui.add_space(10.0);
        changed |= render_window_close_row(ui, config);

        ui.add_space(10.0);
        changed |= render_notification_row(ui, config, view);
    });
    changed
}

/// The row of what a close of the main window does: the three values of the
/// setting, and the sentence of each one.
fn render_window_close_row(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    ui.label(
        egui::RichText::new("When the window is closed")
            .color(theme::FG_SOFT())
            .size(12.0),
    );
    for setting in WindowCloseSetting::VALUES {
        changed |= ui
            .radio_value(
                &mut config.features.when_window_closes,
                setting,
                egui::RichText::new(setting.label()).color(theme::FG()).size(12.0),
            )
            .changed();
    }
    super::dim_label(
        ui,
        "Ask shows a question while a program of this computer runs. Keep Horizon running hides the window, and a start \
         shows it again. Quit Horizon ends every program of this computer.",
    );
    changed
}

/// The row of desktop notifications: the three values of the setting, and the
/// sentence of a computer that cannot show a notification.
fn render_notification_row(ui: &mut Ui, config: &mut Config, view: GeneralView<'_>) -> bool {
    let mut changed = false;
    ui.label(
        egui::RichText::new("Desktop notifications")
            .color(theme::FG_SOFT())
            .size(12.0),
    );
    for setting in NotificationSetting::VALUES {
        changed |= ui
            .radio_value(
                &mut config.features.notifications,
                setting,
                egui::RichText::new(setting.label()).color(theme::FG()).size(12.0),
            )
            .changed();
    }
    super::dim_label(
        ui,
        "One notification when an agent needs you or is done, while no Horizon window has the focus. \
         The sidebar still shows every state.",
    );
    if let Some(sentence) = view.missing_notification_program {
        super::dim_label(ui, sentence);
    }
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
