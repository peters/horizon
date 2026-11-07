//! One panel's download folder and the bounded report of what landed in it.
//!
//! A page names its own downloads, so every value here comes from untrusted
//! browser input: the engine keeps only the file name, the state, the byte
//! counts and the path the browser itself reports. It never returns the URL a
//! download came from, because a download URL can carry a credential.
//!
//! The engine writes no file. The browser writes into a folder that Horizon
//! owns for this panel, and this state only reports it.

use std::collections::VecDeque;
use std::path::Path;

use horizon_browser_protocol::{
    BrowserDownload, BrowserDownloadList, BrowserDownloadState, MAX_BROWSER_DOWNLOADS, download_file_name,
};

/// Create the panel's download folder as a private directory, the way a
/// browser profile is created: only this user may read what a page saved.
pub(crate) fn prepare_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// One observed download. `id` is the engine's own identity for it and never
/// leaves this module.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ObservedDownload {
    id: String,
    row: BrowserDownload,
}

/// The downloads observed by one session, newest last.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DownloadState {
    directory: String,
    /// Whether this session can observe downloads at all. A browser that
    /// cannot is older than the events Horizon reads, or its configuration was
    /// refused.
    reporting: bool,
    observed: VecDeque<ObservedDownload>,
    dropped: u64,
}

impl DownloadState {
    pub(crate) fn new(directory: impl Into<String>, reporting: bool) -> Self {
        Self {
            directory: directory.into(),
            reporting,
            observed: VecDeque::new(),
            dropped: 0,
        }
    }

    pub(crate) fn reporting(&self) -> bool {
        self.reporting
    }

    #[must_use]
    pub(crate) fn list(&self) -> BrowserDownloadList {
        BrowserDownloadList {
            directory: self.directory.clone(),
            reporting: self.reporting,
            downloads: self.observed.iter().map(|observed| observed.row.clone()).collect(),
            dropped: self.dropped,
        }
    }

    /// One Chromium browser-level download event.
    pub(crate) fn chromium_event(&mut self, method: &str, params: &serde_json::Value) {
        match method {
            "Browser.downloadWillBegin" => self.chromium_will_begin(params),
            "Browser.downloadProgress" => self.chromium_progress(params),
            _ => {}
        }
    }

    /// Chromium's `Browser.downloadWillBegin`: the browser accepted a download
    /// and named the file it will write.
    fn chromium_will_begin(&mut self, params: &serde_json::Value) {
        let Some(id) = text(params, "guid") else {
            return;
        };
        let file_name = download_file_name(text(params, "suggestedFilename").unwrap_or_default().as_str());
        self.upsert(ObservedDownload {
            id,
            row: BrowserDownload {
                file_name,
                state: BrowserDownloadState::InProgress,
                received_bytes: 0,
                total_bytes: None,
                path: None,
            },
        });
    }

    /// Chromium's `Browser.downloadProgress`: bytes, a new state, and — once
    /// the browser finished the file — its final path. Chromium also reports
    /// this event before the first `downloadWillBegin` for a resumed download,
    /// so an unknown identity becomes an observed download here.
    fn chromium_progress(&mut self, params: &serde_json::Value) {
        let Some(id) = text(params, "guid") else {
            return;
        };
        let state = match text(params, "state").as_deref() {
            Some("completed") => BrowserDownloadState::Completed,
            Some("canceled") => BrowserDownloadState::Canceled,
            _ => BrowserDownloadState::InProgress,
        };
        let path = text(params, "filePath");
        // The browser names the file it wrote only once the download finished.
        // Until then the row keeps the name the page suggested.
        let suggested = || download_file_name(text(params, "suggestedFilename").unwrap_or_default().as_str());
        let known = self.known(&id).cloned();
        let file_name = path.as_deref().map_or_else(
            || {
                known
                    .as_ref()
                    .map_or_else(suggested, |known| known.row.file_name.clone())
            },
            download_file_name,
        );
        let row = BrowserDownload {
            file_name,
            state,
            received_bytes: count(params, "receivedBytes"),
            total_bytes: count_optional(params, "totalBytes"),
            path: path.or_else(|| known.and_then(|known| known.row.path)),
        };
        self.upsert(ObservedDownload { id, row });
    }

