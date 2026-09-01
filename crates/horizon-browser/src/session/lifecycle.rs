//! Page-session setup and screencast recovery lifecycle.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::AutomationDisclosurePolicy;
use crate::cdp::CdpLink;
use crate::disclosure::{chromium_user_agent_needs_override, chromium_user_agent_override};
use crate::frames::FrameSlot;

use super::{
    BrowserEvent, BrowserEventSender, DriverState, MAX_RESTART_ATTEMPTS, RESTART_BACKOFF, RESTART_BACKOFF_CAP,
};

impl DriverState {
    fn screencast_params(&self) -> serde_json::Value {
        // Change-driven: frames only arrive when the page repaints, so a
        // static page costs nothing. `everyNthFrame` is the only rate knob
        // this Chrome build exposes; there is no maxFPS parameter.
        serde_json::json!({
            "format": "jpeg",
            "quality": self.config.browser.quality,
            "everyNthFrame": self.config.browser.every_nth_frame.max(1),
        })
    }

    pub(super) fn attach_setup(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        session: &str,
        target: &str,
    ) -> bool {
        if self.session_id.as_deref() != Some(session) {
            self.reset_clipboard_tracking();
            self.invalidate_scrollbar_layout();
            self.reset_runtime_enable_state();
        }
        self.session_id = Some(session.to_string());
        self.target_id = Some(target.to_string());
        // Browser-level auto-attach does not recurse into site-isolated child
        // targets. Enable it on the bound page session as well so OOPIF
        // execution contexts remain available to frame-aware input bridges.
        let pre_observation_setup_commands = [
            (
                "Target.setAutoAttach",
                serde_json::json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
            ),
            ("Page.enable", serde_json::json!({})),
        ];
        for (method, params) in pre_observation_setup_commands {
            if !self.setup_command(link, event_tx, frame_slot, method, &params, Some(session)) {
                return false;
            }
        }
        if !self.resolve_main_frame_id(link, event_tx, frame_slot, session) {
            return false;
        }
        // Observe only top-level response metadata so a completed user
        // handoff can report a repeated Cloudflare challenge. The driver
        // receives response headers but never emits them or request bodies.
        if !self.setup_command(
            link,
            event_tx,
            frame_slot,
            "Network.enable",
            &serde_json::json!({}),
            Some(session),
        ) {
            return false;
        }
        self.restore_network_capture(link, event_tx, frame_slot, session);
        if self.config.browser.automation_disclosure == AutomationDisclosurePolicy::MinimizeCommonSignals
            && !self.install_common_signal_minimization(link, event_tx, frame_slot, session)
        {
            return false;
        }
        if !self.setup_command(
            link,
            event_tx,
            frame_slot,
            "Emulation.setDeviceMetricsOverride",
            &Self::viewport_override_params(self.viewport_w, self.viewport_h),
            Some(session),
        ) {
            return false;
        }
        // Never expose the internal metadata-bootstrap page in a screencast.
        // Native automation-flag suppression and any UA override are already
        // active before this first caller-supplied navigation can execute
        // page author script.
        self.navigate_initial_url(link, event_tx, frame_slot);
        // A page that loaded before we attached (restart case) already has
        // its title; a fresh about:blank fetch returns empty and is skipped.
        self.fetch_title(link, event_tx, frame_slot);
        if self.stop_requested.load(Ordering::Acquire) {
            return false;
        }
        self.start_screencast(link, event_tx, frame_slot);
        if self.stop_requested.load(Ordering::Acquire) {
            return false;
        }
        self.write_manifest(true);
        let capabilities = crate::ActiveBackendCapabilities {
            backend: crate::BackendKind::ChromiumCdp,
            capabilities: crate::BackendKind::ChromiumCdp.capabilities(),
            bidi: false,
            automation_disclosure: self
                .config
                .browser
                .automation_disclosure
                .ready_status(crate::BackendKind::ChromiumCdp),
        };
        frame_slot.publish_backend_capabilities(capabilities);
        let _ = event_tx.send(BrowserEvent::BackendReady(capabilities));
        let _ = event_tx.send(BrowserEvent::Ready);
        !self.stop_requested.load(Ordering::Acquire)
    }

