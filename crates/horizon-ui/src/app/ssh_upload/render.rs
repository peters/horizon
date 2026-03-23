use egui::{
    Align, Align2, Color32, Context, CornerRadius, Id, Layout, Margin, Rect, RichText, Stroke, StrokeKind, Vec2,
};

use crate::{loading_spinner, theme};

use super::{
    SshUploadFlow, UploadMode, UploadTransportChoice, UploadUiAction, estimated_remaining_duration, file_summary,
    human_bytes, human_duration, human_transfer_rate, progress_fraction, transfer_speed_bytes_per_second,
};

// Layout
const MODAL_WIDTH: f32 = 520.0;
const SECTION_SPACING: f32 = 16.0;
const INNER_SPACING: f32 = 8.0;
const PROGRESS_BAR_HEIGHT: f32 = 6.0;

// Colors derived from the theme palette.
const HEADER_BG: Color32 = Color32::from_rgb(10, 13, 20);
const SURFACE_TINT: Color32 = Color32::from_rgb(24, 30, 42);
const SURFACE_BORDER: Color32 = Color32::from_rgb(42, 52, 68);
const SEGMENT_BG: Color32 = Color32::from_rgb(18, 22, 32);
const SEGMENT_ACTIVE_BG: Color32 = Color32::from_rgb(30, 42, 68);
const BTN_PRIMARY_BG: Color32 = Color32::from_rgb(56, 112, 210);

pub(super) fn render_backdrop(ctx: &Context) {
    let screen_rect = ctx.input(egui::InputState::viewport_rect);
    egui::Area::new(Id::new("ssh_upload_backdrop"))
        .fixed_pos(screen_rect.min)
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen_rect.size(), egui::Sense::hover());
            ui.painter_at(rect)
                .rect_filled(rect, 0.0, Color32::from_black_alpha(170));
        });
}

pub(super) fn render_upload_window(ctx: &Context, flow: &mut SshUploadFlow) -> Vec<UploadUiAction> {
    let mut actions = Vec::new();

    egui::Window::new("ssh_upload_modal")
        .id(Id::new("ssh_upload_modal"))
        .title_bar(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Debug)
        .fixed_size(Vec2::new(MODAL_WIDTH, 0.0))
        .frame(
            egui::Frame::NONE
                .fill(theme::PANEL_BG)
                .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
                .corner_radius(CornerRadius::same(14))
                .shadow(egui::Shadow {
                    offset: [0, 12],
                    blur: 40,
                    spread: 4,
                    color: Color32::from_black_alpha(140),
                }),
        )
        .show(ctx, |ui| {
            render_header(ui, &flow.host_label, &flow.files);
            ui.add_space(SECTION_SPACING);

            egui::Frame::NONE.inner_margin(Margin::symmetric(24, 0)).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);

                match &flow.mode {
                    UploadMode::Preparing => {
                        ui.add_space(8.0);
                        loading_spinner::show_with_detail(
                            ui,
                            Id::new("ssh_upload_prepare"),
                            "Preparing upload...",
                            "Probing remote and detecting transport options",
                        );
                        ui.add_space(8.0);
                    }
                    UploadMode::Ready => render_ready_state(ui, flow, &mut actions),
                    UploadMode::Uploading => render_uploading_state(ui, flow, &mut actions),
                    UploadMode::Finished(outcome) => {
                        render_finished_state(ui, outcome, &mut actions);
                    }
                    UploadMode::Failed(error) => {
                        render_failed_state(ui, error, flow.files.is_empty(), &mut actions);
                    }
                }
            });

            ui.add_space(20.0);
        });

    actions
}

// ---------------------------------------------------------------------------
// Header with icon, host label, and file summary
// ---------------------------------------------------------------------------

