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

mod clipboard;
mod command_queue;
mod commands;
mod events;
mod handle;
mod http_bodies;
mod lifecycle;
mod manifest_io;
mod network;
mod semantic;
mod shutdown;
mod startup;

use clipboard::ClipboardState;
pub(crate) use command_queue::CommandReceiver;
use command_queue::CommandSender;
pub(crate) use handle::{BrowserEventSender, BrowserEventWake, publish_frame};
pub use handle::{BrowserEventWaker, CommittedUrl};
pub use horizon_browser_protocol::BrowserCommand;
pub use shutdown::BrowserShutdownSignal;
use startup::run_driver;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::cdp::{CdpError, CdpLink};
use crate::frames::FrameSlot;
use crate::process::{ChromeProcess, ChromeProcessControl};
use crate::semantic::SemanticState;
use crate::{ActiveBackendCapabilities, BackendKind, normalize_navigation_target};
use crate::{BrowserConfig, BrowserControlFailure};

/// What the driver reports to the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum BrowserEvent {
    /// Static features plus protocol extensions negotiated by this exact
    /// active session (not assumptions based only on browser name).
    BackendReady(ActiveBackendCapabilities),
    /// Chrome is up and the page session is attached.
    Ready,
    Title(String),
    /// A top-level document committed or recovered; the URL may repeat to clear a prior failure.
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
    /// Arm Safari host-focus recovery before input, then request it
    /// immediately once the blocking `WebDriver` call returns.
    HostFocusRequested {
        ready: bool,
    },
    /// The agent owning this panel changed (`None` = no live owner).
    OwnerChanged(Option<String>),
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
    /// Optional host-owned agent/session coordination. Standalone engine
    /// users leave this unset.
    pub coordination: Option<Arc<dyn crate::BrowserCoordination>>,
    /// Host-owned directory for explicit network-capture exports. Standalone
    /// embedders can opt in without depending on Horizon's home layout.
    pub capture_directory: Option<PathBuf>,
}

/// The panel-side handle to a running driver.
pub struct BrowserSession {
    command_tx: CommandSender,
    stop_requested: Arc<AtomicBool>,
    pub frame_slot: Arc<FrameSlot>,
    pub event_rx: mpsc::Receiver<BrowserEvent>,
    /// Resolved when the driver thread has finished tearing down Chrome.
    completion_rx: mpsc::Receiver<()>,
    event_wake: BrowserEventWake,
    committed_url: CommittedUrl,
    process_control: ChromeProcessControl,
    panel_local_id: String,
    coordination: Option<Arc<dyn crate::BrowserCoordination>>,
}

