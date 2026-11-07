//! Firefox download events. The browser writes each download into the panel's
//! folder and reports the file it saved; this module only records the report.

use serde_json::Value;

use super::Driver;

/// The download folder of this panel. It is a browser-profile setting, so a
/// panel that shares a Firefox profile shares that profile's folder too.
pub(super) fn panel_downloads(
    config: &crate::BrowserSessionConfig,
    group: Option<&super::super::SharedFirefoxSession>,
    reported: bool,
) -> crate::downloads::DownloadState {
    let profile = group.map_or(config.panel_local_id.as_str(), |group| group.profile_id());
    let directory = config.browser.downloads_dir(profile);
    crate::downloads::DownloadState::new(directory.to_string_lossy().to_string(), reported)
}

impl Driver {
    /// `browsingContext.downloadWillBegin` and `browsingContext.downloadEnd`
    /// for this browser. A download a child frame starts names that frame's
    /// context, so the context is not a filter here.
    pub(super) fn handle_download_bidi_event(&mut self, event: &Value) -> bool {
        let Some(method) = event.get("method").and_then(Value::as_str) else {
            return false;
        };
        if !super::bidi::DOWNLOAD_EVENTS.contains(&method) {
            return false;
        }
        let params = event.get("params").unwrap_or(&Value::Null);
        self.downloads.firefox_event(method, params);
        true
    }
}
