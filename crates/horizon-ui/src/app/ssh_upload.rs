mod render;
mod worker;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{Context, ViewportId};
use horizon_core::{PanelId, PanelKind, SshConnection};

use self::render::{render_backdrop, render_upload_window};
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
    target_viewport_id: ViewportId,
    host_label: String,
    connection: SshConnection,
    files: Vec<LocalUploadFile>,
    destination_input: String,
    ssh_upload_error: Option<String>,
    taildrop_target: Option<String>,
    transport_choice: UploadTransportChoice,
    mode: UploadMode,
    browser: RemoteDirectoryBrowser,
    preparation_rx: Option<Receiver<Result<PreparationResult, String>>>,
    upload_handle: Option<UploadWorkerHandle>,
    upload_snapshot: Option<UploadSnapshot>,
    upload_started_at: Option<Instant>,
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
        target_viewport_id: ViewportId,
        connection: SshConnection,
        host_label: String,
        files: Vec<LocalUploadFile>,
        last_destination: Option<String>,
    ) -> Self {
        Self {
            target_viewport_id,
            host_label,
            preparation_rx: Some(spawn_preparation(connection.clone(), last_destination)),
            connection,
            files,
            destination_input: String::new(),
            ssh_upload_error: None,
            taildrop_target: None,
            transport_choice: UploadTransportChoice::Ssh,
            mode: UploadMode::Preparing,
            browser: RemoteDirectoryBrowser::default(),
            upload_handle: None,
            upload_snapshot: None,
            upload_started_at: None,
        }
    }
}

impl HorizonApp {
    pub(super) fn maybe_start_ssh_file_drop(
        &mut self,
        panel_id: PanelId,
        dropped: &[egui::DroppedFile],
        viewport_id: ViewportId,
    ) -> bool {
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
                    target_viewport_id: viewport_id,
                    host_label,
                    connection: connection.unwrap_or_default(),
                    files: Vec::new(),
                    destination_input: String::new(),
                    ssh_upload_error: None,
                    taildrop_target: None,
                    transport_choice: UploadTransportChoice::Ssh,
                    mode: UploadMode::Failed(error),
                    browser: RemoteDirectoryBrowser::default(),
                    preparation_rx: None,
                    upload_handle: None,
                    upload_snapshot: None,
                    upload_started_at: None,
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
        self.ssh_upload_flow = Some(SshUploadFlow::new(
            viewport_id,
            connection,
            host_label,
            files,
            last_destination,
        ));
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
                    flow.upload_started_at = None;
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
        if flow.target_viewport_id != ctx.viewport_id() {
            return;
        }
        if flow.mode.is_uploading() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

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
                flow.upload_started_at = None;
            }
            if actions.contains(&UploadUiAction::StartUpload) {
                start_flow_upload(flow);
            }
        }
    }
}

fn apply_preparation_result(flow: &mut SshUploadFlow, result: PreparationResult) {
    flow.destination_input = result.suggested_destination.unwrap_or_default();
    flow.ssh_upload_error = result.ssh_upload_error;
    flow.taildrop_target = result.taildrop_target;
    if flow.destination_input.is_empty() && flow.taildrop_target.is_none() {
        flow.mode = UploadMode::Failed(
            flow.ssh_upload_error
                .clone()
                .unwrap_or_else(|| "SSH upload is unavailable for this session".to_string()),
        );
        return;
    }
    flow.transport_choice = if flow.taildrop_target.is_some() {
        UploadTransportChoice::Taildrop
    } else {
        UploadTransportChoice::Ssh
    };
    flow.mode = UploadMode::Ready;
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
    flow.upload_started_at = Some(Instant::now());
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
    u16::try_from(per_mille.min(1000)).map_or(1.0, |value| f32::from(value) / 1000.0)
}

fn transfer_speed_bytes_per_second(completed_bytes: u64, started_at: Instant, now: Instant) -> Option<u64> {
    if completed_bytes == 0 {
        return None;
    }

    let elapsed_millis =
        u64::try_from(now.saturating_duration_since(started_at).as_millis()).map_or(u64::MAX, |value| value);
    if elapsed_millis == 0 {
        return None;
    }

    let bytes_per_second = completed_bytes
        .saturating_mul(1000)
        .checked_div(elapsed_millis)
        .unwrap_or(0);
    (bytes_per_second > 0).then_some(bytes_per_second)
}

fn estimated_remaining_duration(completed_bytes: u64, total_bytes: u64, bytes_per_second: u64) -> Option<Duration> {
    if bytes_per_second == 0 || completed_bytes >= total_bytes {
        return None;
    }

    let remaining_bytes = total_bytes.saturating_sub(completed_bytes);
    let eta_seconds = remaining_bytes
        .saturating_add(bytes_per_second.saturating_sub(1))
        .checked_div(bytes_per_second)
        .unwrap_or(0);
    Some(Duration::from_secs(eta_seconds))
}

fn human_transfer_rate(bytes_per_second: u64) -> String {
    format!("{}/s", human_bytes(bytes_per_second))
}

fn human_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    if total_seconds < 60 {
        return format!("{total_seconds}s");
    }

    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    let mut parts = Vec::with_capacity(3);
    if hours > 0 {
        parts.push(format!("{hours} {}", if hours == 1 { "hour" } else { "hours" }));
    }
    if minutes > 0 {
        parts.push(format!("{minutes} {}", if minutes == 1 { "minute" } else { "minutes" }));
    }
    if seconds > 0 || parts.is_empty() {
        parts.push(format!("{seconds} {}", if seconds == 1 { "second" } else { "seconds" }));
    }

    match parts.len() {
        0 => "0s".to_string(),
        1 => parts.remove(0),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => format!("{}, {} and {}", parts[0], parts[1], parts[2]),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        estimated_remaining_duration, file_summary, human_bytes, human_duration, human_transfer_rate,
        join_remote_browser_path, parent_remote_path, progress_fraction, transfer_speed_bytes_per_second,
    };
    use crate::app::ssh_upload::worker::LocalUploadFile;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

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
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(2048), "2.0 KB");
    }

    #[test]
    fn progress_fraction_clamps_and_handles_zero_total() {
        assert!((progress_fraction(0, 0) - 0.0).abs() <= f32::EPSILON);
        assert!((progress_fraction(512, 1024) - 0.5).abs() <= f32::EPSILON);
        assert!((progress_fraction(4096, 1024) - 1.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn transfer_speed_bytes_per_second_uses_elapsed_time() {
        let start = Instant::now();

        assert_eq!(
            transfer_speed_bytes_per_second(2048, start, start + Duration::from_secs(2)),
            Some(1024)
        );
    }

    #[test]
    fn estimated_remaining_duration_rounds_up_to_next_second() {
        assert_eq!(
            estimated_remaining_duration(1024, 4096, 1500),
            Some(Duration::from_secs(3))
        );
    }

    #[test]
    fn human_transfer_rate_formats_per_second_suffix() {
        assert_eq!(human_transfer_rate(1536), "1.5 KB/s");
    }

    #[test]
    fn human_duration_formats_short_and_long_values() {
        assert_eq!(human_duration(Duration::from_secs(53)), "53s");
        assert_eq!(human_duration(Duration::from_secs(123)), "2 minutes and 3 seconds");
        assert_eq!(
            human_duration(Duration::from_secs(3723)),
            "1 hour, 2 minutes and 3 seconds"
        );
    }

    fn test_file(name: &str) -> LocalUploadFile {
        LocalUploadFile {
            path: PathBuf::from(name),
            name: name.to_string(),
            size_bytes: 1,
        }
    }
}
