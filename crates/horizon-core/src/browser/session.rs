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
mod manifest_io;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
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
}

impl BrowserSession {
    #[must_use]
    pub fn send(&self, command: BrowserCommand) -> bool {
        if matches!(command, BrowserCommand::Stop) {
            self.stop_requested.store(true, Ordering::Release);
        }
        self.command_tx.send(command).is_ok()
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

fn publish_frame(event_tx: &mpsc::Sender<BrowserEvent>, frame_slot: &FrameSlot, seq: u64) {
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

/// Spawn the driver thread. Browser startup problems are reported through
/// the event channel rather than failing here.
///
/// # Errors
/// Fails only when the OS thread cannot be spawned.
pub fn start_session(config: BrowserSessionConfig) -> Result<BrowserSession, String> {
    let frame_slot = config.frame_slot.clone();
    let (command_tx, command_rx) = mpsc::channel::<BrowserCommand>();
    let (event_tx, event_rx) = mpsc::channel::<BrowserEvent>();
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
    })
}

/// Resolves the driver's teardown signal exactly once, on thread exit.
struct DriverCompletion(Option<mpsc::Sender<()>>);

impl Drop for DriverCompletion {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

fn cancel_startup_if_requested(
    requested: &AtomicBool,
    chrome: &mut ChromeProcess,
    event_tx: &mpsc::Sender<BrowserEvent>,
) -> bool {
    if !requested.load(Ordering::Acquire) {
        return false;
    }
    chrome.kill();
    let _ = event_tx.send(BrowserEvent::Stopped { code: None });
    true
}

struct DriverConnection {
    chrome: ChromeProcess,
    link: CdpLink,
    ws_url: String,
    target_id: String,
    session_id: String,
}

fn initialize_driver(
    config: &BrowserSessionConfig,
    event_tx: &mpsc::Sender<BrowserEvent>,
    stop_requested: &AtomicBool,
) -> Option<DriverConnection> {
    let launch = match build_launch(config) {
        Ok(launch) => launch,
        Err(message) => {
            let _ = event_tx.send(BrowserEvent::Warning(message));
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return None;
        }
    };
    if stop_requested.load(Ordering::Acquire) {
        let _ = event_tx.send(BrowserEvent::Stopped { code: None });
        return None;
    }
    let mut chrome = match ChromeProcess::spawn(&launch) {
        Ok(chrome) => chrome,
        Err(error) => {
            let _ = event_tx.send(BrowserEvent::Warning(format!("failed to start chrome: {error}")));
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return None;
        }
    };
    let ws_url = match chrome.wait_ws_url(WS_URL_TIMEOUT, || stop_requested.load(Ordering::Acquire)) {
        Ok(Some(url)) => url,
        Ok(None) => {
            chrome.kill();
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return None;
        }
        Err(error) => {
            let _ = event_tx.send(BrowserEvent::Warning(format!("no DevTools endpoint: {error}")));
            chrome.kill();
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return None;
        }
    };
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    let link = match CdpLink::connect(&ws_url) {
        Ok(link) => link,
        Err(error) => {
            let _ = event_tx.send(BrowserEvent::Warning(format!("CDP connect failed: {error}")));
            chrome.kill();
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return None;
        }
    };
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    initialize_target(config, event_tx, stop_requested, chrome, link, ws_url)
}

fn initialize_target(
    config: &BrowserSessionConfig,
    event_tx: &mpsc::Sender<BrowserEvent>,
    stop_requested: &AtomicBool,
    mut chrome: ChromeProcess,
    mut link: CdpLink,
    ws_url: String,
) -> Option<DriverConnection> {
    let _ = link.call_and_drain(
        CALL_TIMEOUT,
        "Target.setDiscoverTargets",
        &serde_json::json!({ "discover": true }),
        None,
    );
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    let _ = link.call_and_drain(
        CALL_TIMEOUT,
        "Target.setAutoAttach",
        &serde_json::json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
        None,
    );
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    // setAutoAttach only covers *future* targets: attach explicitly to the
    // page that Chrome opened at startup (creating one if it has none).
    let Some(target_id) = first_page_target(&mut link).or_else(|| create_page_target(&mut link, config)) else {
        let _ = event_tx.send(BrowserEvent::Warning("no page target found".to_string()));
        chrome.kill();
        let _ = event_tx.send(BrowserEvent::Stopped { code: None });
        return None;
    };
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    let Some(session_id) = link
        .call_and_drain(
            CALL_TIMEOUT,
            "Target.attachToTarget",
            &serde_json::json!({ "targetId": target_id, "flatten": true }),
            None,
        )
        .result
        .ok()
        .and_then(|result| result.get("sessionId").and_then(|s| s.as_str()).map(str::to_string))
    else {
        let _ = event_tx.send(BrowserEvent::Warning("initial attach failed".to_string()));
        chrome.kill();
        let _ = event_tx.send(BrowserEvent::Stopped { code: None });
        return None;
    };
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    Some(DriverConnection {
        chrome,
        link,
        ws_url,
        target_id,
        session_id,
    })
}

fn run_driver(
    config: &BrowserSessionConfig,
    event_tx: &mpsc::Sender<BrowserEvent>,
    command_rx: &mpsc::Receiver<BrowserCommand>,
    frame_slot: &Arc<FrameSlot>,
    stop_requested: &AtomicBool,
    completion_tx: mpsc::Sender<()>,
) {
    let _completion = DriverCompletion(Some(completion_tx));
    let Some(mut connection) = initialize_driver(config, event_tx, stop_requested) else {
        return;
    };

    let mut state = DriverState::new(config, &connection.ws_url);
    state.initialize_manifest();
    state.attach_setup(
        &mut connection.link,
        event_tx,
        frame_slot,
        &connection.session_id,
        &connection.target_id,
    );

    run_loop(
        &mut state,
        &mut connection.chrome,
        &mut connection.link,
        command_rx,
        frame_slot,
        event_tx,
    );
    connection.chrome.kill();
}

fn run_loop(
    state: &mut DriverState,
    chrome: &mut ChromeProcess,
    link: &mut CdpLink,
    command_rx: &mpsc::Receiver<BrowserCommand>,
    frame_slot: &Arc<FrameSlot>,
    event_tx: &mpsc::Sender<BrowserEvent>,
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
            state.remove_manifest();
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
                state.remove_manifest();
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
            state.remove_manifest();
            let _ = event_tx.send(BrowserEvent::Stopped { code: status.code() });
            break;
        }