    fn resolve_main_frame_id(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        session: &str,
    ) -> bool {
        let Some(frame_tree) = self.setup_command_result(
            link,
            event_tx,
            frame_slot,
            "Page.getFrameTree",
            &serde_json::json!({}),
            Some(session),
        ) else {
            return false;
        };
        let Some(frame_id) = main_frame_id_from_tree(&frame_tree).map(str::to_string) else {
            return self.setup_failure(
                event_tx,
                frame_slot,
                "Page.getFrameTree",
                "response omitted the top-level frame id",
            );
        };
        self.main_frame_id = Some(frame_id);
        true
    }

    fn navigate_initial_url(&mut self, link: &mut CdpLink, event_tx: &BrowserEventSender, frame_slot: &Arc<FrameSlot>) {
        if self.initial_navigated {
            return;
        }
        self.initial_navigated = true;
        let initial_url = self.config.initial_url.clone();
        if let Some(initial) = initial_url
            && initial != "about:blank"
        {
            let _ = self.navigate_to(link, event_tx, frame_slot, &initial);
        }
    }

    fn install_common_signal_minimization(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        session: &str,
    ) -> bool {
        let Some(version) = self.setup_command_result(
            link,
            event_tx,
            frame_slot,
            "Browser.getVersion",
            &serde_json::json!({}),
            None,
        ) else {
            return false;
        };
        let needs_user_agent_override = match chromium_user_agent_needs_override(&version) {
            Ok(value) => value,
            Err(error) => return self.setup_failure(event_tx, frame_slot, "Browser.getVersion", error),
        };
        let user_agent_metadata = if needs_user_agent_override {
            let Some(metadata) = self.native_user_agent_metadata.as_ref() else {
                return self.setup_failure(
                    event_tx,
                    frame_slot,
                    "Browser.getVersion",
                    "native Chromium userAgentData metadata was not captured during startup",
                );
            };
            metadata
        } else {
            &serde_json::Value::Null
        };
        let user_agent_override = match chromium_user_agent_override(&version, user_agent_metadata) {
            Ok(value) => value,
            Err(error) => return self.setup_failure(event_tx, frame_slot, "Emulation.setUserAgentOverride", error),
        };
        if let Some(params) = user_agent_override
            && !self.setup_command(
                link,
                event_tx,
                frame_slot,
                "Emulation.setUserAgentOverride",
                &params,
                Some(session),
            )
        {
            return false;
        }
        true
    }

    fn setup_command(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        method: &str,
        params: &serde_json::Value,
        session: Option<&str>,
    ) -> bool {
        self.setup_command_result(link, event_tx, frame_slot, method, params, session)
            .is_some()
    }

    fn setup_command_result(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        method: &str,
        params: &serde_json::Value,
        session: Option<&str>,
    ) -> Option<serde_json::Value> {
        match self.call_and_ack(link, event_tx, frame_slot, method, params, session) {
            Ok(result) => (!self.stop_requested.load(Ordering::Acquire)).then_some(result),
            Err(_) if self.stop_requested.load(Ordering::Acquire) => None,
            Err(error) => {
                self.setup_failure(event_tx, frame_slot, method, &error.to_string());
                None
            }
        }
    }

    fn setup_failure(
        &mut self,
        event_tx: &BrowserEventSender,
        frame_slot: &FrameSlot,
        method: &str,
        error: &str,
    ) -> bool {
        self.session_id = None;
        self.screencast_on = false;
        self.invalidate_scrollbar_layout();
        self.reset_clipboard_tracking();
        self.pending_reattach = self.target_id.is_some();
        frame_slot.clear();
        let message = format!("browser setup failed at {method}: {error}; retry to restart");
        tracing::warn!(target: "browser", "{message}");
        let _ = event_tx.send(BrowserEvent::Warning(message));
        false
    }

    fn start_screencast(&mut self, link: &mut CdpLink, event_tx: &BrowserEventSender, frame_slot: &Arc<FrameSlot>) {
        let Some(session) = self.session_id.clone() else {
            return;
        };
        let params = self.screencast_params();
        match self.call_and_ack(
            link,
            event_tx,
            frame_slot,
            "Page.startScreencast",
            &params,
            Some(session.as_str()),
        ) {
            Ok(_) => {
                self.screencast_on = true;
                self.restart_attempts = 0;
                self.pending_restart_at = None;
            }
            Err(error) => self.note_screencast_failure(&error.to_string()),
        }
    }