fn render_header(ui: &mut egui::Ui, host_label: &str, files: &[super::worker::LocalUploadFile]) {
    egui::Frame::NONE
        .fill(HEADER_BG)
        .corner_radius(CornerRadius {
            nw: 14,
            ne: 14,
            sw: 0,
            se: 0,
        })
        .inner_margin(Margin::symmetric(24, 18))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let icon_size = 32.0;
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(icon_size), egui::Sense::hover());
                paint_upload_icon(ui, icon_rect, theme::ACCENT);

                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(
                        RichText::new(format!("Upload to {host_label}"))
                            .size(15.0)
                            .strong()
                            .color(theme::FG),
                    );
                    ui.label(RichText::new(file_summary(files)).size(12.0).color(theme::FG_DIM));
                });
            });
        });
}

fn paint_upload_icon(ui: &egui::Ui, rect: Rect, color: Color32) {
    let painter = ui.painter_at(rect);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let half = rect.width() * 0.36;

    painter.circle_filled(rect.center(), rect.width() * 0.48, theme::alpha(color, 22));

    // Arrow shaft
    let shaft_top = cy - half * 0.65;
    let shaft_bottom = cy + half * 0.55;
    painter.line_segment(
        [egui::pos2(cx, shaft_top), egui::pos2(cx, shaft_bottom)],
        Stroke::new(2.0, color),
    );

    // Arrow head chevron
    let head_spread = half * 0.5;
    let head_drop = half * 0.4;
    painter.line_segment(
        [
            egui::pos2(cx - head_spread, shaft_top + head_drop),
            egui::pos2(cx, shaft_top),
        ],
        Stroke::new(2.0, color),
    );
    painter.line_segment(
        [
            egui::pos2(cx + head_spread, shaft_top + head_drop),
            egui::pos2(cx, shaft_top),
        ],
        Stroke::new(2.0, color),
    );

    // Base tray
    let tray_y = shaft_bottom + 2.0;
    painter.line_segment(
        [egui::pos2(cx - half * 0.6, tray_y), egui::pos2(cx + half * 0.6, tray_y)],
        Stroke::new(2.0, theme::alpha(color, 120)),
    );
}

// ---------------------------------------------------------------------------
// Ready state
// ---------------------------------------------------------------------------

fn render_ready_state(ui: &mut egui::Ui, flow: &mut SshUploadFlow, actions: &mut Vec<UploadUiAction>) {
    render_file_pills(ui, &flow.files);
    ui.add_space(INNER_SPACING);

    render_transport_choice(ui, flow);

    if let Some(error) = &flow.ssh_upload_error {
        ui.add_space(4.0);
        ui.label(RichText::new(error).size(11.0).color(theme::PALETTE_RED));
    }

    ui.add_space(INNER_SPACING);

    if flow.transport_choice == UploadTransportChoice::Ssh {
        render_destination_editor(ui, flow, actions);
    } else if let Some(target) = &flow.taildrop_target {
        render_taildrop_info(ui, target);
    }

    ui.add_space(SECTION_SPACING);
    render_ready_action_buttons(ui, actions, flow);
}

fn render_file_pills(ui: &mut egui::Ui, files: &[super::worker::LocalUploadFile]) {
    if files.is_empty() {
        return;
    }

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        let visible_count = files.len().min(6);
        for file in files.iter().take(visible_count) {
            render_single_file_pill(ui, &file.name, file.size_bytes);
        }
        if files.len() > visible_count {
            let remaining = files.len() - visible_count;
            egui::Frame::NONE
                .fill(SURFACE_TINT)
                .stroke(Stroke::new(1.0, SURFACE_BORDER))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(10, 4))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!("+{remaining} more"))
                            .size(11.0)
                            .color(theme::FG_DIM),
                    );
                });
        }
    });
}

