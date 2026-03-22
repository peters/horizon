mod worker;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Align2, Color32, Context, Id, RichText, ScrollArea, Stroke, Vec2};
use horizon_core::{PanelId, PanelKind, SshConnection};

use crate::{loading_spinner, theme};

use self::worker::{
    LocalUploadFile, PreparationResult, RemoteDirectoryListing, UploadMessage, UploadOutcome, UploadSnapshot,
    UploadTransport, UploadWorkerHandle, build_local_upload_files, spawn_preparation, spawn_remote_directory_listing,
    start_upload,
};
use super::HorizonApp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UploadTransportChoice {
    Ssh,
    Taildrop,
}

#[derive(Debug)]
enum UploadMode {
    Preparing,
    Ready,
    Uploading,
    Finished(UploadOutcome),
    Failed(String),
}

impl UploadMode {
    fn is_uploading(&self) -> bool {
        matches!(self, Self::Uploading)
    }
}

#[derive(Default)]
struct RemoteDirectoryBrowser {
    open: bool,
    loading: bool,
    current_dir: String,
    entries: Vec<String>,
    error: Option<String>,
    listing_rx: Option<Receiver<Result<RemoteDirectoryListing, String>>>,
}

pub(super) struct SshUploadFlow {
    host_label: String,
    connection: SshConnection,
    files: Vec<LocalUploadFile>,
    destination_input: String,
    taildrop_target: Option<String>,
    transport_choice: UploadTransportChoice,
    mode: UploadMode,
    browser: RemoteDirectoryBrowser,
    preparation_rx: Option<Receiver<Result<PreparationResult, String>>>,
    upload_handle: Option<UploadWorkerHandle>,
    upload_snapshot: Option<UploadSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UploadUiAction {
    Close,
    BackToReady,
    StartUpload,
    CancelUpload,
}

impl SshUploadFlow {
    fn new(
        connection: SshConnection,
        host_label: String,
        files: Vec<LocalUploadFile>,
        last_destination: Option<String>,
    ) -> Self {
        Self {
            host_label,
            connection: connection.clone(),
            files,
            destination_input: String::new(),
            taildrop_target: None,
            transport_choice: UploadTransportChoice::Ssh,
            mode: UploadMode::Preparing,
            browser: RemoteDirectoryBrowser::default(),
            preparation_rx: Some(spawn_preparation(connection, last_destination)),
            upload_handle: None,
            upload_snapshot: None,
        }
    }
}

impl HorizonApp {
    pub(super) fn maybe_start_ssh_file_drop(&mut self, panel_id: PanelId, dropped: &[egui::DroppedFile]) -> bool {
        let Some((panel_kind, host_label, connection)) = self.board.panel(panel_id).map(|panel| {
            (
                panel.kind,
                panel.display_title().into_owned(),
                panel.ssh_connection.clone(),
            )
        }) else {
            return false;
        };
        if panel_kind != PanelKind::Ssh {
            return false;
        }
        if self
            .ssh_upload_flow
            .as_ref()
            .is_some_and(|flow| flow.mode.is_uploading())
        {
            return true;
        }

        let paths: Vec<PathBuf> = dropped.iter().filter_map(|file| file.path.clone()).collect();
        let files = match build_local_upload_files(paths) {
            Ok(files) => files,
            Err(error) => {
                self.ssh_upload_flow = Some(SshUploadFlow {
                    host_label,
                    connection: connection.unwrap_or_default(),
                    files: Vec::new(),
                    destination_input: String::new(),
                    taildrop_target: None,
                    transport_choice: UploadTransportChoice::Ssh,
                    mode: UploadMode::Failed(error),
                    browser: RemoteDirectoryBrowser::default(),
                    preparation_rx: None,
                    upload_handle: None,
                    upload_snapshot: None,
                });
                return true;
            }
        };

        let last_destination = connection.as_ref().and_then(|connection| {
            self.ssh_upload_destinations
                .get(&upload_target_key(connection))
                .cloned()
        });

        let Some(connection) = connection else {
            return false;
        };

        self.board.focus(panel_id);
        self.ssh_upload_flow = Some(SshUploadFlow::new(connection, host_label, files, last_destination));
        true
    }

