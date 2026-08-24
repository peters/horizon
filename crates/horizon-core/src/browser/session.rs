//! The per-panel browser driver thread.
//!
//! One thread per browser panel, mirroring how terminal panels run their
//! alacritty event loops: it owns the Chrome process and the browser-level
//! CDP connection, decodes screencast frames into a shared
//! [`FrameSlot`], and reports lightweight [`BrowserEvent`]s over an mpsc
//! channel that the main thread drains each frame. Frame wake-ups are
//! coalesced to one outstanding event while the slot keeps only the newest
//! image. Input and navigation commands travel the other way over
//! [`BrowserCommand`].
//!
//! CDP behaviors encoded here (validated against real Chrome in the L0
//! spike):
//! - flattened sessions: `sessionId` is a string; events carry it at the
//!   top level; commands need it at the top level, and
//!   `Page.screencastFrameAck` needs it in `params` as well.
//! - screencasts are **change-driven**: frames only arrive when the page
//!   repaints, so a static page costs nothing.
//! - a cross-document navigation — especially one triggered by *another*
//!   CDP client (e.g. agent CDP client) — temporarily breaks the
//!   session's page binding and returns "Not attached to an active page".
//!   Recovery is a backoff-retried `Page.startScreencast` (~100 ms steps),
//!   with a full re-attach as the last resort.
//! - the screencast is implicitly terminated by navigation, so it is
//!   restarted on every top-level `Page.frameNavigated`.

mod commands;
mod events;
mod lifecycle;
mod manifest_io;
mod startup;

pub(super) use startup::profile_dir;
use startup::run_driver;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::browser::cdp::{CdpError, CdpLink};
use crate::browser::frames::FrameSlot;
use crate::browser::process::ChromeProcess;
use crate::browser::{BrowserConfig, BrowserInput};

/// What the driver reports to the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum BrowserEvent {
    /// Chrome is up and the page session is attached.
    Ready,
    Title(String),
    UrlChanged(String),
    NavigationFailed(String),
    Loading(bool),
    /// A new decoded frame is available in the panel's frame slot. The
    /// sequence belongs to the frame that claimed the coalesced wake-up;
    /// callers must read the slot for the newest frame.
    Frame {
        seq: u64,
    },
    /// Non-fatal problem surfaced to the panel body.
    Warning(String),
    /// The driver stopped.
    Stopped {
        code: Option<i32>,
    },
    /// An agent asked the user for the wheel.
    HandoffRequested(String),
    /// The handoff was resolved (user handed back or agent cancelled).
    HandoffCleared,
    /// A requested handoff acknowledgement could not be persisted.
    HandoffResolutionFailed(String),
    /// Selected page text copied by headless Chrome, ready for the host
    /// clipboard bridge in the UI.
    ClipboardText(String),
    /// The agent owning this panel changed (`None` = no live owner).
    OwnerChanged(Option<String>),
}

/// What the panel/UI asks the driver to do.
#[derive(Clone, Debug)]
pub enum BrowserCommand {
    Navigate(String),
    Reload,
    Back,
    Forward,
    SetViewport {
        width: u32,
        height: u32,
    },
    Input(BrowserInput),
    /// The user clicked "hand back to agent" in the panel.
    HandoffDone,
    Stop,
}

/// Everything the driver needs to start.
#[derive(Clone, Debug)]
pub struct BrowserSessionConfig {
    pub browser: BrowserConfig,
    pub panel_local_id: String,
    pub initial_url: Option<String>,
    pub width: u32,
    pub height: u32,
    /// The panel's frame slot, shared across driver (re)starts so frame
    /// sequence numbers stay monotonic across sessions — a retry must not
    /// re-emit sequence 1 and be mistaken for an unchanged frame.
    pub frame_slot: Arc<FrameSlot>,
}

/// The panel-side handle to a running driver.
pub struct BrowserSession {
    command_tx: mpsc::Sender<BrowserCommand>,
    stop_requested: Arc<AtomicBool>,
    pub frame_slot: Arc<FrameSlot>,
    pub event_rx: mpsc::Receiver<BrowserEvent>,
    /// Resolved when the driver thread has finished tearing down Chrome.
    completion_rx: mpsc::Receiver<()>,
    event_wake: BrowserEventWake,
}

/// UI callback invoked after the driver queues an event. Keeping this generic
/// avoids coupling core browser lifecycle code to egui while still allowing
/// the native event loop to wake immediately from its idle backoff.
pub type BrowserEventWaker = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Clone, Default)]
struct BrowserEventWake {
    callback: Arc<Mutex<Option<BrowserEventWaker>>>,
}

