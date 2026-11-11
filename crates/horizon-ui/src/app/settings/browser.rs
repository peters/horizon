//! The Browser tab: the engine of the browser panels that start on this
//! computer.
//!
//! These settings are read when a browser panel starts and when a recording
//! starts, so a change applies to the panels and the recordings of the panels
//! that start after it. Every field writes the typed value of its section and
//! shows the rule of that section when it refuses the value, so a person never
//! saves a setting that the engine would refuse.

use egui::Ui;
use horizon_core::Config;
use horizon_core::browser::{
    MAX_VIDEO_COMPRESSION_LEVEL, MAX_VIDEO_FILE_BYTES, MAX_VIDEO_FPS, MAX_VIDEO_MAX_WIDTH, MAX_VIDEO_QUALITY,
    MIN_VIDEO_FPS, MIN_VIDEO_MAX_WIDTH, MIN_VIDEO_QUALITY,
};

use crate::theme;

/// The bytes of one mebibyte, which is the unit of the capture file limit in the
/// settings window. The store holds bytes.
const MEBIBYTE: u64 = 1024 * 1024;

/// The largest capture file limit, in mebibytes.
const MAX_FILE_MEBIBYTES: u64 = MAX_VIDEO_FILE_BYTES / MEBIBYTE;

/// The largest number of frames between two published frames of the screencast,
/// so a person cannot make a live view that no one sees.
const MAX_EVERY_NTH_FRAME: u32 = 60;

/// Render the Browser tab: the programs of the engines, the live view of a
/// panel, and the default options of a recording.
pub(super) fn render(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    changed |= render_programs_section(ui, config);
    changed |= render_live_view_section(ui, config);
    changed |= render_video_section(ui, config);
    changed |= render_passkey_section(ui, config);
    changed
}

/// The executables of the three engines, and the two arguments that a person may
/// need when an engine is not on the `PATH` of this computer.
fn render_programs_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Engines");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "The programs that a browser panel starts. A blank field uses the program that this \
             computer finds on its PATH.",
        );
        ui.add_space(8.0);

        egui::Grid::new("settings_browser_programs_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                for (label, value, hint, id) in [
                    (
                        "Chromium",
                        &mut config.browser.command,
                        "/usr/bin/chromium",
                        "settings_browser_chromium",
                    ),
                    (
                        "Firefox",
                        &mut config.browser.firefox_command,
                        "/usr/bin/firefox",
                        "settings_browser_firefox",
                    ),
                    (
                        "Geckodriver",
                        &mut config.browser.geckodriver_command,
                        "/usr/bin/geckodriver",
                        "settings_browser_geckodriver",
                    ),
                    (
                        "Safaridriver",
                        &mut config.browser.safaridriver_command,
                        "/usr/bin/safaridriver",
                        "settings_browser_safaridriver",
                    ),
                ] {
                    ui.label(egui::RichText::new(label).color(theme::FG_SOFT()).size(12.0));
                    changed |= super::optional_text_field(ui, value, hint, id);
                    ui.end_row();
                }
            });
        super::validation_notice(ui, config.browser.validate());
    });
    changed
}

/// The live view of a browser panel: the JPEG quality of the screencast and how
/// many frames it skips.
fn render_live_view_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Live view");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "The picture of a browser panel, as this computer receives it from the engine.",
        );
        ui.add_space(8.0);

        egui::Grid::new("settings_browser_live_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("Picture quality")
                        .color(theme::FG_SOFT())
                        .size(12.0),
                );
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.browser.quality)
                            .range(1..=MAX_VIDEO_QUALITY)
                            .clamp_existing_to_range(false)
                            .speed(1.0)
                            .suffix(" of 100"),
                    )
                    .changed();
                ui.end_row();

                ui.label(egui::RichText::new("Skip frames").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.browser.every_nth_frame)
                            .range(1..=MAX_EVERY_NTH_FRAME)
                            .clamp_existing_to_range(false)
                            .speed(1.0)
                            .suffix(" of every"),
                    )
                    .changed();
                ui.end_row();
            });
        super::validation_notice(ui, config.browser.validate());
    });
    changed
}