    pub(super) fn poll_ssh_upload_flow(&mut self) {
        let mut remembered_destination = None;

        if let Some(flow) = self.ssh_upload_flow.as_mut() {
            if let Some(rx) = flow.preparation_rx.take() {
                match rx.try_recv() {
                    Ok(Ok(result)) => {
                        apply_preparation_result(flow, result);
                    }
                    Ok(Err(error)) => {
                        flow.mode = UploadMode::Failed(error);
                    }
                    Err(TryRecvError::Empty) => {
                        flow.preparation_rx = Some(rx);
                    }
                    Err(TryRecvError::Disconnected) => {
                        flow.mode = UploadMode::Failed("upload preparation worker disconnected".to_string());
                    }
                }
            }

            if let Some(rx) = flow.browser.listing_rx.take() {
                match rx.try_recv() {
                    Ok(Ok(listing)) => {
                        flow.browser.loading = false;
                        flow.browser.error = None;
                        flow.browser.current_dir = listing.current_dir.clone();
                        flow.browser.entries = listing.entries;
                        flow.destination_input = listing.current_dir;
                    }
                    Ok(Err(error)) => {
                        flow.browser.loading = false;
                        flow.browser.error = Some(error);
                    }
                    Err(TryRecvError::Empty) => {
                        flow.browser.listing_rx = Some(rx);
                    }
                    Err(TryRecvError::Disconnected) => {
                        flow.browser.loading = false;
                        flow.browser.error = Some("remote directory browser disconnected".to_string());
                    }
                }
            }

            if let Some(handle) = flow.upload_handle.take() {
                let mut finished_result = None;
                while let Ok(message) = handle.progress_rx.try_recv() {
                    match message {
                        UploadMessage::Snapshot(snapshot) => {
                            flow.upload_snapshot = Some(snapshot);
                        }
                        UploadMessage::Finished(result) => {
                            finished_result = Some(result);
                        }
                    }
                }

                if let Some(result) = finished_result {
                    match result {
                        Ok(outcome) => {
                            if !outcome.cancelled && matches!(flow.transport_choice, UploadTransportChoice::Ssh) {
                                remembered_destination =
                                    Some((upload_target_key(&flow.connection), flow.destination_input.clone()));
                            }
                            flow.mode = UploadMode::Finished(outcome);
                        }
                        Err(error) => {
                            flow.mode = UploadMode::Failed(error);
                        }
                    }
                } else {
                    flow.upload_handle = Some(handle);
                }
            }
        }

        if let Some((key, path)) = remembered_destination {
            self.ssh_upload_destinations.insert(key, path);
        }
    }

    pub(super) fn render_ssh_upload_flow(&mut self, ctx: &Context) {
        let Some(flow) = self.ssh_upload_flow.as_mut() else {
            return;
        };

        render_backdrop(ctx);
        let actions = render_upload_window(ctx, flow);

        if actions.contains(&UploadUiAction::Close) {
            self.ssh_upload_flow = None;
            return;
        }
        if let Some(flow) = self.ssh_upload_flow.as_mut() {
            if actions.contains(&UploadUiAction::CancelUpload)
                && let Some(handle) = &flow.upload_handle
            {
                handle.cancel();
            }
            if actions.contains(&UploadUiAction::BackToReady) {
                flow.mode = UploadMode::Ready;
                flow.upload_snapshot = None;
                flow.upload_handle = None;
            }
            if actions.contains(&UploadUiAction::StartUpload) {
                start_flow_upload(flow);
            }
        }
    }
}

fn apply_preparation_result(flow: &mut SshUploadFlow, result: PreparationResult) {
    flow.destination_input = result.suggested_destination;
    flow.taildrop_target = result.taildrop_target;
    flow.transport_choice = if flow.taildrop_target.is_some() {
        UploadTransportChoice::Taildrop
    } else {
        UploadTransportChoice::Ssh
    };
    flow.mode = UploadMode::Ready;
}

fn render_backdrop(ctx: &Context) {
    let screen_rect = ctx.input(egui::InputState::viewport_rect);
    egui::Area::new(Id::new("ssh_upload_backdrop"))
        .fixed_pos(screen_rect.min)
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen_rect.size(), egui::Sense::click());
            ui.painter_at(rect)
                .rect_filled(rect, 0.0, Color32::from_black_alpha(150));
        });
}