    /// Firefox's `browsingContext.downloadWillBegin` and `downloadEnd` events.
    /// They arrive per browsing context, so only the panel's own context is
    /// recorded.
    pub(crate) fn firefox_event(&mut self, method: &str, params: &serde_json::Value, context_id: Option<&str>) {
        if context_id
            .zip(text(params, "context").as_deref())
            .is_some_and(|(expected, reported)| expected != reported)
        {
            return;
        }
        let Some(id) = text(params, "download") else {
            return;
        };
        if method == "browsingContext.downloadWillBegin" {
            let file_name = download_file_name(text(params, "suggestedFilename").unwrap_or_default().as_str());
            self.upsert(ObservedDownload {
                id,
                row: BrowserDownload {
                    file_name,
                    state: BrowserDownloadState::InProgress,
                    received_bytes: 0,
                    total_bytes: None,
                    path: None,
                },
            });
            return;
        }
        if method != "browsingContext.downloadEnd" {
            return;
        }
        let completed = text(params, "status").as_deref() == Some("complete");
        let path = text(params, "filepath");
        let known = self.known(&id).cloned();
        let file_name = path.as_deref().map_or_else(
            || {
                known
                    .as_ref()
                    .map_or_else(|| "download".to_string(), |known| known.row.file_name.clone())
            },
            download_file_name,
        );
        // Firefox reports no byte counts, so a finished file is measured on
        // disk. Only a path inside the panel folder is read.
        let size = completed
            .then(|| path.as_deref().and_then(|path| self.saved_file_size(path)))
            .flatten();
        self.upsert(ObservedDownload {
            id,
            row: BrowserDownload {
                file_name,
                state: if completed {
                    BrowserDownloadState::Completed
                } else {
                    BrowserDownloadState::Canceled
                },
                received_bytes: size.unwrap_or_else(|| known.as_ref().map_or(0, |known| known.row.received_bytes)),
                total_bytes: size.or_else(|| known.and_then(|known| known.row.total_bytes)),
                path: completed.then_some(path).flatten(),
            },
        });
    }

    /// The size of a file the browser named, when that path is inside this
    /// panel's folder. A path outside it is not this panel's file to read.
    fn saved_file_size(&self, path: &str) -> Option<u64> {
        let path = Path::new(path);
        path.strip_prefix(&self.directory).ok()?;
        std::fs::metadata(path).ok()?.len().into()
    }