const WS_URL_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const RESTART_BACKOFF: Duration = Duration::from_millis(250);
const RESTART_BACKOFF_CAP: Duration = Duration::from_secs(1);
const MAX_RESTART_ATTEMPTS: u32 = 8;
/// Max CDP messages handled per pump pass. One message per pass capped the
/// link at ~200 msg/s (pass + 5 ms sleep), which backs up the socket under
/// animated pages plus pointer input and delays the screencast acks new
/// frames wait on.
const MAX_CDP_BURST: u32 = 16;
const MANIFEST_MIN_INTERVAL: Duration = Duration::from_millis(200);
const SIGNAL_MIN_INTERVAL: Duration = Duration::from_millis(250);
/// Pointer-move storms rewrite the manifest on every event otherwise; the
/// 5 s user-active TTL tolerates a 1 s refresh.
const USER_ACTIVE_STAMP_INTERVAL: Duration = Duration::from_secs(1);
const USER_ACTIVE_TTL: Duration = Duration::from_secs(5);
/// Wait until a resize gesture settles before forcing one current-viewport
/// screenshot. Chrome can update device metrics without emitting a
/// screencast frame, which otherwise leaves the previous frame stretched.
const VIEWPORT_CAPTURE_DELAY: Duration = Duration::from_millis(75);
const VIEWPORT_RETRY_DELAY: Duration = Duration::from_millis(100);
const SCROLLBAR_LAYOUT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Spawn the driver thread. Browser startup problems are reported through
/// the event channel rather than failing here.
///
/// The launch config is resolved on the calling thread (profile root
/// anchoring, quality clamping) so direct callers of this low-level entry
/// point behave identically to `BrowserPanelState::start` instead of
/// resolving a relative profile root against the driver thread's working
/// directory.
///
/// # Errors
/// Fails when the launch config cannot be resolved or the OS thread cannot
/// be spawned.
pub fn start_session(config: BrowserSessionConfig) -> Result<BrowserSession, crate::BrowserError> {
    let mut config = config;
    config.browser = config
        .browser
        .resolved_for_launch()
        .map_err(crate::BrowserError::LaunchConfig)?;
    let frame_slot = config.frame_slot.clone();
    let (command_tx, command_rx) = command_queue::channel(Arc::clone(&frame_slot));
    let (raw_event_tx, event_rx) = mpsc::channel::<BrowserEvent>();
    let event_wake = BrowserEventWake::default();
    let committed_url = CommittedUrl::default();
    let event_tx = BrowserEventSender {
        tx: raw_event_tx,
        wake: event_wake.clone(),
        committed_url: committed_url.clone(),
    };
    let (completion_tx, completion_rx) = mpsc::channel::<()>();
    let process_control = ChromeProcessControl::default();
    let driver_process_control = process_control.clone();
    let panel_local_id = config.panel_local_id.clone();
    let coordination = config.coordination.clone();
    let stop_requested = Arc::new(AtomicBool::new(false));
    let driver_stop_requested = Arc::clone(&stop_requested);
    let slot = Arc::clone(&frame_slot);
    std::thread::Builder::new()
        .name("browser-driver".into())
        .spawn(move || match config.browser.backend {
            BackendKind::ChromiumCdp => run_driver(
                &config,
                &event_tx,
                &command_rx,
                &slot,
                &driver_stop_requested,
                completion_tx,
                driver_process_control,
            ),
            BackendKind::FirefoxBidi | BackendKind::SafariWebDriver => crate::webdriver::run_webdriver(
                &config,
                &event_tx,
                &command_rx,
                &slot,
                &driver_stop_requested,
                completion_tx,
                &driver_process_control,
            ),
        })
        .map_err(crate::BrowserError::DriverThread)?;
    Ok(BrowserSession {
        command_tx,
        stop_requested,
        frame_slot,
        event_rx,
        completion_rx,
        event_wake,
        committed_url,
        process_control,
        panel_local_id,
        coordination,
    })
}