        std::thread::sleep(Duration::from_millis(5));
    }
}

/// First existing `page` target, if any.
fn first_page_target(link: &mut CdpLink) -> Option<String> {
    let result = link
        .call_and_drain(CALL_TIMEOUT, "Target.getTargets", &serde_json::json!({}), None)
        .result
        .ok()?;
    result
        .get("targetInfos")
        .and_then(|t| t.as_array())
        .and_then(|targets| {
            targets
                .iter()
                .find(|t| t.get("type").and_then(serde_json::Value::as_str) == Some("page"))
                .filter(|t| !t.get("attached").and_then(serde_json::Value::as_bool).unwrap_or(false))
        })
        .and_then(|t| t.get("targetId"))
        .and_then(|t| t.as_str())
        .map(str::to_string)
}

/// Create a fresh page target as a fallback (browser opened without one).
fn create_page_target(link: &mut CdpLink, config: &BrowserSessionConfig) -> Option<String> {
    let url = config
        .initial_url
        .clone()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "about:blank".to_string());
    let result = link
        .call_and_drain(
            CALL_TIMEOUT,
            "Target.createTarget",
            &serde_json::json!({ "url": url }),
            None,
        )
        .result
        .ok()?;
    result.get("targetId").and_then(|t| t.as_str()).map(str::to_string)
}

fn build_launch(config: &BrowserSessionConfig) -> Result<crate::browser::process::ChromeLaunch, String> {
    let command = crate::browser::process::resolve_binary_or_default(&config.browser.command)
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())?;
    let profile_dir = profile_dir(&config.browser, &config.panel_local_id);
    Ok(crate::browser::process::ChromeLaunch {
        command,
        profile_dir,
        width: config.width,
        height: config.height,
        extra_args: config.browser.extra_args.clone(),
    })
}