    /// Exponential backoff between screencast restarts, capped so a page
    /// whose screencast is genuinely dead still re-attaches in a bounded
    /// time. A navigation's brief screencast outage must not burn through
    /// all restart attempts (that forces a full re-attach mid-load).
    fn restart_backoff(&self) -> Duration {
        let shift = self.restart_attempts.saturating_sub(1).min(4);
        let millis = RESTART_BACKOFF
            .as_millis()
            .saturating_mul(1u128 << shift)
            .min(RESTART_BACKOFF_CAP.as_millis());
        // The value is capped by `RESTART_BACKOFF_CAP`, far below u64::MAX.
        Duration::from_millis(u64::try_from(millis).unwrap_or(u64::MAX))
    }

    fn note_screencast_failure(&mut self, message: &str) {
        self.screencast_on = false;
        self.restart_attempts += 1;
        tracing::debug!(
            target: "browser",
            "screencast start rejected (attempt {}): {message}",
            self.restart_attempts
        );
        if self.restart_attempts >= MAX_RESTART_ATTEMPTS {
            // The session's page binding is probably stale (another CDP
            // client navigated away from under us). Force a re-attach.
            self.restart_attempts = 0;
            self.pending_reattach = true;
        } else {
            self.pending_restart_at = Some(Instant::now() + self.restart_backoff());
        }
    }

    pub(super) fn note_reattach_failure(&mut self, event_tx: &BrowserEventSender, message: &str) {
        self.reattach_failures += 1;
        if self.reattach_failures >= 5 {
            self.reattach_failures = 0;
            self.pending_reattach = false;
            self.pending_restart_at = None;
            let _ = event_tx.send(BrowserEvent::Warning(format!(
                "could not re-attach to the page: {message}; retry to restart"
            )));
        } else {
            self.pending_reattach = true;
            self.pending_restart_at = Some(Instant::now() + self.restart_backoff());
        }
    }

    pub(super) fn pending_restart_tick(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) {
        if self.pending_reattach && !self.reattach_in_flight {
            // A rejected re-attach parked a backoff delay first.
            if let Some(due) = self.pending_restart_at {
                if Instant::now() < due {
                    return;
                }
                self.pending_restart_at = None;
            }
            self.pending_reattach = false;
            if let Some(target) = self.target_id.clone() {
                self.reattach_in_flight = true;
                self.session_id = None;
                self.invalidate_scrollbar_layout();
                match link.send_request(
                    "Target.attachToTarget",
                    &serde_json::json!({ "targetId": target, "flatten": true }),
                    None,
                ) {
                    Ok(request_id) => self.reattach_request_id = Some(request_id),
                    Err(error) => {
                        tracing::warn!(target: "browser", "re-attach request failed: {error}");
                        self.reattach_in_flight = false;
                        self.reattach_request_id = None;
                        self.note_reattach_failure(event_tx, &error.to_string());
                    }
                }
            }
            return;
        }
        let Some(due) = self.pending_restart_at else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        self.pending_restart_at = None;
        if self.session_id.is_some() {
            self.start_screencast(link, event_tx, frame_slot);
        }
    }
}

fn main_frame_id_from_tree(frame_tree: &serde_json::Value) -> Option<&str> {
    frame_tree.pointer("/frameTree/frame/id")?.as_str()
}

#[cfg(test)]
mod tests {
    use super::main_frame_id_from_tree;

    #[test]
    fn frame_tree_requires_a_top_level_frame_id() {
        let frame_tree = serde_json::json!({
            "frameTree": {
                "frame": { "id": "main", "url": "about:blank" },
                "childFrames": [{ "frame": { "id": "child" } }]
            }
        });

        assert_eq!(main_frame_id_from_tree(&frame_tree), Some("main"));
        assert_eq!(main_frame_id_from_tree(&serde_json::json!({ "frameTree": {} })), None);
    }
}