fn render_upload_window(ctx: &Context, flow: &mut SshUploadFlow) -> Vec<UploadUiAction> {
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

        let start_enabled =
            flow.transport_choice == UploadTransportChoice::Taildrop || !flow.destination_input.trim().is_empty();
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

fn render_finished_state(ui: &mut egui::Ui, outcome: &UploadOutcome, actions: &mut Vec<UploadUiAction>) {
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
        if ui
            .selectable_label(flow.transport_choice == UploadTransportChoice::Ssh, "SSH upload")
            .clicked()
        {
            flow.transport_choice = UploadTransportChoice::Ssh;
        }
        if let Some(target) = &flow.taildrop_target {
            let label = format!("Taildrop ({target})");
            if ui
                .selectable_label(flow.transport_choice == UploadTransportChoice::Taildrop, label)
                .clicked()
            {
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

fn render_upload_progress(ui: &mut egui::Ui, snapshot: &UploadSnapshot) {
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

fn start_flow_upload(flow: &mut SshUploadFlow) {
    let transport = match flow.transport_choice {
        UploadTransportChoice::Ssh => UploadTransport::Ssh {
            destination_dir: flow.destination_input.trim().to_string(),
        },
        UploadTransportChoice::Taildrop => UploadTransport::Taildrop {
            target: flow.taildrop_target.clone().unwrap_or_default(),
        },
    };

    flow.upload_snapshot = Some(UploadSnapshot {
        completed_files: 0,
        total_files: flow.files.len(),
        completed_bytes: 0,
        total_bytes: flow.files.iter().map(|file| file.size_bytes).sum(),
        current_file_name: None,
        current_file_size: None,
        detail: "Starting upload…".to_string(),
    });
    flow.upload_handle = Some(start_upload(flow.connection.clone(), flow.files.clone(), transport));
    flow.mode = UploadMode::Uploading;
}

fn request_directory_listing(flow: &mut SshUploadFlow, requested_path: String) {
    flow.browser.loading = true;
    flow.browser.error = None;
    flow.browser.listing_rx = Some(spawn_remote_directory_listing(flow.connection.clone(), requested_path));
}

fn join_remote_browser_path(current_dir: &str, entry: &str) -> String {
    if entry == ".." {
        return parent_remote_path(current_dir);
    }
    if current_dir == "/" {
        return format!("/{entry}");
    }

    format!("{current_dir}/{entry}")
}

fn parent_remote_path(path: &str) -> String {
    if path == "/" {
        return "/".to_string();
    }

    match path.rsplit_once('/') {
        Some(("", _)) | None => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
    }
}

fn file_summary(files: &[LocalUploadFile]) -> String {
    if files.is_empty() {
        return "No files selected".to_string();
    }

    let names: Vec<&str> = files.iter().take(3).map(|file| file.name.as_str()).collect();
    if files.len() <= 3 {
        return format!("{} file(s): {}", files.len(), names.join(", "));
    }

    format!(
        "{} file(s): {}, and {} more",
        files.len(),
        names.join(", "),
        files.len() - names.len()
    )
}

fn upload_target_key(connection: &SshConnection) -> String {
    format!(
        "{}|{}|{}",
        connection.transport_target().to_ascii_lowercase(),
        connection.port.unwrap_or_default(),
        connection
            .proxy_jump
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase(),
    )
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

    let mut unit_index = 0;
    let mut divisor = 1_u64;
    while bytes / divisor >= 1024 && unit_index < UNITS.len() - 1 {
        divisor = divisor.saturating_mul(1024);
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{bytes} {}", UNITS[unit_index])
    } else {
        let whole = bytes / divisor;
        let decimal = bytes.saturating_sub(whole.saturating_mul(divisor)).saturating_mul(10) / divisor;
        format!("{whole}.{decimal} {}", UNITS[unit_index])
    }
}

fn progress_fraction(completed_bytes: u64, total_bytes: u64) -> f32 {
    if total_bytes == 0 {
        return 0.0;
    }

    let per_mille = completed_bytes
        .saturating_mul(1000)
        .checked_div(total_bytes)
        .unwrap_or(0);
    let per_mille = u16::try_from(per_mille).unwrap_or(1000);
    f32::from(per_mille) / 1000.0
}

#[cfg(test)]
mod tests {
    use super::{file_summary, human_bytes, join_remote_browser_path, parent_remote_path};
    use crate::app::ssh_upload::worker::LocalUploadFile;
    use std::path::PathBuf;

    #[test]
    fn parent_remote_path_preserves_root() {
        assert_eq!(parent_remote_path("/"), "/");
        assert_eq!(parent_remote_path("/srv/logs"), "/srv");
    }

    #[test]
    fn join_remote_browser_path_handles_root_and_parent() {
        assert_eq!(join_remote_browser_path("/", "logs"), "/logs");
        assert_eq!(join_remote_browser_path("/srv/logs", ".."), "/srv");
    }

    #[test]
    fn file_summary_compacts_long_lists() {
        let files = vec![
            test_file("a.txt"),
            test_file("b.txt"),
            test_file("c.txt"),
            test_file("d.txt"),
        ];

        assert_eq!(file_summary(&files), "4 file(s): a.txt, b.txt, c.txt, and 1 more");
    }

    #[test]
    fn human_bytes_formats_binary_units() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
    }

    fn test_file(name: &str) -> LocalUploadFile {
        LocalUploadFile {
            path: PathBuf::from(name),
            name: name.to_string(),
            size_bytes: 1,
        }
    }
}