pub(super) fn profile_dir(config: &BrowserConfig, panel_local_id: &str) -> std::path::PathBuf {
    let home = crate::horizon_home::HorizonHome::resolve();
    // The configured value is a *root*: every panel still gets its own
    // profile directory, or concurrent panels would fight over Chrome's
    // profile lock.
    match &config.profile_root {
        Some(root) => crate::config::Config::expand_tilde(&root.to_string_lossy())
            .join(crate::horizon_home::safe_local_id(panel_local_id)),
        None => home.browser_profile_dir(panel_local_id),
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
}

impl DriverState {
    fn new(config: &BrowserSessionConfig, browser_ws: &str) -> Self {
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
        event_tx: &mpsc::Sender<BrowserEvent>,
        frame_slot: &Arc<FrameSlot>,
        method: &str,
        params: &serde_json::Value,
        session: Option<&str>,
    ) -> Result<serde_json::Value, crate::browser::cdp::CdpError> {
        let outcome = link.call_and_drain(CALL_TIMEOUT, method, params, session);
        for message in outcome.drained {
            self.handle_message(link, event_tx, frame_slot, message);
        }
        outcome.result
    }

    fn send_page_command(
        &mut self,
        link: &mut CdpLink,
        event_tx: &mpsc::Sender<BrowserEvent>,
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
        event_tx: &mpsc::Sender<BrowserEvent>,
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

    fn attach_setup(
        &mut self,
        link: &mut CdpLink,
        event_tx: &mpsc::Sender<BrowserEvent>,
        frame_slot: &Arc<FrameSlot>,
        session: &str,
        target: &str,
    ) {
        self.session_id = Some(session.to_string());
        self.target_id = Some(target.to_string());
        let _ = self.call_and_ack(
            link,
            event_tx,
            frame_slot,
            "Page.enable",
            &serde_json::json!({}),
            Some(session),
        );
        let _ = self.call_and_ack(
            link,
            event_tx,
            frame_slot,
            "Runtime.enable",
            &serde_json::json!({}),
            Some(session),
        );
        let _ = self.call_and_ack(
            link,
            event_tx,
            frame_slot,
            "Emulation.setDeviceMetricsOverride",
            &serde_json::json!({
                "width": self.viewport_w,
                "height": self.viewport_h,
                "deviceScaleFactor": 1,
                "mobile": false,
            }),
            Some(session),
        );
        // A page that loaded before we attached (restart case) already has
        // its title; a fresh about:blank fetch returns empty and is skipped.
        self.fetch_title(link, event_tx, frame_slot);
        self.start_screencast(link, event_tx, frame_slot);
        self.write_manifest(true);
        let _ = event_tx.send(BrowserEvent::Ready);
        // One-shot: navigate to the panel's initial URL after first attach.
        if !self.initial_navigated {
            self.initial_navigated = true;
            let initial_url = self.config.initial_url.clone();
            if let Some(initial) = initial_url
                && initial != "about:blank"
            {
                self.navigate_to(link, event_tx, frame_slot, &initial);
            }
        }
    }

    fn start_screencast(
        &mut self,
        link: &mut CdpLink,
        event_tx: &mpsc::Sender<BrowserEvent>,
        frame_slot: &Arc<FrameSlot>,
    ) {
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

    fn note_reattach_failure(&mut self, event_tx: &mpsc::Sender<BrowserEvent>, message: &str) {
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

    fn pending_restart_tick(
        &mut self,
        link: &mut CdpLink,
        event_tx: &mpsc::Sender<BrowserEvent>,
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
            if let Some(ref target) = self.target_id {
                self.reattach_in_flight = true;
                self.session_id = None;
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

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn configured_profile_root_confines_unsafe_panel_ids() {
        let config = BrowserConfig {
            profile_root: Some("/tmp/horizon-browser-profiles".into()),
            ..BrowserConfig::default()
        };

        assert_eq!(
            profile_dir(&config, "../outside/profile"),
            std::path::PathBuf::from("/tmp/horizon-browser-profiles/%2e2e2f6f7574736964652f70726f66696c65")
        );
        assert_ne!(profile_dir(&config, "a/b"), profile_dir(&config, "a_b"));
    }

    #[test]
    fn shutdown_cancels_devtools_endpoint_wait() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("create temp dir: {error}"));
        let browser = temp.path().join("delayed-browser");
        let started = temp.path().join("started");
        std::fs::write(
            &browser,
            format!("#!/bin/sh\nprintf started > '{}'\nexec sleep 30\n", started.display()),
        )
        .unwrap_or_else(|error| panic!("write delayed browser: {error}"));
        let mut permissions = std::fs::metadata(&browser)
            .unwrap_or_else(|error| panic!("read delayed browser metadata: {error}"))
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&browser, permissions)
            .unwrap_or_else(|error| panic!("make delayed browser executable: {error}"));

        let session = start_session(BrowserSessionConfig {
            browser: BrowserConfig {
                command: Some(browser.to_string_lossy().into_owned()),
                profile_root: Some(temp.path().join("profiles")),
                ..BrowserConfig::default()
            },
            panel_local_id: "startup-cancel".to_string(),
            initial_url: None,
            width: 320,
            height: 200,
            frame_slot: Arc::new(FrameSlot::new()),
        })
        .unwrap_or_else(|error| panic!("start browser session: {error}"));

        let deadline = Instant::now() + Duration::from_secs(2);
        while !started.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(started.exists(), "delayed browser did not start");

        let completion = session.shutdown_signal();
        assert_eq!(completion.recv_timeout(Duration::from_secs(2)), Ok(()));
    }
}
