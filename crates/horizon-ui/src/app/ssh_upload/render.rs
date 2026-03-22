use egui::{Align2, Color32, Context, Id, RichText, ScrollArea, Stroke, Vec2};

use crate::{loading_spinner, theme};

use super::{
    SshUploadFlow, UploadMode, UploadTransportChoice, UploadUiAction, file_summary, human_bytes,
    join_remote_browser_path, progress_fraction, request_directory_listing,
};

pub(super) fn render_backdrop(ctx: &Context) {
    let screen_rect = ctx.input(egui::InputState::viewport_rect);
    egui::Area::new(Id::new("ssh_upload_backdrop"))
        .fixed_pos(screen_rect.min)
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen_rect.size(), egui::Sense::hover());
            ui.painter_at(rect)
                .rect_filled(rect, 0.0, Color32::from_black_alpha(150));
        });
}

pub(super) fn render_upload_window(ctx: &Context, flow: &mut SshUploadFlow) -> Vec<UploadUiAction> {
    let mut actions = Vec::new();

    egui::Window::new(format!("Upload to {}", flow.host_label))
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .default_width(560.0)
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(theme::PANEL_BG)
                .stroke(Stroke::new(1.0, theme::BORDER_STRONG)),
        )
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(10.0, 10.0);
            ui.label(
                RichText::new(file_summary(&flow.files))
                    .size(12.0)
                    .color(theme::FG_SOFT),
            );
            ui.add_space(4.0);

            match &flow.mode {
                UploadMode::Preparing => {
                    loading_spinner::show_with_detail(
                        ui,
                        Id::new("ssh_upload_prepare"),
                        "Checking upload options…",
                        "Detecting Taildrop and probing a remote destination",
                    );
                }
                UploadMode::Ready => render_ready_state(ui, flow, &mut actions),
                UploadMode::Uploading => render_uploading_state(ui, flow, &mut actions),
                UploadMode::Finished(outcome) => render_finished_state(ui, outcome, &mut actions),
                UploadMode::Failed(error) => render_failed_state(ui, error, flow.files.is_empty(), &mut actions),
            }
        });

    actions
}

fn render_ready_state(ui: &mut egui::Ui, flow: &mut SshUploadFlow, actions: &mut Vec<UploadUiAction>) {
    render_transport_choice(ui, flow);
    if let Some(error) = &flow.ssh_upload_error {
        ui.label(RichText::new(error).size(11.0).color(theme::PALETTE_RED));
    }

    if flow.transport_choice == UploadTransportChoice::Ssh {
        render_destination_editor(ui, flow);
    } else if let Some(target) = &flow.taildrop_target {
        ui.label(
            RichText::new(format!("Taildrop target: {target}"))
                .size(12.0)
                .color(theme::FG),
        );
        ui.label(
            RichText::new("Taildrop delivers files to the device inbox; no destination directory is selected here.")
                .size(11.0)
                .color(theme::FG_DIM),
        );
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Cancel").clicked() {
            actions.push(UploadUiAction::Close);
        }

        let start_enabled = match flow.transport_choice {
            UploadTransportChoice::Ssh => flow.ssh_upload_error.is_none() && !flow.destination_input.trim().is_empty(),
            UploadTransportChoice::Taildrop => true,
        };
        let start = ui.add_enabled(start_enabled, egui::Button::new("Start Upload"));
        if start.clicked() {
            actions.push(UploadUiAction::StartUpload);
        }
    });
}

fn render_uploading_state(ui: &mut egui::Ui, flow: &mut SshUploadFlow, actions: &mut Vec<UploadUiAction>) {
    if let Some(snapshot) = &flow.upload_snapshot {
        render_upload_progress(ui, snapshot);
    } else {
        loading_spinner::show(ui, Id::new("ssh_upload_start"), Some("Starting upload…"));
    }

    ui.add_space(8.0);
    if ui.button("Cancel Upload").clicked() {
        actions.push(UploadUiAction::CancelUpload);
    }
}

fn render_finished_state(ui: &mut egui::Ui, outcome: &super::UploadOutcome, actions: &mut Vec<UploadUiAction>) {
    let title = if outcome.cancelled {
        "Upload cancelled"
    } else {
        "Upload complete"
    };
    ui.label(RichText::new(title).size(14.0).strong().color(theme::FG));
    ui.label(RichText::new(&outcome.detail).size(12.0).color(theme::FG_SOFT));
    render_bytes_summary(ui, outcome.completed_bytes, outcome.total_bytes);
    ui.add_space(8.0);
    if ui.button("Close").clicked() {
        actions.push(UploadUiAction::Close);
    }
}

