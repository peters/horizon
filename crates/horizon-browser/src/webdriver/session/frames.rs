//! Adaptive screenshot polling for classic `WebDriver` backends: capture
//! cadence, change detection, and the page-scroll sample that follows a frame.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::PageScrollState;
use crate::frames::FrameSlot;
use crate::session::{BrowserEventSender, publish_frame};

use super::{Driver, webdriver_value};

pub(super) const ACTIVE_FRAME_INTERVAL: Duration = Duration::from_millis(33);
pub(super) const ACTIVE_WINDOW: Duration = Duration::from_millis(900);
pub(super) const STATIC_CONFIRMATIONS: u8 = 3;
pub(super) const SCROLL_STATE_INTERVAL: Duration = Duration::from_millis(100);
pub(super) const PAGE_SCROLL_STATE_SCRIPT: &str = "const root = document.scrollingElement || document.documentElement; return { scroll_x: window.scrollX, scroll_y: window.scrollY, viewport_width: window.innerWidth, viewport_height: window.innerHeight, client_width: document.documentElement.clientWidth, client_height: document.documentElement.clientHeight, content_width: root.scrollWidth, content_height: root.scrollHeight };";

pub(super) struct AdaptiveFrames {
    pub(super) next_capture: Option<Instant>,
    pub(super) active_until: Instant,
    pub(super) last_hash: Option<u64>,
    pub(super) unchanged: u8,
    pub(super) interaction_started_at: Option<Instant>,
}

impl AdaptiveFrames {
    pub(super) fn new() -> Self {
        let now = Instant::now();
        Self {
            next_capture: Some(now),
            active_until: now + ACTIVE_WINDOW,
            last_hash: None,
            unchanged: 0,
            interaction_started_at: None,
        }
    }

    pub(super) fn demand(&mut self) {
        let now = Instant::now();
        self.active_until = now + ACTIVE_WINDOW;
        self.next_capture = Some(now);
        self.unchanged = 0;
        self.interaction_started_at.get_or_insert(now);
    }

    pub(super) fn invalidate(&mut self) {
        self.last_hash = None;
        self.demand();
    }

    pub(super) fn suspend_for_navigation(&mut self) {
        let now = Instant::now();
        self.last_hash = None;
        self.unchanged = 0;
        self.active_until = now + ACTIVE_WINDOW;
        self.next_capture = None;
        self.interaction_started_at.get_or_insert(now);
    }

    pub(super) fn due(&self, now: Instant) -> bool {
        self.next_capture.is_some_and(|next| now >= next)
    }

    pub(super) fn completed(&mut self, encoded: &str) -> bool {
        let mut hasher = DefaultHasher::new();
        encoded.hash(&mut hasher);
        let hash = hasher.finish();
        let changed = self.last_hash != Some(hash);
        self.last_hash = Some(hash);
        let now = Instant::now();
        if changed {
            self.unchanged = 0;
            self.active_until = now + ACTIVE_WINDOW;
        } else {
            self.unchanged = self.unchanged.saturating_add(1);
        }
        self.next_capture = if now < self.active_until || self.unchanged < STATIC_CONFIRMATIONS {
            Some(now + ACTIVE_FRAME_INTERVAL)
        } else {
            None
        };
        changed
    }

    pub(super) fn failed(&mut self) {
        self.next_capture = Some(Instant::now() + Duration::from_millis(250));
    }

    pub(super) fn published(&mut self) -> Option<Duration> {
        self.interaction_started_at.take().map(|started| started.elapsed())
    }
}

impl Driver {
    pub(super) fn capture_frame(&mut self, frame_slot: &FrameSlot, event_tx: &BrowserEventSender) {
        self.capture_frame_until(frame_slot, event_tx, None);
    }