fn run_loop(
    state: &mut DriverState,
    chrome: &mut ChromeProcess,
    link: &mut CdpLink,
    command_rx: &CommandReceiver,
    frame_slot: &Arc<FrameSlot>,
    event_tx: &BrowserEventSender,
) {
    loop {
        // 1. Commands from the UI.
        if state.drain_commands(link, command_rx, event_tx, frame_slot) {
            state.flush_http_response_bodies(link, event_tx, frame_slot);
            state.capture_final_url(link, event_tx, frame_slot);
            // Ask Chrome to exit cleanly so it marks its profile session
            // complete (kill alone leaves a "crashed" state that makes the
            // next launch restore stale tabs); the kill below is the
            // fallback for an uncooperative process.
            let outcome = link.call_and_drain(Duration::from_secs(1), "Browser.close", &serde_json::json!({}), None);
            for message in outcome.drained {
                state.handle_message(link, event_tx, frame_slot, message);
            }
            let _ = chrome.kill();
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            break;
        }

        // 2. CDP messages: responses and events. Drain a bounded burst of
        //    ready messages so sustained traffic keeps pace with what one
        //    pass can emit (input, viewport, clipboard, screencast acks).
        let mut connection_lost = None;
        let mut burst = 0;
        while burst < MAX_CDP_BURST {
            match link.read_one() {
                Ok(Some(message)) => {
                    state.handle_message(link, event_tx, frame_slot, message);
                    burst += 1;
                }
                Ok(None) => break,
                Err(error) => {
                    connection_lost = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = connection_lost {
            // The connection is gone. Reconnecting a single panel's
            // session is not worth the state surgery; surface the
            // failure and stop — the panel offers Retry.
            tracing::warn!(target: "browser", "cdp connection lost: {error}");
            let _ = event_tx.send(BrowserEvent::Warning(format!("CDP connection lost: {error}")));
            let _ = chrome.kill();
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            break;
        }

        // Response bodies are opt-in and fetched only after the matching
        // loading-finished event. Keep each pass bounded so network traffic
        // cannot starve input, frames, or coordination.
        state.tick_http_response_bodies(link, event_tx, frame_slot);
        if let Some(message) = state.challenge_loop.take_rejection() {
            let _ = event_tx.send(BrowserEvent::NavigationFailed(message.to_string()));
        }

        // 3. Flush a pending throttled manifest write (the loop always
        //    iterates, so a quiet page still gets its url/title flushed).
        state.write_manifest(false);

        // 3b. Delayed post-load title fetch.
        state.tick_title_fetch(link, event_tx, frame_slot);

        // 3c. Retry a transiently rejected viewport command, then capture one
        //     fresh frame after the resize burst settles.
        state.tick_viewport_resize(link, event_tx, frame_slot);
        state.tick_viewport_capture(link, frame_slot);

        // 3d. Keep the native-scrollbar hit-test geometry warm without ever
        //     blocking the input path on a CDP roundtrip.
        state.tick_scrollbar_layout(link);

        // 4. Backoff-retried screencast restart / re-attach.
        state.pending_restart_tick(link, event_tx, frame_slot);

        // 5. Ownership / handoff signals from the manifest (agent side).
        let actions = state.tick_signals(event_tx);
        let _ = state.drain_agent_actions(link, event_tx, frame_slot, actions);

        // 6. Chrome process liveness.
        if let Some(status) = chrome.child_status() {
            let _ = event_tx.send(BrowserEvent::Stopped { code: status.code() });
            break;
        }

        std::thread::sleep(Duration::from_millis(5));
    }
}

#[derive(Clone, Copy, Debug)]
struct VerticalScrollbarLayout {
    client_width: f64,
    client_height: f64,
    scroll_y: f64,
    content_height: f64,
}

impl VerticalScrollbarLayout {
    fn from_metrics(metrics: &serde_json::Value) -> Option<Self> {
        let layout = metrics.get("cssLayoutViewport")?;
        let content = metrics.get("cssContentSize")?;
        let candidate = Self {
            client_width: layout.get("clientWidth")?.as_f64()?,
            client_height: layout.get("clientHeight")?.as_f64()?,
            scroll_y: layout.get("pageY")?.as_f64()?,
            content_height: content.get("height")?.as_f64()?,
        };
        candidate.is_valid().then_some(candidate)
    }

    fn is_valid(self) -> bool {
        self.client_width.is_finite()
            && self.client_width >= 0.0
            && self.client_height.is_finite()
            && self.client_height > 0.0
            && self.scroll_y.is_finite()
            && self.scroll_y >= 0.0
            && self.content_height.is_finite()
            && self.content_height >= self.client_height
    }
}

#[derive(Debug)]
struct ScrollbarLayoutCache {
    layout: Option<VerticalScrollbarLayout>,
    request_id: Option<u64>,
    refresh_at: Option<Instant>,
}

impl ScrollbarLayoutCache {
    fn new() -> Self {
        Self {
            layout: None,
            request_id: None,
            refresh_at: Some(Instant::now()),
        }
    }
}

/// Driver-side state machine for one page session.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // intentional: per-concern state flags
struct DriverState {
    config: BrowserSessionConfig,
    browser_ws: String,
    /// Browser-owned Client Hints captured from a temporary hidden target
    /// before the caller page is attached. Reused across target reattaches so
    /// the main page never needs a diagnostic bootstrap navigation.
    native_user_agent_metadata: Option<serde_json::Value>,
    session_id: Option<String>,
    target_id: Option<String>,
    main_frame_id: Option<String>,
    viewport_w: u32,
    viewport_h: u32,
    pending_viewport: Option<(u32, u32)>,
    viewport_retry_at: Option<Instant>,
    pending_viewport_capture_at: Option<Instant>,
    viewport_capture_request_id: Option<u64>,
    /// Headless Chromium paints its native scrollbar into screencast frames,
    /// but `Input.dispatchMouseEvent` cannot operate that browser-owned UI.
    /// A press inside the measured gutter therefore becomes an engine-owned
    /// scroll interaction until the matching release.
    vertical_scrollbar_drag: Option<VerticalScrollbarDrag>,
    scrollbar_layout: ScrollbarLayoutCache,
    /// First visually meaningful command waiting for a published frame.
    /// Additional commands coalesce into the same sample until pixels arrive.
    interaction_started_at: Option<Instant>,
    /// Keep the last good pixels while an explicit navigation has not yet
    /// committed a reachable top-level document.
    retain_frame_during_navigation: bool,
    navigation_failed: bool,
    clipboard: ClipboardState,
    url: String,
    title: String,
    initial_navigated: bool,
    screencast_on: bool,
    pending_restart_at: Option<Instant>,
    /// Delayed `Target.getTargetInfo` read after navigation. Title updates
    /// come from protocol target metadata, not a page-JS binding.
    title_fetch_at: Option<Instant>,
    /// `Runtime.enable` is deferred until snapshot, evaluate, clipboard, or
    /// scrollbar scrolling actually needs it. Enabling it at attach is a
    /// script-visible CDP signal.
    runtime_enabled: bool,
    runtime_enable_requested: HashSet<String>,
    runtime_enable_inflight: HashMap<u64, String>,
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
    handoff_seen: Option<String>,
    audit_sampler: crate::audit::BrowserAuditSampler,
    semantic: SemanticState,
    challenge_loop: crate::challenge::ChallengeLoopDetector,
    network: crate::network::NetworkCaptureState,
    pending_http_bodies: VecDeque<http_bodies::PendingHttpBody>,
    http_body_evidence: http_bodies::HttpBodyEvidenceTable,
    stop_requested: Arc<AtomicBool>,
}

impl DriverState {
    fn new(
        config: &BrowserSessionConfig,
        browser_ws: &str,
        native_user_agent_metadata: Option<serde_json::Value>,
        stop_requested: Arc<AtomicBool>,
    ) -> Self {
        Self {
            config: config.clone(),
            browser_ws: browser_ws.to_string(),
            native_user_agent_metadata,
            session_id: None,
            target_id: None,
            main_frame_id: None,
            viewport_w: config.width,
            viewport_h: config.height,
            pending_viewport: None,
            viewport_retry_at: None,
            pending_viewport_capture_at: None,
            viewport_capture_request_id: None,
            vertical_scrollbar_drag: None,
            scrollbar_layout: ScrollbarLayoutCache::new(),
            interaction_started_at: None,
            retain_frame_during_navigation: false,
            navigation_failed: false,
            clipboard: ClipboardState::default(),
            // The requested initial URL is not committed state. Chrome starts
            // at about:blank and navigation may fail or be cancelled.
            url: String::new(),
            title: String::new(),
            initial_navigated: false,
            screencast_on: false,
            pending_restart_at: None,
            title_fetch_at: None,
            runtime_enabled: false,
            runtime_enable_requested: HashSet::new(),
            runtime_enable_inflight: HashMap::new(),
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
            audit_sampler: crate::audit::BrowserAuditSampler::default(),
            semantic: SemanticState::default(),
            challenge_loop: crate::challenge::ChallengeLoopDetector::default(),
            network: crate::network::NetworkCaptureState::default(),
            pending_http_bodies: VecDeque::new(),
            http_body_evidence: http_bodies::HttpBodyEvidenceTable::default(),
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
    ) -> Result<serde_json::Value, crate::cdp::CdpError> {
        let stop_requested = Arc::clone(&self.stop_requested);
        let outcome = link.call_and_drain_until(CALL_TIMEOUT, method, params, session, || {
            stop_requested.load(Ordering::Acquire)
        });
        for message in outcome.drained {
            self.handle_message(link, event_tx, frame_slot, message);
        }
        outcome.result
    }

    /// Freeze an in-flight navigation, route every event received while that
    /// happens, then ask the target for its authoritative committed URL.
    fn capture_final_url(&mut self, link: &mut CdpLink, event_tx: &BrowserEventSender, frame_slot: &Arc<FrameSlot>) {
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
        if method.starts_with("Runtime.") {
            self.ensure_page_runtime(link)?;
        }
        self.call_and_ack(link, event_tx, frame_slot, method, params, Some(session.as_str()))
    }

    fn ensure_page_runtime(&mut self, link: &mut CdpLink) -> Result<(), CdpError> {
        if self.session_id.is_none() {
            return Err(CdpError::NoPageSession {
                method: "Runtime.enable".to_string(),
            });
        }
        self.request_runtime_for_sessions(link);
        Ok(())
    }

    fn request_runtime_for_sessions(&mut self, link: &mut CdpLink) {
        let Some(page_session) = self.session_id.clone() else {
            return;
        };
        self.queue_runtime_enable(link, &page_session);
        let iframe_sessions: Vec<String> = self.clipboard.iframe_sessions.iter().cloned().collect();
        for session in iframe_sessions {
            self.queue_runtime_enable(link, &session);
        }
    }

    fn queue_runtime_enable(&mut self, link: &mut CdpLink, session: &str) {
        if self.runtime_enable_requested.contains(session) {
            return;
        }
        match link.send_request("Runtime.enable", &serde_json::json!({}), Some(session)) {
            Ok(request_id) => {
                self.runtime_enable_requested.insert(session.to_string());
                self.runtime_enable_inflight.insert(request_id, session.to_string());
            }
            Err(error) => tracing::debug!(
                target: "browser",
                session,
                "Runtime.enable request failed: {error}"
            ),
        }
    }

    fn handle_runtime_enable_response(&mut self, id: u64, error: Option<&crate::cdp::CdpErrorInfo>) -> bool {
        let Some(session) = self.runtime_enable_inflight.remove(&id) else {
            return false;
        };
        if error.is_some() {
            self.runtime_enable_requested.remove(&session);
            return true;
        }
        if self.session_id.as_deref() == Some(session.as_str()) {
            self.runtime_enabled = true;
        }
        true
    }

    fn reset_runtime_enable_state(&mut self) {
        self.runtime_enabled = false;
        self.runtime_enable_requested.clear();
        self.runtime_enable_inflight.clear();
    }

    fn navigate_to(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        url: &str,
    ) -> Result<(), BrowserControlFailure> {
        let url = normalize_navigation_target(url);
        self.challenge_loop.document_navigation_started();
        self.retain_frame_during_navigation = true;
        self.navigation_failed = false;
        let result = self.send_page_command(
            link,
            event_tx,
            frame_slot,
            "Page.navigate",
            &serde_json::json!({ "url": &url }),
        );
        match result {
            Ok(result) => {
                if let Some(error) = result.get("errorText").and_then(serde_json::Value::as_str)
                    && !error.is_empty()
                {
                    self.navigation_failed = true;
                    self.interaction_started_at = None;
                    let _ = event_tx.send(BrowserEvent::NavigationFailed(format!(
                        "could not navigate to {url}: {error}"
                    )));
                    let _ = event_tx.send(BrowserEvent::Loading(false));
                    return Err(BrowserControlFailure::new("navigation_failed", error));
                }
                // The committed URL remains authoritative and arrives via a
                // navigation event. Do not overwrite it with a merely
                // requested value.
                self.pending_restart_at = Some(Instant::now());
                let _ = event_tx.send(BrowserEvent::Loading(true));
                Ok(())
            }
            Err(error) => {
                self.navigation_failed = true;
                self.interaction_started_at = None;
                let _ = event_tx.send(BrowserEvent::NavigationFailed(format!(
                    "could not navigate to {url}: {error}"
                )));
                let _ = event_tx.send(BrowserEvent::Loading(false));
                Err(BrowserControlFailure::new("protocol_error", error.to_string()))
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct VerticalScrollbarDrag {
    pointer_y: f64,
    scroll_y: f64,
    max_scroll: f64,
    scroll_per_pointer_pixel: f64,
}

impl VerticalScrollbarDrag {
    fn target_scroll_y(self, pointer_y: f64) -> f64 {
        (self.scroll_y + ((pointer_y - self.pointer_y) * self.scroll_per_pointer_pixel)).clamp(0.0, self.max_scroll)
    }
}

fn normalized_committed_url(url: &str) -> &str {
    if url.is_empty() || url == "about:blank" {
        ""
    } else {
        url
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::is_user_activity;

    #[test]
    fn blank_target_is_an_empty_committed_url() {
        assert_eq!(normalized_committed_url("about:blank"), "");
        assert_eq!(normalized_committed_url(""), "");
        assert_eq!(normalized_committed_url("https://example.com"), "https://example.com");
    }

    #[test]
    fn page_controls_count_as_user_steering_but_system_controls_do_not() {
        assert!(is_user_activity(&BrowserCommand::Navigate(
            "https://example.test".to_string()
        )));
        assert!(is_user_activity(&BrowserCommand::Reload));
        assert!(is_user_activity(&BrowserCommand::Back));
        assert!(is_user_activity(&BrowserCommand::Forward));
        assert!(!is_user_activity(&BrowserCommand::SetViewport {
            width: 800,
            height: 600
        }));
        assert!(!is_user_activity(&BrowserCommand::HandoffDone));
        assert!(!is_user_activity(&BrowserCommand::Stop));
    }
}