fn render_failed_state(ui: &mut egui::Ui, error: &str, no_files: bool, actions: &mut Vec<UploadUiAction>) {
    ui.label(
        RichText::new("Upload failed")
            .size(14.0)
            .strong()
            .color(theme::PALETTE_RED),
    );
    ui.label(RichText::new(error).size(12.0).color(theme::FG_SOFT));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Close").clicked() {
            actions.push(UploadUiAction::Close);
        }
        if !no_files && ui.button("Back").clicked() {
            actions.push(UploadUiAction::BackToReady);
        }
    });
}

fn render_transport_choice(ui: &mut egui::Ui, flow: &mut SshUploadFlow) {
    ui.label(RichText::new("Transfer method").size(12.0).color(theme::FG_SOFT));
    ui.horizontal(|ui| {
        let ssh_button = ui
            .add_enabled_ui(flow.ssh_upload_error.is_none(), |ui| {
                ui.selectable_label(flow.transport_choice == UploadTransportChoice::Ssh, "SSH upload")
            })
            .inner;
        if ssh_button.clicked() {
            flow.transport_choice = UploadTransportChoice::Ssh;
        }

        if let Some(target) = &flow.taildrop_target {
            let label = format!("Taildrop ({target})");
            let taildrop_button = ui.selectable_label(flow.transport_choice == UploadTransportChoice::Taildrop, label);
            if taildrop_button.clicked() {
                flow.transport_choice = UploadTransportChoice::Taildrop;
            }
        }
    });
}

fn render_destination_editor(ui: &mut egui::Ui, flow: &mut SshUploadFlow) {
    ui.label(RichText::new("Remote destination").size(12.0).color(theme::FG_SOFT));
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut flow.destination_input)
                .desired_width(360.0)
                .hint_text("~/uploads"),
        );
        let browse_label = if flow.browser.open { "Hide Browser" } else { "Browse" };
        if ui.button(browse_label).clicked() {
            flow.browser.open = !flow.browser.open;
            if flow.browser.open {
                request_directory_listing(flow, flow.destination_input.clone());
            }
        }
        if ui.button("Refresh").clicked() {
            request_directory_listing(flow, flow.destination_input.clone());
        }
    });

    if flow.browser.open {
        ui.add_space(4.0);
        egui::Frame::default()
            .fill(theme::BG_ELEVATED)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                if flow.browser.loading {
                    loading_spinner::show(ui, Id::new("ssh_upload_browser"), Some("Listing remote directories…"));
                    return;
                }

                if let Some(error) = &flow.browser.error {
                    ui.label(RichText::new(error).size(11.0).color(theme::PALETTE_RED));
                } else if !flow.browser.current_dir.is_empty() {
                    ui.label(
                        RichText::new(format!("Browsing {}", flow.browser.current_dir))
                            .size(11.0)
                            .color(theme::FG_DIM),
                    );
                }

                if !flow.browser.current_dir.is_empty() && ui.button("Use This Folder").clicked() {
                    flow.destination_input.clone_from(&flow.browser.current_dir);
                }

                let mut navigate_to = None;
                ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    for entry in &flow.browser.entries {
                        if ui.button(entry).clicked() {
                            navigate_to = Some(join_remote_browser_path(&flow.browser.current_dir, entry));
                        }
                    }
                });
                if let Some(next_path) = navigate_to {
                    request_directory_listing(flow, next_path);
                }
            });
    }
}

fn render_upload_progress(ui: &mut egui::Ui, snapshot: &super::UploadSnapshot) {
    ui.label(RichText::new("Upload in progress").size(14.0).strong().color(theme::FG));
    ui.label(RichText::new(&snapshot.detail).size(12.0).color(theme::FG_SOFT));
    if let Some(current_file) = &snapshot.current_file_name {
        ui.label(RichText::new(current_file).size(12.0).color(theme::FG));
    }

    let progress = progress_fraction(snapshot.completed_bytes, snapshot.total_bytes);
    ui.add(
        egui::ProgressBar::new(progress.clamp(0.0, 1.0))
            .show_percentage()
            .desired_width(420.0),
    );
    ui.label(
        RichText::new(format!("{} / {} files", snapshot.completed_files, snapshot.total_files))
            .size(11.0)
            .color(theme::FG_DIM),
    );
    render_bytes_summary(ui, snapshot.completed_bytes, snapshot.total_bytes);
}

fn render_bytes_summary(ui: &mut egui::Ui, completed_bytes: u64, total_bytes: u64) {
    ui.label(
        RichText::new(format!(
            "{} / {}",
            human_bytes(completed_bytes),
            human_bytes(total_bytes),
        ))
        .size(11.0)
        .color(theme::FG_DIM),
    );
}