    pub(super) fn capture_frame_until(
        &mut self,
        frame_slot: &FrameSlot,
        event_tx: &BrowserEventSender,
        deadline: Option<Instant>,
    ) {
        frame_slot.record_capture_request();
        let generation = self.generation;
        let context_id = self.context_id.clone();
        let response = match deadline {
            Some(deadline) => match super::super::orientation::remaining(deadline) {
                Ok(timeout) => self
                    .host
                    .transport()
                    .get_with_read_timeout(&self.session_path("screenshot"), timeout.min(Duration::from_secs(3)))
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error.message),
            },
            None => self.classic_get("screenshot"),
        };
        let result = response.and_then(|response| {
            webdriver_value(&response)
                .and_then(Value::as_str)
                .map(|data| (data.to_string(), false))
                .ok_or_else(|| "WebDriver screenshot response had no data".to_string())
        });
        let (encoded, jpeg) = match result {
            Ok(frame) => {
                frame_slot.record_capture_completion();
                frame
            }
            Err(error) => {
                frame_slot.record_capture_failure();
                tracing::warn!("adaptive browser screenshot failed: {error}");
                self.frames.failed();
                return;
            }
        };
        if !capture_is_current(
            generation,
            self.generation,
            context_id.as_deref(),
            self.context_id.as_deref(),
        ) {
            frame_slot.record_capture_superseded();
            self.frames.invalidate();
            return;
        }
        let frame_changed = self.frames.completed(&encoded);
        if frame_changed {
            let seq = if jpeg {
                frame_slot.store_base64_jpeg(&encoded)
            } else {
                frame_slot.store_base64_png(&encoded)
            };
            if let Some(seq) = seq {
                if let Some(elapsed) = self.frames.published() {
                    frame_slot.record_interaction_to_frame(elapsed);
                }
                publish_frame(event_tx, frame_slot, seq);
            } else {
                tracing::warn!("browser screenshot decode failed; retaining previous frame");
            }
        } else {
            frame_slot.record_unchanged_frame();
        }
        let refreshed = match deadline {
            Some(deadline) => self.refresh_page_scroll_state_until(frame_slot, Some(deadline)),
            None => self.refresh_page_scroll_state(frame_slot),
        };
        if refreshed {
            // A page can scroll over visually identical pixels. Wake the host
            // even when the screenshot hash did not change so a scrollbar
            // overlay still follows the browser's authoritative position.
            event_tx.wake_ui();
        }
    }

    pub(super) fn refresh_page_scroll_state(&mut self, frame_slot: &FrameSlot) -> bool {
        self.refresh_page_scroll_state_until(frame_slot, None)
    }

    fn refresh_page_scroll_state_until(&mut self, frame_slot: &FrameSlot, deadline: Option<Instant>) -> bool {
        let now = Instant::now();
        if now < self.scrollbar.refresh_at {
            return false;
        }
        self.scrollbar.refresh_at = now + SCROLL_STATE_INTERVAL;
        let body = json!({ "script": PAGE_SCROLL_STATE_SCRIPT, "args": [] });
        let response = match deadline {
            Some(deadline) => match super::super::orientation::remaining(deadline) {
                Ok(timeout) => {
                    self.classic_navigation_post_within("execute/sync", &body, timeout.min(Duration::from_secs(3)))
                }
                Err(error) => Err(error.message),
            },
            None => self.classic_post("execute/sync", &body),
        };
        let Ok(response) = response else {
            return self.scrollbar.clear_sampled(frame_slot);
        };
        let Some(value) = webdriver_value(&response).cloned() else {
            return self.scrollbar.clear_sampled(frame_slot);
        };
        let Ok(state) = serde_json::from_value::<PageScrollState>(value) else {
            return self.scrollbar.clear_sampled(frame_slot);
        };
        self.scrollbar.sample(state);
        frame_slot.publish_page_scroll_state(state)
    }
}

pub(super) fn capture_is_current(
    capture_generation: u64,
    current_generation: u64,
    capture_context: Option<&str>,
    current_context: Option<&str>,
) -> bool {
    capture_generation == current_generation && capture_context == current_context
}