    /// Record one download, replacing the row with the same identity and
    /// keeping the newest [`MAX_BROWSER_DOWNLOADS`] rows.
    fn upsert(&mut self, observed: ObservedDownload) {
        if let Some(position) = self.observed.iter().position(|known| known.id == observed.id) {
            self.observed.remove(position);
        }
        self.observed.push_back(observed);
        while self.observed.len() > MAX_BROWSER_DOWNLOADS {
            self.observed.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    fn known(&self, id: &str) -> Option<&ObservedDownload> {
        self.observed.iter().find(|known| known.id == id)
    }
}

fn text(params: &serde_json::Value, field: &str) -> Option<String> {
    params
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn count(params: &serde_json::Value, field: &str) -> u64 {
    count_optional(params, field).unwrap_or(0)
}

/// A byte count the browser reported. A value that is not a non-negative
/// integer is not a count, so it is left out instead of rounded.
fn count_optional(params: &serde_json::Value, field: &str) -> Option<u64> {
    params.get(field)?.as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> DownloadState {
        DownloadState::new("/home/alice/Horizon/downloads", true)
    }

    #[test]
    fn a_chromium_download_reports_its_own_path_and_name() {
        let mut downloads = state();
        downloads.chromium_will_begin(&json!({
            "guid": "g1",
            "url": "https://example.test/secret?token=abc",
            "suggestedFilename": "../../quarterly.pdf",
        }));
        downloads.chromium_progress(&json!({
            "guid": "g1",
            "totalBytes": 4096,
            "receivedBytes": 1024,
            "state": "inProgress",
        }));
        downloads.chromium_progress(&json!({
            "guid": "g1",
            "totalBytes": 4096,
            "receivedBytes": 4096,
            "state": "completed",
            "filePath": "/home/alice/Horizon/downloads/quarterly (1).pdf",
        }));

        let list = downloads.list();
        assert_eq!(list.directory, "/home/alice/Horizon/downloads");
        assert!(list.reporting);
        assert_eq!(list.downloads.len(), 1);
        assert_eq!(list.downloads[0].file_name, "quarterly (1).pdf");
        assert_eq!(list.downloads[0].state, BrowserDownloadState::Completed);
        assert_eq!(list.downloads[0].received_bytes, 4096);
        assert_eq!(list.downloads[0].total_bytes, Some(4096));
        assert_eq!(
            list.downloads[0].path.as_deref(),
            Some("/home/alice/Horizon/downloads/quarterly (1).pdf")
        );
        // A download URL can carry a credential, so it is never part of a row.
        assert!(!serde_json::to_string(&list).unwrap_or_default().contains("token=abc"));
    }

    #[test]
    fn a_canceled_chromium_download_keeps_its_suggested_name() {
        let mut downloads = state();
        downloads.chromium_will_begin(&json!({"guid": "g2", "suggestedFilename": "report.pdf"}));
        downloads.chromium_progress(&json!({"guid": "g2", "state": "canceled", "receivedBytes": 12}));

        let list = downloads.list();
        assert_eq!(list.downloads[0].state, BrowserDownloadState::Canceled);
        assert_eq!(list.downloads[0].file_name, "report.pdf");
        assert_eq!(list.downloads[0].path, None);
    }

    #[test]
    fn a_firefox_download_reports_the_path_the_browser_saved() {
        let mut downloads = state();
        downloads.firefox_event(
            "browsingContext.downloadWillBegin",
            &json!({
                "context": "ctx",
                "download": "d1",
                "suggestedFilename": "invoice.pdf",
                "url": "https://example.test/invoice.pdf",
            }),
            Some("ctx"),
        );
        assert_eq!(downloads.list().downloads[0].state, BrowserDownloadState::InProgress);

        downloads.firefox_event(
            "browsingContext.downloadEnd",
            &json!({
                "context": "ctx",
                "download": "d1",
                "status": "complete",
                "filepath": "/home/alice/Horizon/downloads/invoice.pdf",
            }),
            Some("ctx"),
        );

        let list = downloads.list();
        assert_eq!(list.downloads.len(), 1);
        assert_eq!(list.downloads[0].file_name, "invoice.pdf");
        assert_eq!(list.downloads[0].state, BrowserDownloadState::Completed);
        assert_eq!(
            list.downloads[0].path.as_deref(),
            Some("/home/alice/Horizon/downloads/invoice.pdf")
        );
    }

    #[test]
    fn a_finished_firefox_download_is_measured_on_disk_inside_the_folder() {
        let folder = tempfile::tempdir().expect("temp folder");
        let saved = folder.path().join("invoice.pdf");
        std::fs::write(&saved, b"report bytes").expect("write");
        let mut downloads = DownloadState::new(folder.path().to_string_lossy().to_string(), true);
        downloads.firefox_event(
            "browsingContext.downloadWillBegin",
            &json!({"context": "ctx", "download": "d1", "suggestedFilename": "invoice.pdf"}),
            Some("ctx"),
        );
        downloads.firefox_event(
            "browsingContext.downloadEnd",
            &json!({
                "context": "ctx",
                "download": "d1",
                "status": "complete",
                "filepath": saved.to_string_lossy(),
            }),
            Some("ctx"),
        );
        let row = downloads.list().downloads.remove(0);
        assert_eq!(row.received_bytes, 12);
        assert_eq!(row.total_bytes, Some(12));

        // A path that is not inside this panel's folder is named, not read.
        let mut outside = DownloadState::new(folder.path().join("other").to_string_lossy().to_string(), true);
        outside.firefox_event(
            "browsingContext.downloadEnd",
            &json!({
                "context": "ctx",
                "download": "d2",
                "status": "complete",
                "filepath": saved.to_string_lossy(),
            }),
            Some("ctx"),
        );
        let row = outside.list().downloads.remove(0);
        assert_eq!(row.received_bytes, 0);
        assert_eq!(row.total_bytes, None);
    }

    #[test]
    fn firefox_events_for_another_context_are_not_recorded() {
        let mut downloads = state();
        downloads.firefox_event(
            "browsingContext.downloadWillBegin",
            &json!({"context": "elsewhere", "download": "d1", "suggestedFilename": "x.pdf"}),
            Some("ctx"),
        );
        assert!(downloads.list().downloads.is_empty());
    }

    #[test]
    fn retention_keeps_the_newest_reports_and_counts_the_rest() {
        let mut downloads = state();
        for index in 0..(MAX_BROWSER_DOWNLOADS + 5) {
            downloads.chromium_will_begin(&json!({"guid": format!("g{index}"), "suggestedFilename": "same.pdf"}));
        }

        let list = downloads.list();
        assert_eq!(list.downloads.len(), MAX_BROWSER_DOWNLOADS);
        assert_eq!(list.dropped, 5);
    }

    #[test]
    fn reports_stay_empty_and_unreported_until_the_session_observes_one() {
        let downloads = DownloadState::new("/home/alice/Horizon/downloads", false);
        let list = downloads.list();
        assert!(!list.reporting);
        assert!(list.downloads.is_empty());
        assert_eq!(list.dropped, 0);
    }
}