fn render_single_file_pill(ui: &mut egui::Ui, name: &str, size_bytes: u64) {
    egui::Frame::NONE
        .fill(SURFACE_TINT)
        .stroke(Stroke::new(1.0, SURFACE_BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (dot_rect, _) = ui.allocate_exact_size(Vec2::new(6.0, 14.0), egui::Sense::hover());
                ui.painter_at(dot_rect)
                    .circle_filled(dot_rect.center(), 3.0, theme::PALETTE_CYAN);
                let display_name = truncate_name(name, 20);
                ui.label(RichText::new(display_name).size(11.0).color(theme::FG_SOFT));
                ui.label(RichText::new(human_bytes(size_bytes)).size(10.0).color(theme::FG_DIM));
            });
        });
}

fn truncate_name(name: &str, max_chars: usize) -> String {
    let char_count = name.chars().count();
    if char_count > max_chars {
        let truncated: String = name.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{truncated}...")
    } else {
        name.to_string()
    }
}

// ---------------------------------------------------------------------------
// Segmented transport control
// ---------------------------------------------------------------------------

fn render_transport_choice(ui: &mut egui::Ui, flow: &mut SshUploadFlow) {
    ui.label(RichText::new("Transfer method").size(11.0).color(theme::FG_DIM));

    egui::Frame::NONE
        .fill(SEGMENT_BG)
        .stroke(Stroke::new(1.0, SURFACE_BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let ssh_enabled = flow.ssh_upload_error.is_none();

                let ssh_active = flow.transport_choice == UploadTransportChoice::Ssh;
                if render_segment_button(ui, "SSH", ssh_active, ssh_enabled) {
                    flow.transport_choice = UploadTransportChoice::Ssh;
                }

                if let Some(target) = &flow.taildrop_target {
                    let label = format!("Taildrop ({target})");
                    let td_active = flow.transport_choice == UploadTransportChoice::Taildrop;
                    if render_segment_button(ui, &label, td_active, true) {
                        flow.transport_choice = UploadTransportChoice::Taildrop;
                    }
                }
            });
        });
}

fn render_segment_button(ui: &mut egui::Ui, label: &str, active: bool, enabled: bool) -> bool {
    let fill = if active {
        SEGMENT_ACTIVE_BG
    } else {
        Color32::TRANSPARENT
    };
    let text_color = if !enabled {
        theme::alpha(theme::FG_DIM, 100)
    } else if active {
        theme::FG
    } else {
        theme::FG_DIM
    };
    let stroke = if active {
        Stroke::new(1.0, theme::alpha(theme::ACCENT, 80))
    } else {
        Stroke::NONE
    };

    let resp = egui::Frame::NONE
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(14, 6))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(12.0).color(text_color));
        })
        .response;

    let interact = resp.interact(egui::Sense::click());
    if enabled && interact.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && interact.clicked()
}

// ---------------------------------------------------------------------------
// Destination input & browser
// ---------------------------------------------------------------------------

fn render_destination_editor(ui: &mut egui::Ui, flow: &mut SshUploadFlow, actions: &mut Vec<UploadUiAction>) {
    ui.label(RichText::new("Remote destination").size(11.0).color(theme::FG_DIM));

    egui::Frame::NONE
        .fill(theme::BG_ELEVATED)
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (folder_rect, _) = ui.allocate_exact_size(Vec2::new(16.0, 16.0), egui::Sense::hover());
                paint_folder_icon(&ui.painter_at(folder_rect), folder_rect);

                ui.add_space(4.0);
                ui.add(
                    egui::TextEdit::singleline(&mut flow.destination_input)
                        .desired_width(ui.available_width() - 88.0)
                        .hint_text("~/uploads")
                        .frame(false)
                        .text_color(theme::FG),
                );
                if styled_small_button(ui, "Browse") {
                    actions.push(UploadUiAction::OpenDestinationPicker);
                }
            });
        });
}

fn styled_small_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).size(11.0).color(theme::FG_SOFT))
            .fill(theme::alpha(theme::ACCENT, 16))
            .stroke(Stroke::new(1.0, theme::alpha(theme::ACCENT, 40)))
            .corner_radius(CornerRadius::same(6)),
    )
    .clicked()
}

