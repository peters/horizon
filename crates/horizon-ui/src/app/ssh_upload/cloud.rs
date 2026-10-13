//! Drops on cloud agent and shell panels: the files go to the panel's worker session, and
//! the panel gets their worker paths only after the upload. A cloud panel never gets a
//! local path.
use std::path::PathBuf;
use std::time::Instant;

use egui::ViewportId;
use horizon_core::PanelId;
use horizon_core::cloud_runtime::file_drop::Destination;

use super::worker::{LocalUploadFile, UploadOutcome, UploadSnapshot, start_cloud_upload};
use super::{SshUploadFlow, UploadMode, UploadTransportChoice};
use crate::app::HorizonApp;
use crate::app::file_drop::format_posix_path;

/// The worker session that a drop on a cloud panel goes to, captured at the drop.
#[derive(Clone, Debug)]
pub(in crate::app) struct CloudDrop {
    /// The cloud's `issue`, which [`HorizonApp::cloud_file_drop_recipient`] looks up again.
    pub(in crate::app) cloud: u32,
    pub(in crate::app) destination: Destination,
}

/// What a drop on a member of a cloud does: upload, or show why it cannot.
pub(in crate::app) struct CloudDropTarget {
    pub(in crate::app) label: String,
    pub(in crate::app) drop: Result<CloudDrop, String>,
}

pub(super) struct CloudUpload {
    pub(super) drop: CloudDrop,
    paths: Vec<PathBuf>,
    /// The person clicked **Cancel Upload**. It wins over a success that the worker
    /// thread reported before the cancel reached it.
    cancelled: bool,
}

impl SshUploadFlow {
    fn cloud(target_viewport_id: ViewportId, host_label: String, drop: CloudDrop, paths: Vec<PathBuf>) -> Self {
        // Sizes come from the worker thread; the UI thread reads no file.
        let files = paths
            .iter()
            .map(|path| LocalUploadFile {
                path: path.clone(),
                name: path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                ),
                size_bytes: 0,
            })
            .collect();
        let mut flow = Self::with_mode(target_viewport_id, host_label, UploadMode::Preparing);
        flow.files = files;
        flow.cloud = Some(CloudUpload {
            drop,
            paths,
            cancelled: false,
        });
        flow.start_cloud_upload();
        flow
    }

    /// Starts the upload, again on **Try Again**, to the session captured at the drop.
    pub(super) fn start_cloud_upload(&mut self) {
        let Some(cloud) = self.cloud.as_mut() else {
            return;
        };
        cloud.cancelled = false;
        self.transport_choice = UploadTransportChoice::Ssh;
        self.upload_snapshot = Some(UploadSnapshot {
            completed_files: 0,
            total_files: self.files.len(),
            completed_bytes: 0,
            total_bytes: 0,
            current_file_name: None,
            current_file_size: None,
            detail: "Starting upload…".to_string(),
        });
        self.upload_started_at = Some(Instant::now());
        self.upload_handle = Some(start_cloud_upload(cloud.drop.destination.clone(), cloud.paths.clone()));
        self.mode = UploadMode::Uploading;
    }
}

impl SshUploadFlow {
    pub(super) fn note_cloud_cancel(&mut self) {
        if let Some(cloud) = self.cloud.as_mut() {
            cloud.cancelled = true;
        }
    }
}

impl HorizonApp {
    /// Takes a drop on a member of a cloud. Returns `false` for any other panel.
    pub(super) fn maybe_start_cloud_file_drop(
        &mut self,
        panel_id: PanelId,
        dropped: &[egui::DroppedFileHandle],
        viewport_id: ViewportId,
    ) -> bool {
        let Some(target) = self.cloud_file_drop_target(panel_id) else {
            return false;
        };
        if self
            .ssh_upload_flow
            .as_ref()
            .is_some_and(|flow| flow.mode.is_uploading())
        {
            return true;
        }
        self.ssh_upload_flow = Some(match target.drop {
            Ok(drop) => {
                self.board.focus(panel_id);
                let paths = dropped.iter().map(|file| file.path().to_path_buf()).collect();
                SshUploadFlow::cloud(viewport_id, target.label, drop, paths)
            }
            Err(reason) => SshUploadFlow::failed(viewport_id, target.label, reason),
        });
        true
    }