/// The default options of a recording that an agent starts in a browser panel.
fn render_video_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Recording");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "The defaults of a WebM recording of a page. A recording may name other values, and a \
             change here applies to the recordings that start after it.",
        );
        ui.add_space(8.0);

        egui::Grid::new("settings_browser_video_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Quality").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.browser.video.quality)
                            .range(MIN_VIDEO_QUALITY..=MAX_VIDEO_QUALITY)
                            .clamp_existing_to_range(false)
                            .speed(1.0)
                            .suffix(" of 100"),
                    )
                    .changed();
                ui.end_row();

                ui.label(
                    egui::RichText::new("Frames per second")
                        .color(theme::FG_SOFT())
                        .size(12.0),
                );
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.browser.video.fps)
                            .range(MIN_VIDEO_FPS..=MAX_VIDEO_FPS)
                            .clamp_existing_to_range(false)
                            .speed(1.0),
                    )
                    .changed();
                ui.end_row();

                ui.label(egui::RichText::new("Compression").color(theme::FG_SOFT()).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.browser.video.compression_level)
                            .range(0..=MAX_VIDEO_COMPRESSION_LEVEL)
                            .clamp_existing_to_range(false)
                            .speed(1.0)
                            .suffix(" of 10"),
                    )
                    .changed();
                ui.end_row();

                ui.label(egui::RichText::new("Longest side").color(theme::FG_SOFT()).size(12.0));
                changed |= optional_max_width(ui, config);
                ui.end_row();
            });
        ui.add_space(8.0);
        changed |= file_limit(ui, config);
        super::validation_notice(ui, config.browser.validate());
    });
    changed
}

/// The longest side of a recorded frame: a box that a person ticks, and a width
/// that only an enabled box edits.
fn optional_max_width(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    let video = &mut config.browser.video;
    let mut enabled = video.max_width.is_some();
    ui.horizontal(|ui| {
        if ui.checkbox(&mut enabled, "").changed() {
            video.max_width = enabled.then_some(MAX_VIDEO_MAX_WIDTH);
            changed = true;
        }
        if let Some(width) = video.max_width.as_mut() {
            changed |= ui
                .add(
                    egui::DragValue::new(width)
                        .range(MIN_VIDEO_MAX_WIDTH..=MAX_VIDEO_MAX_WIDTH)
                        .clamp_existing_to_range(false)
                        .speed(1.0)
                        .suffix(" px"),
                )
                .changed();
        } else {
            super::dim_label(ui, "the size of the page");
        }
    });
    changed
}

/// The largest recording file, in mebibytes. The store holds bytes.
fn file_limit(ui: &mut Ui, config: &mut Config) -> bool {
    let mut mebibytes = config.browser.video.max_file_bytes / MEBIBYTE;
    let response = ui.add(
        egui::DragValue::new(&mut mebibytes)
            .range(1..=MAX_FILE_MEBIBYTES)
            .clamp_existing_to_range(false)
            .speed(1.0)
            .suffix(" MiB"),
    );
    super::dim_label(ui, "The recording stops when its file reaches this size.");
    if !response.changed() {
        return false;
    }
    config.browser.video.max_file_bytes = mebibytes * MEBIBYTE;
    true
}

/// The command of an external authenticator, for the passkey bridge of a page.
fn render_passkey_section(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Passkeys");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "The command of an external WebAuthn authenticator. A browser panel of this computer \
             sends the passkey requests of a page to it. An empty field leaves the passkeys of the \
             engine alone. Write one argument on each line, so a path with a space stays one \
             argument.",
        );
        ui.add_space(8.0);

        let mut text = config.browser.webauthn_command.join("\n");
        let width = ui.available_width();
        let response = ui.add(
            egui::TextEdit::multiline(&mut text)
                .hint_text("/usr/local/bin/authenticator")
                .desired_width(width)
                .desired_rows(2)
                .id_salt("settings_browser_webauthn"),
        );
        if response.changed() {
            config.browser.webauthn_command = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect();
            changed = true;
        }
    });
    changed
}

#[cfg(test)]
mod tests;
