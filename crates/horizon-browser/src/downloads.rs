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
        let incomplete = self.incomplete_files();
        let waiting: Vec<&str> = self
            .observed
            .iter()
            .filter(|observed| Self::is_waiting(&observed.row))
            .map(|observed| observed.row.file_name.as_str())
            .collect();
        BrowserDownloadList {
            directory: self.directory.clone(),
            reporting: self.reporting,
            downloads: self
                .observed
                .iter()
                .map(|observed| {
                    let size = Self::is_waiting(&observed.row)
                        .then(|| Self::measured(&observed.row.file_name, &incomplete, &waiting))
                        .flatten();
                    Self::with_progress(observed, size)
                })
                .collect(),
            dropped: self.dropped,
        }
    }

    /// Whether a transfer still runs without a byte count of its own, which is
    /// the only row that a file on disk can describe.
    fn is_waiting(row: &BrowserDownload) -> bool {
        row.state == BrowserDownloadState::InProgress && row.received_bytes == 0
    }

    /// The bytes of the one file on disk that belongs to this download.
    ///
    /// Two transfers can share a name, and a browser then states a different one
    /// for the second (`export(1).csv`), or leaves a stale file of an earlier
    /// run beside it. A name that matches more than one row, or one file that
    /// more than one waiting row could claim, describes no single transfer, so it
    /// states no size instead of another transfer's bytes.
    fn measured(file_name: &str, incomplete: &[(String, u64)], waiting: &[&str]) -> Option<u64> {
        let mut matched = incomplete
            .iter()
            .filter(|(name, _)| names_one_transfer(file_name, name));
        let (name, size) = matched.next()?;
        if matched.next().is_some() {
            return None;
        }
        let mut claimants = waiting.iter().filter(|other| names_one_transfer(other, name));
        claimants.next()?;
        if claimants.next().is_some() {
            return None;
        }
        Some(*size)
    }

    /// The row of one observed download, with the bytes of a transfer that the
    /// browser does not count measured on disk.
    ///
    /// Firefox reports no byte counts at all, so an in-progress row of its
    /// session would otherwise read zero until the file finished. Both engines
    /// write the bytes they have into a file beside the one they will save —
    /// Chromium adds `.crdownload` and Firefox adds `.part` — and the folder is
    /// this panel's own, so a read measures the transfer without reading the
    /// page's content.
    fn with_progress(observed: &ObservedDownload, size: Option<u64>) -> BrowserDownload {
        let mut row = observed.row.clone();
        if let Some(size) = size.filter(|_| Self::is_waiting(&row)) {
            row.received_bytes = row.total_bytes.map_or(size, |total| size.min(total));
        }
        row
    }

    /// The files of the panel folder that a transfer is still writing, with the
    /// bytes each one holds.
    ///
    /// Both engines write the bytes they have beside the file they will save:
    /// Chromium adds `.crdownload` to the whole name, and Firefox writes
    /// `<stem>.<random>.<extension>.part`. One listing serves every row of one
    /// report, so a folder with many downloads is read once.
    fn incomplete_files(&self) -> Vec<(String, u64)> {
        let Ok(entries) = std::fs::read_dir(&self.directory) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                let size = entry.metadata().ok()?.len();
                is_incomplete(&name).then_some((name, size))
            })
            .collect()
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
    /// A page can start a download in a child frame, and the event then names
    /// that frame's context, so the context is not a filter: every event of this
    /// browser names a file the browser saved in the panel folder.
    pub(crate) fn firefox_event(&mut self, method: &str, params: &serde_json::Value) {
        // Firefox names the download `download` and the navigation that started
        // it `navigation`; one navigation can start several downloads, so the
        // download wins. The BiDi specification states only `navigation`, which
        // is the identity an engine that sends no separate one provides.
        let Some(id) = text(params, "download").or_else(|| text(params, "navigation")) else {
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

/// Whether a name in the panel folder is a file that a browser is still
/// writing. Both engines keep the part of the name a person sees.
fn is_incomplete(name: &str) -> bool {
    // The suffix is compared without case, because a browser states its own
    // spelling and a file that a transfer writes is still that transfer's file.
    [".crdownload", ".part"]
        .iter()
        .any(|suffix| name.to_ascii_lowercase().ends_with(suffix))
}

/// Whether an incomplete file belongs to the download whose file name this is.
///
/// Chromium states the whole name and appends its suffix. Firefox keeps the stem
/// and the extension and puts a random part between them, so the comparison
/// covers both shapes and never matches a different extension.
fn names_one_transfer(file_name: &str, incomplete: &str) -> bool {
    if file_name.is_empty() {
        return false;
    }
    if incomplete == format!("{file_name}.crdownload") || incomplete == format!("{file_name}.part") {
        return true;
    }
    let stem = file_name.rsplit_once('.').map_or(file_name, |(stem, _)| stem);
    let extension = file_name.rsplit_once('.').map(|(_, extension)| extension);
    let Some(rest) = incomplete.strip_prefix(&format!("{stem}.")) else {
        return false;
    };
    let Some(rest) = rest.strip_suffix(".part") else {
        return false;
    };
    let Some((random, billed)) = rest.rsplit_once('.') else {
        return false;
    };
    !random.is_empty() && Some(billed) == extension
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
        );
        downloads.firefox_event(
            "browsingContext.downloadEnd",
            &json!({
                "context": "ctx",
                "download": "d1",
                "status": "complete",
                "filepath": saved.to_string_lossy(),
            }),
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
        );
        let row = outside.list().downloads.remove(0);
        assert_eq!(row.received_bytes, 0);
        assert_eq!(row.total_bytes, None);
    }

    #[test]
    fn an_in_progress_row_counts_the_bytes_the_browser_wrote_so_far() {
        let folder = tempfile::tempdir().expect("temp folder");
        std::fs::write(folder.path().join("slow.pdf.part"), b"1200").expect("write");
        let mut downloads = DownloadState::new(folder.path().to_string_lossy().to_string(), true);
        downloads.firefox_event(
            "browsingContext.downloadWillBegin",
            &json!({"context": "ctx", "download": "d1", "suggestedFilename": "slow.pdf"}),
        );
        let row = downloads.list().downloads.remove(0);
        assert_eq!(row.state, BrowserDownloadState::InProgress);
        assert_eq!(row.received_bytes, 4);
        assert_eq!(row.total_bytes, None);

        // The transfer continues, so the next read states the new size.
        std::fs::write(folder.path().join("slow.pdf.part"), b"1200more").expect("write");
        assert_eq!(downloads.list().downloads.remove(0).received_bytes, 8);
    }

    #[test]
    fn a_firefox_transfer_is_measured_through_its_random_part_name() {
        let folder = tempfile::tempdir().expect("temp folder");
        // Firefox writes `<stem>.<random>.<extension>.part`, so the name a row
        // states is not the name of the file that holds the bytes.
        std::fs::write(folder.path().join("slow.2ImBNVAW.bin.part"), b"1200").expect("write");
        std::fs::write(folder.path().join("slow.bin"), b"").expect("write");
        let mut downloads = DownloadState::new(folder.path().to_string_lossy().to_string(), true);
        downloads.firefox_event(
            "browsingContext.downloadWillBegin",
            &json!({"context": "ctx", "download": "d1", "suggestedFilename": "slow.bin"}),
        );
        assert_eq!(downloads.list().downloads.remove(0).received_bytes, 4);

        // Another extension is another transfer.
        std::fs::write(folder.path().join("other.9Xq.bin.part"), b"12345678").expect("write");
        assert_eq!(downloads.list().downloads.remove(0).received_bytes, 4);
    }

    #[test]
    fn two_transfers_of_one_name_never_state_one_file_twice() {
        let folder = tempfile::tempdir().expect("temp folder");
        std::fs::write(folder.path().join("export.2ImBNVAW.csv.part"), b"1200").expect("write");
        let mut downloads = DownloadState::new(folder.path().to_string_lossy().to_string(), true);
        for download in ["d1", "d2"] {
            downloads.firefox_event(
                "browsingContext.downloadWillBegin",
                &json!({"context": "ctx", "download": download, "suggestedFilename": "export.csv"}),
            );
        }
        let rows = downloads.list().downloads;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].received_bytes, 0);
        assert_eq!(rows[1].received_bytes, 0);

        // One transfer of that name is one file, so it states its size.
        let mut single = DownloadState::new(folder.path().to_string_lossy().to_string(), true);
        single.firefox_event(
            "browsingContext.downloadWillBegin",
            &json!({"context": "ctx", "download": "d1", "suggestedFilename": "export.csv"}),
        );
        assert_eq!(single.list().downloads.remove(0).received_bytes, 4);
    }

    #[test]
    fn an_in_progress_chromium_row_never_overstates_its_total() {
        let folder = tempfile::tempdir().expect("temp folder");
        std::fs::write(folder.path().join("slow.pdf.crdownload"), b"1200").expect("write");
        let mut downloads = DownloadState::new(folder.path().to_string_lossy().to_string(), true);
        downloads.chromium_will_begin(&json!({"guid": "g1", "suggestedFilename": "slow.pdf"}));
        // Chromium states its own count when it reports progress, so only a row
        // without one is measured.
        assert_eq!(downloads.list().downloads.remove(0).received_bytes, 4);
        downloads.chromium_progress(&json!({"guid": "g2", "suggestedFilename": "other.pdf", "receivedBytes": 2, "totalBytes": 3, "state": "inProgress"}));
        let row = downloads.list().downloads.remove(1);
        assert_eq!(row.received_bytes, 2);
        assert_eq!(row.total_bytes, Some(3));

        // A measured size never passes the total that the browser stated.
        std::fs::write(folder.path().join("small.pdf.crdownload"), b"1200").expect("write");
        downloads.chromium_progress(&json!({"guid": "g3", "suggestedFilename": "small.pdf", "receivedBytes": 0, "totalBytes": 2, "state": "inProgress"}));
        let row = downloads.list().downloads.remove(2);
        assert_eq!(row.received_bytes, 2);
        assert_eq!(row.total_bytes, Some(2));
    }

    #[test]
    fn a_finished_row_keeps_the_count_of_the_browser() {
        let folder = tempfile::tempdir().expect("temp folder");
        std::fs::write(folder.path().join("done.pdf.part"), b"999999").expect("write");
        let mut downloads = DownloadState::new(folder.path().to_string_lossy().to_string(), true);
        downloads.firefox_event(
            "browsingContext.downloadEnd",
            &json!({"context": "ctx", "download": "d1", "status": "canceled"}),
        );
        let row = downloads.list().downloads.remove(0);
        assert_eq!(row.state, BrowserDownloadState::Canceled);
        assert_eq!(row.received_bytes, 0);
    }

    #[test]
    fn firefox_downloads_of_one_navigation_stay_separate_rows() {
        // Firefox 159 sends one `navigation` for every file a navigation
        // starts, so rows that keyed on it alone would merge.
        let mut downloads = state();
        for (order, name) in ["one.pdf", "two.pdf"].into_iter().enumerate() {
            downloads.firefox_event(
                "browsingContext.downloadWillBegin",
                &json!({
                    "context": "ctx",
                    "download": format!("file-{order}"),
                    "navigation": "navigation-1",
                    "suggestedFilename": name,
                }),
            );
        }
        let list = downloads.list();
        assert_eq!(list.downloads.len(), 2);
        assert_eq!(list.downloads[0].file_name, "one.pdf");
        assert_eq!(list.downloads[1].file_name, "two.pdf");
    }

    #[test]
    fn a_firefox_event_with_only_the_specified_navigation_identity_is_recorded() {
        let mut downloads = state();
        downloads.firefox_event(
            "browsingContext.downloadWillBegin",
            &json!({
                "context": "ctx",
                "navigation": "navigation-2",
                "suggestedFilename": "report.pdf",
            }),
        );
        downloads.firefox_event(
            "browsingContext.downloadEnd",
            &json!({"context": "ctx", "navigation": "navigation-2", "status": "canceled"}),
        );
        let list = downloads.list();
        assert_eq!(list.downloads.len(), 1);
        assert_eq!(list.downloads[0].state, BrowserDownloadState::Canceled);
    }

    #[test]
    fn a_download_a_child_frame_starts_is_recorded() {
        // The event names the frame's context, and the file still lands in the
        // panel folder, so the row must not be filtered away.
        let mut downloads = state();
        downloads.firefox_event(
            "browsingContext.downloadWillBegin",
            &json!({
                "context": "frame-context",
                "download": "d1",
                "suggestedFilename": "report.pdf",
            }),
        );
        downloads.firefox_event(
            "browsingContext.downloadEnd",
            &json!({
                "context": "frame-context",
                "download": "d1",
                "status": "complete",
                "filepath": "/home/alice/Horizon/downloads/report.pdf",
            }),
        );
        let list = downloads.list();
        assert_eq!(list.downloads.len(), 1);
        assert_eq!(list.downloads[0].state, BrowserDownloadState::Completed);
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