pub(super) fn paint_folder_icon(painter: &egui::Painter, rect: Rect) {
    let color = theme::PALETTE_YELLOW;
    let body = Rect::from_min_max(
        egui::pos2(rect.min.x, rect.min.y + 4.0),
        egui::pos2(rect.max.x, rect.max.y),
    );
    painter.rect_filled(body, 2.0, theme::alpha(color, 50));
    painter.rect_stroke(
        body,
        2.0,
        Stroke::new(1.0, theme::alpha(color, 120)),
        StrokeKind::Inside,
    );

    let tab = Rect::from_min_max(
        egui::pos2(rect.min.x, rect.min.y + 1.0),
        egui::pos2(rect.min.x + rect.width() * 0.45, rect.min.y + 5.0),
    );
    painter.rect_filled(
        tab,
        CornerRadius {
            nw: 2,
            ne: 2,
            sw: 0,
            se: 0,
        },
        theme::alpha(color, 80),
    );
}

// ---------------------------------------------------------------------------
// Taildrop info
// ---------------------------------------------------------------------------

fn render_taildrop_info(ui: &mut egui::Ui, target: &str) {
    egui::Frame::NONE
        .fill(theme::alpha(theme::PALETTE_CYAN, 10))
        .stroke(Stroke::new(1.0, theme::alpha(theme::PALETTE_CYAN, 30)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!("Taildrop target: {target}"))
                    .size(12.0)
                    .color(theme::PALETTE_CYAN),
            );
            ui.label(
                RichText::new(
                    "Files will be delivered to the device inbox. \
                     No destination directory needed.",
                )
                .size(11.0)
                .color(theme::FG_DIM),
            );
        });
}

// ---------------------------------------------------------------------------
// Action buttons (ready state)
// ---------------------------------------------------------------------------

fn render_ready_action_buttons(ui: &mut egui::Ui, actions: &mut Vec<UploadUiAction>, flow: &SshUploadFlow) {
    paint_separator(ui);
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        if ghost_button(ui, "Cancel") {
            actions.push(UploadUiAction::Close);
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let start_enabled = match flow.transport_choice {
                UploadTransportChoice::Ssh => {
                    flow.ssh_upload_error.is_none() && !flow.destination_input.trim().is_empty()
                }
                UploadTransportChoice::Taildrop => true,
            };

            if primary_button(ui, "Start Upload", start_enabled) {
                actions.push(UploadUiAction::StartUpload);
            }
        });
    });
}

fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    let fill = if enabled {
        BTN_PRIMARY_BG
    } else {
        theme::alpha(BTN_PRIMARY_BG, 60)
    };
    let text_color = if enabled {
        Color32::WHITE
    } else {
        theme::alpha(Color32::WHITE, 100)
    };

    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(label).size(13.0).strong().color(text_color))
            .fill(fill)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(10))
            .min_size(Vec2::new(130.0, 34.0)),
    )
    .clicked()
}

fn ghost_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).size(12.0).color(theme::FG_DIM))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
            .corner_radius(CornerRadius::same(10))
            .min_size(Vec2::new(80.0, 34.0)),
    )
    .clicked()
}

fn paint_separator(ui: &mut egui::Ui) {
    let (sep_rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter_at(sep_rect).rect_filled(sep_rect, 0.0, theme::BORDER_SUBTLE);
}

// ---------------------------------------------------------------------------
// Uploading state
// ---------------------------------------------------------------------------

fn render_uploading_state(ui: &mut egui::Ui, flow: &mut SshUploadFlow, actions: &mut Vec<UploadUiAction>) {
    if let Some(snapshot) = &flow.upload_snapshot {
        render_upload_progress(ui, snapshot, flow.upload_started_at);
    } else {
        ui.add_space(8.0);
        loading_spinner::show(ui, Id::new("ssh_upload_start"), Some("Starting upload..."));
        ui.add_space(8.0);
    }

    ui.add_space(SECTION_SPACING);
    paint_separator(ui);
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        if ghost_button(ui, "Cancel Upload") {
            actions.push(UploadUiAction::CancelUpload);
        }
    });
}

