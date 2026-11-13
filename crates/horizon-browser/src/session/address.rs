//! The authoritative address of the page: the read of a finished navigation and
//! the one normalisation of an address that this crate keeps.

use std::sync::Arc;
use std::time::Duration;

use super::{BrowserEvent, BrowserEventSender, CdpLink, DriverState, FrameSlot};

impl DriverState {
    /// Freeze an in-flight navigation, route every event received while that
    /// happens, then ask the target for its authoritative committed URL.
    pub(super) fn capture_final_url(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) {
        if let Some(session_id) = self.session_id.clone() {
            let outcome = link.call_and_drain(
                Duration::from_secs(1),
                "Page.stopLoading",
                &serde_json::json!({}),
                Some(session_id.as_str()),
            );
            for message in outcome.drained {
                self.handle_message(link, event_tx, frame_slot, message);
            }
        }
        if self.navigation_failed {
            return;
        }
        let Some(target_id) = self.target_id.clone() else {
            return;
        };
        let outcome = link.call_and_drain(
            Duration::from_secs(1),
            "Target.getTargetInfo",
            &serde_json::json!({ "targetId": target_id }),
            None,
        );
        for message in outcome.drained {
            self.handle_message(link, event_tx, frame_slot, message);
        }
        let Some(target_url) = outcome.result.ok().and_then(|result| {
            result
                .pointer("/targetInfo/url")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }) else {
            return;
        };
        let url = normalized_committed_url(&target_url).to_string();
        if self.url != url {
            self.url = url;
            self.manifest_dirty = true;
            self.write_manifest(true);
        }
        let _ = event_tx.send(BrowserEvent::UrlChanged(self.url.clone()));
    }
}

pub(super) fn normalized_committed_url(url: &str) -> &str {
    if url.is_empty() || url == "about:blank" {
        ""
    } else {
        url
    }
}

#[cfg(test)]
mod tests;