impl BrowserEventWake {
    fn is_set(&self) -> bool {
        self.callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    fn set(&self, callback: BrowserEventWaker) {
        *self.callback.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(callback);
    }

    fn wake(&self) {
        let callback = self
            .callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(callback) = callback {
            callback();
        }
    }
}

struct BrowserEventSender {
    tx: mpsc::Sender<BrowserEvent>,
    wake: BrowserEventWake,
}

impl BrowserEventSender {
    fn send(&self, event: BrowserEvent) -> Result<(), mpsc::SendError<BrowserEvent>> {
        self.tx.send(event)?;
        self.wake.wake();
        Ok(())
    }
}

impl BrowserSession {
    #[must_use]
    pub fn send(&self, command: BrowserCommand) -> bool {
        if matches!(command, BrowserCommand::Stop) {
            self.stop_requested.store(true, Ordering::Release);
        }
        self.command_tx.send(command).is_ok()
    }

    pub fn set_event_waker(&self, callback: BrowserEventWaker) {
        self.event_wake.set(callback);
    }

    #[must_use]
    pub fn needs_event_waker(&self) -> bool {
        !self.event_wake.is_set()
    }

    /// Send `Stop` and return the teardown-completion signal. The receiver
    /// resolves once the driver has closed the `DevTools` connection, killed
    /// Chrome, and removed the manifest.
    #[must_use]
    pub fn shutdown_signal(self) -> mpsc::Receiver<()> {
        self.stop_requested.store(true, Ordering::Release);
        let _ = self.command_tx.send(BrowserCommand::Stop);
        self.frame_slot.release_notification();
        self.completion_rx
    }

    /// Return the existing teardown-completion signal after the driver has
    /// already announced `Stopped`; no second stop request is necessary.
    #[must_use]
    pub fn completion_signal(self) -> mpsc::Receiver<()> {
        self.frame_slot.release_notification();
        self.completion_rx
    }
}

fn publish_frame(event_tx: &BrowserEventSender, frame_slot: &FrameSlot, seq: u64) {
    if frame_slot.claim_notification() && event_tx.send(BrowserEvent::Frame { seq }).is_err() {
        frame_slot.release_notification();
    }
}

const WS_URL_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const RESTART_BACKOFF: Duration = Duration::from_millis(250);
const RESTART_BACKOFF_CAP: Duration = Duration::from_secs(1);
const MAX_RESTART_ATTEMPTS: u32 = 8;
const MANIFEST_MIN_INTERVAL: Duration = Duration::from_millis(200);
const SIGNAL_MIN_INTERVAL: Duration = Duration::from_millis(250);
/// Pointer-move storms rewrite the manifest on every event otherwise; the
/// 5 s user-active TTL tolerates a 1 s refresh.
const USER_ACTIVE_STAMP_INTERVAL: Duration = Duration::from_secs(1);
/// Wait until a resize gesture settles before forcing one current-viewport
/// screenshot. Chrome can update device metrics without emitting a
/// screencast frame, which otherwise leaves the previous frame stretched.
const VIEWPORT_CAPTURE_DELAY: Duration = Duration::from_millis(75);
const TITLE_BINDING_NAME: &str = "__horizonBrowserTitleChanged";
const TITLE_OBSERVER_SCRIPT: &str = r"(() => {
    if (window !== top || window.__horizonBrowserTitleObserver) return;
    let lastTitle;
    const publish = () => {
        const title = document.title;
        if (title === lastTitle) return;
        lastTitle = title;
        window.__horizonBrowserTitleChanged(JSON.stringify({ title, href: location.href }));
    };
    const install = () => {
        if (window.__horizonBrowserTitleObserver) return;
        const root = document.head || document.documentElement;
        if (!root) return;
        const observer = new MutationObserver(publish);
        observer.observe(root, { childList: true, characterData: true, subtree: true });
        window.__horizonBrowserTitleObserver = observer;
        publish();
    };
    if (document.documentElement) install();
    else addEventListener('DOMContentLoaded', install, { once: true });
})()";

