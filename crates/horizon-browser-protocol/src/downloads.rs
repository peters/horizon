//! Public download values for one browser panel.
//!
//! A panel saves every download of its page into one panel-owned folder. The
//! values here are what the engine reports about that folder: the folder path,
//! whether this exact session observed download events, and a bounded list of
//! the downloads it observed.

use serde::{Deserialize, Serialize};

/// Downloads one panel keeps in its report. Older rows leave the report; their
/// files stay in the folder.
pub const MAX_BROWSER_DOWNLOADS: usize = 64;

/// Longest file name a row carries. A page names its own download, so the name
/// is untrusted text and is cut to this many bytes on a character boundary.
pub const MAX_BROWSER_DOWNLOAD_NAME_BYTES: usize = 128;

/// Where one download stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDownloadState {
    /// The engine is still receiving bytes.
    InProgress,
    /// The file is complete and [`BrowserDownload::path`] is its final path.
    Completed,
    /// The page, the browser or the network stopped this download.
    Canceled,
}

impl BrowserDownloadState {
    /// Whether no more bytes will arrive for this download.
    #[must_use]
    pub const fn finished(self) -> bool {
        !matches!(self, Self::InProgress)
    }
}

/// One download an engine observed for a panel.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct BrowserDownload {
    /// File name the browser used, never a host path chosen by the page.
    pub file_name: String,
    pub state: BrowserDownloadState,
    /// Bytes the engine has written so far.
    pub received_bytes: u64,
    /// Size the engine expects, when the response stated it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    /// Absolute path of the saved file. Present once the browser named the
    /// file it wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// One panel's download folder and the downloads this session observed.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct BrowserDownloadList {
    /// Panel-owned folder that every download of this panel is saved into.
    pub directory: String,
    /// Whether this exact session observed download events. When it is false
    /// the engine saved the page's downloads without reporting them, so an
    /// empty list is not proof that nothing was downloaded.
    pub reporting: bool,
    #[serde(default)]
    pub downloads: Vec<BrowserDownload>,
    /// Reports this session forgot out of its bounded retention.
    #[serde(default)]
    pub dropped: u64,
}

/// The file name a browser reported, made safe to show to a caller: the last
/// path component, no control characters, and at most
/// [`MAX_BROWSER_DOWNLOAD_NAME_BYTES`] bytes on a character boundary.
#[must_use]
pub fn download_file_name(reported: &str) -> String {
    let component = reported
        .rsplit(['/', '\\'])
        .find(|component| !component.is_empty() && *component != "." && *component != "..")
        .unwrap_or("download");
    let cleaned: String = component.chars().filter(|character| !character.is_control()).collect();
    if cleaned.len() <= MAX_BROWSER_DOWNLOAD_NAME_BYTES {
        return cleaned;
    }
    let mut cut = String::new();
    for character in cleaned.chars() {
        if cut.len() + character.len_utf8() > MAX_BROWSER_DOWNLOAD_NAME_BYTES {
            break;
        }
        cut.push(character);
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reported_name_keeps_only_its_last_path_component() {
        assert_eq!(download_file_name("report.pdf"), "report.pdf");
        assert_eq!(download_file_name("/etc/passwd"), "passwd");
        assert_eq!(download_file_name("..\\..\\windows\\evil.exe"), "evil.exe");
        assert_eq!(download_file_name("../../"), "download");
        assert_eq!(download_file_name(""), "download");
    }

    #[test]
    fn a_reported_name_loses_control_characters_and_stays_bounded() {
        assert_eq!(download_file_name("a\u{0}b\u{1f}c"), "abc");
        let long = "é".repeat(200);
        let cut = download_file_name(&long);
        assert!(cut.len() <= MAX_BROWSER_DOWNLOAD_NAME_BYTES);
        assert!(cut.chars().all(|character| character == 'é'));
        assert_eq!(cut.chars().count(), MAX_BROWSER_DOWNLOAD_NAME_BYTES / 2);
    }

    #[test]
    fn only_in_progress_downloads_are_unfinished() {
        assert!(!BrowserDownloadState::InProgress.finished());
        assert!(BrowserDownloadState::Completed.finished());
        assert!(BrowserDownloadState::Canceled.finished());
    }
}
