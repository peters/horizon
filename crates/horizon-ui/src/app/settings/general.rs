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
    changed |= render_overlays_section(ui, config);
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
                changed |= optional_text_field(
                    ui,
                    &mut config.browser_host.ssh_target,
                    "user@host or an SSH alias",
                    "settings_browser_host_target",
                );
                ui.end_row();

                ui.label(egui::RichText::new("Profile").color(theme::FG_SOFT()).size(12.0));
                changed |= optional_text_field(
                    ui,
                    &mut config.browser_host.profile,
                    "profile name on that machine",
                    "settings_browser_host_profile",
                );
                ui.end_row();
            });
        validation_notice(ui, config.browser_host.validate());
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
                changed |= optional_text_field(
                    ui,
                    &mut config.server.ssh_target,
                    "user@host or an SSH alias",
                    "settings_server_target",
                );
                ui.end_row();

                ui.label(egui::RichText::new("Command").color(theme::FG_SOFT()).size(12.0));
                changed |= optional_text_field(
                    ui,
                    &mut config.server.command,
                    horizon_core::DEFAULT_LINK_COMMAND,
                    "settings_server_command",
                );
                ui.end_row();
            });
        validation_notice(ui, config.server.validate());
    });
    changed
}

/// One field of an entry that a person may leave blank. The box edits a copy,
/// so a blank box means the entry is not set; an entry that is not set gives an
/// empty copy, so the common case allocates nothing. The value is written back
/// only when the text changed, never on every frame.
fn optional_text_field(ui: &mut Ui, value: &mut Option<String>, hint: &str, id: &str) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text(hint)
            .desired_width((ui.available_width() - ui.spacing().item_spacing.x).max(120.0))
            .id_salt(id),
    );
    if !response.changed() {
        return false;
    }
    let trimmed = text.trim();
    *value = (!trimmed.is_empty()).then(|| trimmed.to_string());
    true
}

/// The refusal of an entry, in the words of its rule.
fn validation_notice(ui: &mut Ui, result: horizon_core::Result<()>) {
    if let Err(error) = result {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(error.to_string())
                .color(theme::PALETTE_RED())
                .size(11.0),
        );
    }
}

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