/// Spawn the driver thread. Browser startup problems are reported through
/// the event channel rather than failing here.
///
/// # Errors
/// Fails only when the OS thread cannot be spawned.
pub fn start_session(config: BrowserSessionConfig) -> Result<BrowserSession, String> {
    let frame_slot = config.frame_slot.clone();
    let (command_tx, command_rx) = mpsc::channel::<BrowserCommand>();
    let (raw_event_tx, event_rx) = mpsc::channel::<BrowserEvent>();
    let event_wake = BrowserEventWake::default();
    let event_tx = BrowserEventSender {
        tx: raw_event_tx,
        wake: event_wake.clone(),
    };
    let (completion_tx, completion_rx) = mpsc::channel::<()>();
    let stop_requested = Arc::new(AtomicBool::new(false));
    let driver_stop_requested = Arc::clone(&stop_requested);
    let slot = Arc::clone(&frame_slot);
    std::thread::Builder::new()
        .name("browser-driver".into())
        .spawn(move || {
            run_driver(
                &config,
                &event_tx,
                &command_rx,
                &slot,
                &driver_stop_requested,
                completion_tx,
            );
        })
        .map_err(|e| format!("failed to spawn browser driver: {e}"))?;
    Ok(BrowserSession {
        command_tx,
        stop_requested,
        frame_slot,
        event_rx,
        completion_rx,
        event_wake,
    })
}