fn render_upload_progress(
    ui: &mut egui::Ui,
    snapshot: &super::UploadSnapshot,
    upload_started_at: Option<std::time::Instant>,
) {
    ui.label(RichText::new("Uploading...").size(14.0).strong().color(theme::FG));
    ui.add_space(4.0);

    if let Some(current_file) = &snapshot.current_file_name {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (dot_rect, _) = ui.allocate_exact_size(Vec2::new(6.0, 14.0), egui::Sense::hover());
            ui.painter_at(dot_rect)
                .circle_filled(dot_rect.center(), 3.0, theme::ACCENT);
            ui.label(RichText::new(current_file).size(12.0).color(theme::FG_SOFT));
        });
        ui.add_space(6.0);
    }

    // Custom progress bar
    let frac = progress_fraction(snapshot.completed_bytes, snapshot.total_bytes).clamp(0.0, 1.0);
    let bar_width = ui.available_width();
    let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(bar_width, PROGRESS_BAR_HEIGHT), egui::Sense::hover());

    if ui.is_rect_visible(bar_rect) {
        let painter = ui.painter_at(bar_rect);
        painter.rect_filled(bar_rect, 3.0, SURFACE_TINT);
        if frac > 0.0 {
            let fill_rect = Rect::from_min_size(bar_rect.min, Vec2::new(bar_rect.width() * frac, bar_rect.height()));
            painter.rect_filled(fill_rect, 3.0, theme::ACCENT);
            if frac < 1.0 {
                let glow_center = egui::pos2(fill_rect.max.x, fill_rect.center().y);
                painter.circle_filled(glow_center, 4.0, theme::alpha(theme::ACCENT, 60));
            }
        }
    }

    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} / {} files", snapshot.completed_files, snapshot.total_files))
                .size(11.0)
                .color(theme::FG_DIM),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            render_bytes_summary(ui, snapshot.completed_bytes, snapshot.total_bytes);
        });
    });

    ui.add_space(2.0);
    render_transfer_timing(ui, snapshot, upload_started_at);
    ui.label(RichText::new(snapshot.detail.as_str()).size(11.0).color(theme::FG_DIM));
}

fn render_transfer_timing(
    ui: &mut egui::Ui,
    snapshot: &super::UploadSnapshot,
    upload_started_at: Option<std::time::Instant>,
) {
    let Some(started_at) = upload_started_at else {
        ui.label(
            RichText::new("Estimating transfer speed…")
                .size(11.0)
                .color(theme::FG_DIM),
        );
        return;
    };

    let now = std::time::Instant::now();
    let Some(bytes_per_second) = transfer_speed_bytes_per_second(snapshot.completed_bytes, started_at, now) else {
        ui.label(
            RichText::new("Estimating transfer speed…")
                .size(11.0)
                .color(theme::FG_DIM),
        );
        return;
    };

    let speed_label = human_transfer_rate(bytes_per_second);
    let eta_label = estimated_remaining_duration(snapshot.completed_bytes, snapshot.total_bytes, bytes_per_second)
        .map_or_else(
            || "ETA calculating…".to_string(),
            |duration| format!("ETA {}", human_duration(duration)),
        );

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(speed_label).size(11.0).color(theme::FG_DIM));
        ui.label(RichText::new("•").size(11.0).color(theme::FG_DIM));
        ui.label(RichText::new(eta_label).size(11.0).color(theme::FG_DIM));
    });
}

// ---------------------------------------------------------------------------
// Finished state
// ---------------------------------------------------------------------------

