//! `browser_downloads`: the panel's download folder and what this session saved.

use horizon_browser::{BrowserDownload, BrowserDownloadList, BrowserDownloadState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
pub(crate) struct DownloadsInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// Per-action timeout in milliseconds (1-60000).
    pub(crate) timeout_millis: Option<u64>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct DownloadsOutput {
    pub(crate) panel_id: String,
    pub(crate) action_id: String,
    /// Panel-owned folder that every download of this panel is saved into.
    directory: String,
    /// Whether this exact session observed download events. When it is false
    /// the browser saved downloads into its own destination and this list
    /// cannot show them.
    reporting: bool,
    /// Downloads this session observed, newest last.
    downloads: Vec<DownloadRow>,
    /// Reports this session forgot out of its bounded retention. The files stay
    /// in the folder.
    dropped: u64,
    /// What the caller can do with this result.
    next_step: String,
}

#[derive(Serialize, JsonSchema)]
struct DownloadRow {
    /// File name the browser used. It is the page's own choice, cut to a safe
    /// length.
    file_name: String,
    /// `in_progress`, `completed` or `canceled`.
    state: &'static str,
    /// Bytes written so far.
    received_bytes: u64,
    /// Size the browser expected, when the response stated it.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_bytes: Option<u64>,
    /// Absolute path of the saved file, once the browser named it.
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

impl From<BrowserDownload> for DownloadRow {
    fn from(download: BrowserDownload) -> Self {
        Self {
            state: match download.state {
                BrowserDownloadState::InProgress => "in_progress",
                BrowserDownloadState::Completed => "completed",
                BrowserDownloadState::Canceled => "canceled",
            },
            file_name: download.file_name,
            received_bytes: download.received_bytes,
            total_bytes: download.total_bytes,
            path: download.path,
        }
    }
}

impl DownloadsOutput {
    pub(crate) fn new(panel_id: String, action_id: String, downloads: BrowserDownloadList) -> Self {
        let next_step = next_step(&downloads);
        Self {
            panel_id,
            action_id,
            directory: downloads.directory,
            reporting: downloads.reporting,
            downloads: downloads.downloads.into_iter().map(DownloadRow::from).collect(),
            dropped: downloads.dropped,
            next_step,
        }
    }
}

/// Tell the caller what this report does and does not prove.
fn next_step(downloads: &BrowserDownloadList) -> String {
    if !downloads.reporting {
        return format!(
            "This session did not confirm that it observes downloads, so an empty list does not mean nothing was saved. The page's own destination holds any file it saved; open a local Chromium or Firefox panel to report downloads into {}.",
            downloads.directory
        );
    }
    if downloads.downloads.iter().any(|download| !download.state.finished()) {
        return "A download is still in progress: call browser_downloads again to read its state and final path."
            .to_string();
    }
    if downloads.downloads.is_empty() {
        return format!(
            "No download reached {}. A page that saves nothing leaves this list empty; a download a person starts in the panel appears here too.",
            downloads.directory
        );
    }
    format!(
        "Read the file at its path in {}. A completed row names the file the browser wrote; a canceled row left no file.",
        downloads.directory
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(reporting: bool, downloads: Vec<BrowserDownload>) -> BrowserDownloadList {
        BrowserDownloadList {
            directory: "/home/alice/Horizon/downloads".to_string(),
            reporting,
            downloads,
            dropped: 3,
        }
    }

    fn row(state: BrowserDownloadState) -> BrowserDownload {
        BrowserDownload {
            file_name: "report.pdf".to_string(),
            state,
            received_bytes: 2048,
            total_bytes: Some(4096),
            path: state
                .finished()
                .then(|| "/home/alice/Horizon/downloads/report.pdf".to_string()),
        }
    }

    #[test]
    fn a_completed_download_reports_its_path_and_state() {
        let output = DownloadsOutput::new(
            "panel".to_string(),
            "action".to_string(),
            list(true, vec![row(BrowserDownloadState::Completed)]),
        );
        let encoded = serde_json::to_value(&output).unwrap_or_default();
        assert_eq!(encoded["directory"], "/home/alice/Horizon/downloads");
        assert_eq!(encoded["reporting"], true);
        assert_eq!(encoded["dropped"], 3);
        assert_eq!(encoded["downloads"][0]["state"], "completed");
        assert_eq!(
            encoded["downloads"][0]["path"],
            "/home/alice/Horizon/downloads/report.pdf"
        );
        assert!(
            encoded["next_step"]
                .as_str()
                .unwrap_or_default()
                .contains("Read the file")
        );
    }

    #[test]
    fn an_unreported_session_does_not_claim_an_empty_folder() {
        let output = DownloadsOutput::new("panel".to_string(), "action".to_string(), list(false, Vec::new()));
        let encoded = serde_json::to_value(&output).unwrap_or_default();
        assert_eq!(encoded["reporting"], false);
        assert!(
            encoded["next_step"]
                .as_str()
                .unwrap_or_default()
                .contains("does not mean nothing was saved")
        );
    }

    #[test]
    fn an_in_progress_download_asks_the_caller_to_read_it_again() {
        let output = DownloadsOutput::new(
            "panel".to_string(),
            "action".to_string(),
            list(true, vec![row(BrowserDownloadState::InProgress)]),
        );
        let encoded = serde_json::to_value(&output).unwrap_or_default();
        assert_eq!(encoded["downloads"][0]["state"], "in_progress");
        assert_eq!(encoded["downloads"][0]["path"], serde_json::Value::Null);
        assert!(
            encoded["next_step"]
                .as_str()
                .unwrap_or_default()
                .contains("still in progress")
        );
    }
}