    /// Pastes the worker paths into the panel of the session that the files went to. When
    /// that panel is gone, detached or on another worker, the flow shows the paths instead.
    pub(super) fn finish_cloud_upload(&mut self, outcome: UploadOutcome) {
        let Some((drop, cancelled)) = self
            .ssh_upload_flow
            .as_ref()
            .and_then(|flow| flow.cloud.as_ref())
            .map(|cloud| (cloud.drop.clone(), cloud.cancelled))
        else {
            return;
        };
        if cancelled {
            if let Some(flow) = self.ssh_upload_flow.as_mut() {
                let detail = format!(
                    "The cancel came after every file arrived, so Horizon did not insert the paths. The files are on the worker at:\n{}",
                    outcome.remote_paths.join("\n")
                );
                flow.mode = UploadMode::Finished(UploadOutcome {
                    cancelled: true,
                    detail,
                    ..outcome
                });
            }
            return;
        }
        let recipient = self.cloud_file_drop_recipient(&drop);
        // The drop focused the panel, and the upload window held the keys since. Give
        // them back to it, as a local drop does, unless the person chose another panel.
        let refocus = recipient.is_some() && self.board.focused == recipient;
        if let Some(panel_id) = recipient
            && self.paste_native_text_into_terminal(panel_id, &cloud_terminal_text(&outcome.remote_paths), refocus)
        {
            self.ssh_upload_flow = None;
            return;
        }
        if let Some(flow) = self.ssh_upload_flow.as_mut() {
            let detail = format!(
                "The panel of this session is closed or no longer attached to the same worker, so Horizon did not insert the paths. The files are on the worker at:\n{}",
                outcome.remote_paths.join("\n")
            );
            flow.mode = UploadMode::Finished(UploadOutcome { detail, ..outcome });
        }
    }
}

/// The worker paths as one paste for a terminal on the worker, which is always POSIX.
fn cloud_terminal_text(paths: &[String]) -> String {
    let mut text = paths
        .iter()
        .map(|path| format_posix_path(path))
        .collect::<Vec<_>>()
        .join(" ");
    text.push(' ');
    text
}

#[cfg(test)]
impl SshUploadFlow {
    /// What the flow shows: `Uploading`, `Failed: …`, `Finished: …` or `Ready`.
    pub(in crate::app) fn test_summary(&self) -> String {
        match &self.mode {
            UploadMode::Uploading => "Uploading".into(),
            UploadMode::Failed(error) => format!("Failed: {error}"),
            UploadMode::Finished(outcome) => format!("Finished: {}", outcome.detail),
            UploadMode::Ready | UploadMode::Preparing => "Ready".into(),
        }
    }

    /// As a click on **Cancel Upload** that comes after the worker thread reported.
    pub(in crate::app) fn test_cancel(&mut self) {
        self.note_cloud_cancel();
    }

    /// Replaces the upload worker with one that reports `result` at the next poll.
    pub(in crate::app) fn test_finish(&mut self, result: Result<Vec<String>, String>) {
        let (handle, sender) = super::worker::UploadWorkerHandle::for_test();
        let result = result.map(|remote_paths| UploadOutcome {
            cancelled: false,
            completed_files: remote_paths.len(),
            total_files: remote_paths.len(),
            completed_bytes: 0,
            total_bytes: 0,
            detail: String::new(),
            remote_paths,
        });
        let _ = sender.send(super::worker::UploadMessage::Finished(result));
        self.upload_handle = Some(handle);
    }
}

#[cfg(test)]
mod tests;