fn render_finished_state(ui: &mut egui::Ui, outcome: &super::UploadOutcome, actions: &mut Vec<UploadUiAction>) {
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        let icon_size = 28.0;
        let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(icon_size), egui::Sense::hover());

        if outcome.cancelled {
            paint_cancel_icon(ui, icon_rect, theme::PALETTE_YELLOW);
            ui.add_space(8.0);
            ui.label(RichText::new("Upload cancelled").size(15.0).strong().color(theme::FG));
        } else {
            paint_checkmark_icon(ui, icon_rect, theme::PALETTE_GREEN);
            ui.add_space(8.0);
            ui.label(RichText::new("Upload complete").size(15.0).strong().color(theme::FG));
        }
    });

    ui.add_space(8.0);
    ui.label(RichText::new(&outcome.detail).size(12.0).color(theme::FG_SOFT));
    render_bytes_summary(ui, outcome.completed_bytes, outcome.total_bytes);

    ui.add_space(SECTION_SPACING);
    paint_separator(ui);
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if primary_button(ui, "Done", true) {
                actions.push(UploadUiAction::Close);
            }
        });
    });
}

fn paint_checkmark_icon(ui: &egui::Ui, rect: Rect, color: Color32) {
    let painter = ui.painter_at(rect);
    painter.circle_filled(rect.center(), rect.width() * 0.48, theme::alpha(color, 22));

    let cx = rect.center().x;
    let cy = rect.center().y;
    let s = rect.width() * 0.22;

    painter.line_segment(
        [egui::pos2(cx - s, cy), egui::pos2(cx - s * 0.1, cy + s * 0.8)],
        Stroke::new(2.5, color),
    );
    painter.line_segment(
        [
            egui::pos2(cx - s * 0.1, cy + s * 0.8),
            egui::pos2(cx + s * 1.2, cy - s * 0.6),
        ],
        Stroke::new(2.5, color),
    );
}

fn paint_cancel_icon(ui: &egui::Ui, rect: Rect, color: Color32) {
    let painter = ui.painter_at(rect);
    painter.circle_filled(rect.center(), rect.width() * 0.48, theme::alpha(color, 22));

    let cx = rect.center().x;
    let cy = rect.center().y;
    let s = rect.width() * 0.2;

    painter.line_segment(
        [egui::pos2(cx - s, cy), egui::pos2(cx + s, cy)],
        Stroke::new(2.5, color),
    );
}

// ---------------------------------------------------------------------------
// Failed state
// ---------------------------------------------------------------------------

fn render_failed_state(ui: &mut egui::Ui, error: &str, no_files: bool, actions: &mut Vec<UploadUiAction>) {
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        let icon_size = 28.0;
        let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(icon_size), egui::Sense::hover());
        paint_error_icon(ui, icon_rect, theme::PALETTE_RED);

        ui.add_space(8.0);
        ui.label(
            RichText::new("Upload failed")
                .size(15.0)
                .strong()
                .color(theme::PALETTE_RED),
        );
    });

    ui.add_space(8.0);

    egui::Frame::NONE
        .fill(theme::alpha(theme::PALETTE_RED, 8))
        .stroke(Stroke::new(1.0, theme::alpha(theme::PALETTE_RED, 25)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.label(RichText::new(error).size(12.0).color(theme::FG_SOFT));
        });

    ui.add_space(SECTION_SPACING);
    paint_separator(ui);
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        if ghost_button(ui, "Close") {
            actions.push(UploadUiAction::Close);
        }
        if !no_files {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if primary_button(ui, "Try Again", true) {
                    actions.push(UploadUiAction::BackToReady);
                }
            });
        }
    });
}

fn paint_error_icon(ui: &egui::Ui, rect: Rect, color: Color32) {
    let painter = ui.painter_at(rect);
    painter.circle_filled(rect.center(), rect.width() * 0.48, theme::alpha(color, 22));

    let cx = rect.center().x;
    let cy = rect.center().y;
    let s = rect.width() * 0.18;

    painter.line_segment(
        [egui::pos2(cx - s, cy - s), egui::pos2(cx + s, cy + s)],
        Stroke::new(2.5, color),
    );
    painter.line_segment(
        [egui::pos2(cx + s, cy - s), egui::pos2(cx - s, cy + s)],
        Stroke::new(2.5, color),
    );
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

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