fn run_loop(
    state: &mut DriverState,
    chrome: &mut ChromeProcess,
    link: &mut CdpLink,
    command_rx: &mpsc::Receiver<BrowserCommand>,
    frame_slot: &Arc<FrameSlot>,
    event_tx: &BrowserEventSender,
) {
    loop {
        // 1. Commands from the UI.
        if state.drain_commands(link, command_rx, event_tx, frame_slot) {
            // Ask Chrome to exit cleanly so it marks its profile session
            // complete (kill alone leaves a "crashed" state that makes the
            // next launch restore stale tabs); the kill below is the
            // fallback for an uncooperative process.
            let _ = link.call_and_drain(Duration::from_secs(1), "Browser.close", &serde_json::json!({}), None);
            chrome.kill();
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            break;
        }

        // 2. CDP messages: responses and events.
        match link.read_one() {
            Ok(Some(message)) => state.handle_message(link, event_tx, frame_slot, message),
            Ok(None) => {}
            Err(error) => {
                // The connection is gone. Reconnecting a single panel's
                // session is not worth the state surgery; surface the
                // failure and stop — the panel offers Retry.
                tracing::warn!(target: "browser", "cdp connection lost: {error}");
                let _ = event_tx.send(BrowserEvent::Warning(format!("CDP connection lost: {error}")));
                chrome.kill();
                let _ = event_tx.send(BrowserEvent::Stopped { code: None });
                break;
            }
        }

        // 3. Flush a pending throttled manifest write (the loop always
        //    iterates, so a quiet page still gets its url/title flushed).
        state.write_manifest(false);

        // 3b. Delayed post-load title fetch.
        state.tick_title_fetch(link, event_tx, frame_slot);

        // 3c. One fresh frame after a viewport-resize burst settles.
        state.tick_viewport_capture(link);

        // 4. Backoff-retried screencast restart / re-attach.
        state.pending_restart_tick(link, event_tx, frame_slot);

        // 5. Ownership / handoff signals from the manifest (agent side).
        state.tick_signals(event_tx);

        // 6. Chrome process liveness.
        if let Some(status) = chrome.child_status() {
            let _ = event_tx.send(BrowserEvent::Stopped { code: status.code() });
            break;
        }

        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Driver-side state machine for one page session.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // intentional: per-concern state flags
struct DriverState {
    config: BrowserSessionConfig,
    browser_ws: String,
    session_id: Option<String>,
    target_id: Option<String>,
    main_frame_id: Option<String>,
    viewport_w: u32,
    viewport_h: u32,
    pending_viewport_capture_at: Option<Instant>,
    viewport_capture_request_id: Option<u64>,
    clipboard_read_request_ids: std::collections::HashSet<u64>,
    url: String,
    title: String,
    initial_navigated: bool,
    screencast_on: bool,
    pending_restart_at: Option<Instant>,
    /// Default execution context of the current document. After a
    /// navigation, `Runtime.evaluate` without an explicit `contextId` can
    /// still hit the old document's context for a while, so the title
    /// fetch targets the latest default context explicitly.
    title_context_id: Option<u64>,
    /// `executionContextCreated` for the new document can land after
    /// `loadEventFired`, so the post-load fetch is slightly delayed to let
    /// the context tracking catch up first.
    title_fetch_at: Option<Instant>,
    /// Retries for a title fetch that still evaluated in the previous
    /// document's execution context (detected via `location.href`).
    title_fetch_retries: u32,
    restart_attempts: u32,
    pending_reattach: bool,
    reattach_in_flight: bool,
    reattach_request_id: Option<u64>,
    /// Consecutive rejected re-attach attempts; surfaced as a retryable
    /// error once re-attach clearly cannot succeed (dead target).
    reattach_failures: u32,
    last_manifest_write: Instant,
    manifest_dirty: bool,
    last_signal_check: Instant,
    last_user_active_stamp: Option<Instant>,
    owner_seen: Option<String>,
    handoff_seen: Option<i64>,
    stop_requested: Arc<AtomicBool>,
}

impl DriverState {
    fn new(config: &BrowserSessionConfig, browser_ws: &str, stop_requested: Arc<AtomicBool>) -> Self {
        let url = config
            .initial_url
            .clone()
            .filter(|u| u != "about:blank")
            .unwrap_or_else(|| "about:blank".to_string());
        Self {
            config: config.clone(),
            browser_ws: browser_ws.to_string(),
            session_id: None,
            target_id: None,
            main_frame_id: None,
            viewport_w: config.width,
            viewport_h: config.height,
            pending_viewport_capture_at: None,
            viewport_capture_request_id: None,
            clipboard_read_request_ids: std::collections::HashSet::new(),
            url,
            title: String::new(),
            initial_navigated: false,
            screencast_on: false,
            pending_restart_at: None,
            title_context_id: None,
            title_fetch_at: None,
            title_fetch_retries: 0,
            restart_attempts: 0,
            pending_reattach: false,
            reattach_in_flight: false,
            reattach_request_id: None,
            reattach_failures: 0,
            last_manifest_write: Instant::now(),
            manifest_dirty: true,
            last_signal_check: Instant::now(),
            last_user_active_stamp: None,
            owner_seen: None,
            handoff_seen: None,
            stop_requested,
        }
    }

    /// `call_and_drain` plus full routing for everything drained while the
    /// roundtrip is in flight: a `Page.screencastFrame` without an ack
    /// stops the stream, dropping other drained events (title, load,
    /// navigation commits) loses state Chrome already committed, and a
    /// drained *response* may belong to another in-flight request (notably
    /// a re-attach roundtrip) whose state machine would otherwise never
    /// hear back.
    fn call_and_ack(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        method: &str,
        params: &serde_json::Value,
        session: Option<&str>,
    ) -> Result<serde_json::Value, crate::browser::cdp::CdpError> {
        let stop_requested = Arc::clone(&self.stop_requested);
        let outcome = link.call_and_drain_until(CALL_TIMEOUT, method, params, session, || {
            stop_requested.load(Ordering::Acquire)
        });
        for message in outcome.drained {
            self.handle_message(link, event_tx, frame_slot, message);
        }
        outcome.result
    }

    fn send_page_command(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, CdpError> {
        let Some(session) = self.session_id.clone() else {
            return Err(CdpError::NoPageSession {
                method: method.to_string(),
            });
        };
        self.call_and_ack(link, event_tx, frame_slot, method, params, Some(session.as_str()))
    }

    fn navigate_to(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        url: &str,
    ) {
        let result = self.send_page_command(
            link,
            event_tx,
            frame_slot,
            "Page.navigate",
            &serde_json::json!({ "url": url }),
        );
        match result {
            Ok(result) => {
                if let Some(error) = result.get("errorText").and_then(serde_json::Value::as_str)
                    && !error.is_empty()
                {
                    let _ = event_tx.send(BrowserEvent::NavigationFailed(format!(
                        "could not navigate to {url}: {error}"
                    )));
                    return;
                }
                // The committed URL remains authoritative and arrives via a
                // navigation event. Do not overwrite it with a merely
                // requested value.
                self.pending_restart_at = Some(Instant::now());
                let _ = event_tx.send(BrowserEvent::Loading(true));
            }
            Err(error) => {
                let _ = event_tx.send(BrowserEvent::NavigationFailed(format!(
                    "could not navigate to {url}: {error}"
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn queued_browser_events_wake_the_registered_ui_callback() {
        let wake_count = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&wake_count);
        let wake = BrowserEventWake::default();
        wake.set(Arc::new(move || {
            counted.fetch_add(1, Ordering::Relaxed);
        }));
        let (tx, rx) = mpsc::channel();
        let sender = BrowserEventSender { tx, wake };

        assert!(sender.send(BrowserEvent::Ready).is_ok());
        assert_eq!(rx.recv(), Ok(BrowserEvent::Ready));
        assert_eq!(wake_count.load(Ordering::Relaxed), 1);
    }
}
