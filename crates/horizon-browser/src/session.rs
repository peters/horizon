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

mod address;
mod call;
mod clipboard;
mod command_queue;
mod commands;
mod events;
mod file_chooser;
pub(crate) mod file_drop;
mod handle;
mod http_auth;
mod http_bodies;
mod lifecycle;
mod manifest_io;
mod native_select;
mod navigation;
mod network;
mod semantic;
mod shared;
mod shutdown;
use shared::DriverProcess;
pub use shared::SharedBrowserSession;

pub use shutdown::{DriverTeardown, RemoteReleaseReport};
mod startup;
pub(crate) mod viewport;
mod wait;
mod webauthn;

use clipboard::ClipboardState;
pub(crate) use command_queue::CommandReceiver;
use command_queue::CommandSender;
pub(crate) use handle::{BrowserEventSender, BrowserEventWake, publish_frame};
pub use handle::{BrowserEventWaker, CommittedUrl};
pub use horizon_browser_protocol::BrowserCommand;
use native_select::NativeSelectState;
pub use shutdown::BrowserShutdownSignal;
use startup::{DriverLaunch, run_driver};

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::cdp::{CdpError, CdpLink};
use crate::frames::FrameSlot;
use crate::page_scroll::VerticalScrollbarDrag;
use crate::process::ChromeProcessControl;
use crate::semantic::SemanticState;
use crate::webdriver::RemoteReleaseOutcome;
use crate::{ActiveBackendCapabilities, BackendKind, PageScrollState, normalize_navigation_target};
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
    /// Page-pixel recording could not start, pause, resume, or finalize.
    VideoFailed(String),
    /// Measured remote orientation and rotation progress.
    OrientationChanged(crate::remote::RemoteOrientationView),
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
    /// The page this session drives. `resumed` says that it is the page the
    /// panel drove before a lost link came back, so the page keeps what it
    /// had; `false` says that the page is new and the previous state is gone.
    PageAttached {
        target_id: String,
        resumed: bool,
    },
    /// Allocation, expiry and release facts for a remote session.
    RemoteSession(crate::RemoteSessionEvent),
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
    /// Shared page-pixel recording status for the host UI and driver.
    pub video: Arc<crate::VideoCaptureHandle>,
    /// When set, the session runs at a remote grid through the classic
    /// `WebDriver` path instead of a local browser process.
    pub remote: Option<crate::RemoteSessionRequest>,
    /// When set, this session drives a browser that another process started,
    /// and it owns nothing of that browser: it launches none, prepares and
    /// removes no profile folder, sets no download behavior, closes only the
    /// page it created and never the browser.
    pub attach: Option<crate::DevtoolsEndpoint>,
    /// The page target that this panel drove before its link was lost. A
    /// session with a resume target adopts that page instead of creating one,
    /// and it does not navigate: the page still holds the text the person
    /// typed into it, its scroll place and its session. A target that is gone
    /// falls back to a new page of `initial_url`.
    pub resume_target: Option<String>,
}

/// The panel-side handle to a running driver.
pub struct BrowserSession {
    command_tx: CommandSender,
    stop_requested: Arc<AtomicBool>,
    pub frame_slot: Arc<FrameSlot>,
    video: Arc<crate::VideoCaptureHandle>,
    pub event_rx: mpsc::Receiver<BrowserEvent>,
    /// Resolved when the driver thread has finished tearing down Chrome.
    completion_rx: mpsc::Receiver<()>,
    /// What a remote driver established at release; shared with its signal.
    remote_release: shutdown::RemoteReleaseReport,
    /// Configured provider of a remote session, for the host's occupancy count.
    remote_provider: Option<String>,
    remote_recovery: Option<crate::RemoteAllocation>,
    /// The provider identity the allocation counts against across instances.
    remote_quota_key: Option<String>,
    event_wake: BrowserEventWake,
    committed_url: CommittedUrl,
    process_control: ChromeProcessControl,
    panel_local_id: String,
    coordination: Option<Arc<dyn crate::BrowserCoordination>>,
}

const WS_URL_TIMEOUT: Duration = Duration::from_secs(15);
pub(super) use call::CALL_TIMEOUT;
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
/// How often a painting page re-reads its scroll geometry: four times a second
/// is enough for an overlay, and a request per picture would put a message pair
/// on the same link as every frame.
const SCROLLBAR_LAYOUT_REFRESH_INTERVAL: Duration = Duration::from_millis(250);

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
    start_session_with_group(config, None)
}

/// A local browser profile with independently controlled pages.
#[derive(Clone, Debug)]
pub enum SharedSessionGroup {
    Chromium(SharedBrowserSession),
    Firefox(crate::webdriver::SharedFirefoxSession),
}

impl From<SharedBrowserSession> for SharedSessionGroup {
    fn from(group: SharedBrowserSession) -> Self {
        Self::Chromium(group)
    }
}

impl SharedSessionGroup {
    #[must_use]
    pub fn new_firefox(profile_id: String) -> Self {
        Self::Firefox(crate::webdriver::SharedFirefoxSession::new(profile_id))
    }

    #[must_use]
    pub fn profile_id(&self) -> &str {
        match self {
            Self::Chromium(group) => group.profile_id(),
            Self::Firefox(group) => group.profile_id(),
        }
    }

    #[must_use]
    pub fn is_idle(&self) -> bool {
        match self {
            Self::Chromium(group) => group.is_idle(),
            Self::Firefox(group) => group.is_idle(),
        }
    }

    pub(super) fn retire_profile_for_cleanup(&self) -> bool {
        match self {
            Self::Chromium(group) => group.retire_profile_for_cleanup(),
            Self::Firefox(group) => group.retire_profile_for_cleanup(),
        }
    }

    fn backend(&self) -> BackendKind {
        match self {
            Self::Chromium(_) => BackendKind::ChromiumCdp,
            Self::Firefox(_) => BackendKind::FirefoxBidi,
        }
    }

    fn firefox(&self) -> Option<&crate::webdriver::SharedFirefoxSession> {
        match self {
            Self::Firefox(group) => Some(group),
            Self::Chromium(_) => None,
        }
    }

    fn reserve(self, stop: Arc<AtomicBool>) -> Option<shared::SharedDriverReservation> {
        match self {
            Self::Chromium(group) => Some(group.reserve(stop)),
            Self::Firefox(_) => None,
        }
    }
}

/// Start a separate page in a shared local Chromium or Firefox session.
///
/// # Errors
/// Rejects unsupported backends or invalid launch configurations.
pub fn start_shared_session(
    config: BrowserSessionConfig,
    group: impl Into<SharedSessionGroup>,
) -> Result<BrowserSession, crate::BrowserError> {
    let group = group.into();
    if config.browser.backend != group.backend() || config.remote.is_some() {
        return Err(crate::BrowserError::LaunchConfig(std::io::Error::other(
            "shared session backend must match the local browser",
        )));
    }
    start_session_with_group(config, Some(group))
}

fn start_session_with_group(
    config: BrowserSessionConfig,
    group: Option<SharedSessionGroup>,
) -> Result<BrowserSession, crate::BrowserError> {
    let mut config = config;
    config.browser = config
        .browser
        .resolved_for_launch()
        .map_err(crate::BrowserError::LaunchConfig)?;
    let frame_slot = config.frame_slot.clone();
    let video = Arc::clone(&config.video);
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
    // Until the driver is about to ask the provider for a session, a remote
    // teardown holds nothing: a failure before New Session (a coordination
    // preflight, a rejected endpoint) must not consume the provider's slot.
    // The driver switches the report to unknown right before that request.
    let remote_release = shutdown::RemoteReleaseReport::new(std::sync::Mutex::new(
        config.remote.as_ref().map(|_| RemoteReleaseOutcome::NeverAllocated),
    ));
    let remote_recovery = config.remote.as_ref().map(|r| r.recovery.clone());
    let remote_provider = config.remote.as_ref().map(|request| request.provider.clone());
    let remote_quota_key = config.remote.as_ref().map(|request| request.quota_key.clone());
    let driver_remote_release = Arc::clone(&remote_release);
    let process_control = ChromeProcessControl::default();
    let driver_process_control = process_control.clone();
    let panel_local_id = config.panel_local_id.clone();
    let coordination = config.coordination.clone();
    let stop_requested = Arc::new(AtomicBool::new(false));
    let firefox_group = group
        .as_ref()
        .and_then(SharedSessionGroup::firefox)
        .map(|group| group.reserve(Arc::clone(&stop_requested)));
    let group = group.and_then(|group| group.reserve(Arc::clone(&stop_requested)));
    let driver_stop_requested = Arc::clone(&stop_requested);
    let slot = Arc::clone(&frame_slot);
    std::thread::Builder::new()
        .name("browser-driver".into())
        .spawn(move || match config.browser.backend {
            _ if config.remote.is_some() => crate::webdriver::run_webdriver(
                &config,
                &event_tx,
                &command_rx,
                &slot,
                &driver_stop_requested,
                DriverTeardown {
                    completion: completion_tx,
                    remote_release: driver_remote_release,
                },
                crate::webdriver::WebDriverLaunch {
                    process_control: &driver_process_control,
                    group: None,
                },
            ),
            BackendKind::ChromiumCdp => run_driver(
                &config,
                &event_tx,
                &command_rx,
                &slot,
                &driver_stop_requested,
                DriverLaunch {
                    completion_tx,
                    process_control: driver_process_control,
                    group,
                },
            ),
            BackendKind::FirefoxBidi | BackendKind::SafariWebDriver => crate::webdriver::run_webdriver(
                &config,
                &event_tx,
                &command_rx,
                &slot,
                &driver_stop_requested,
                DriverTeardown {
                    completion: completion_tx,
                    remote_release: driver_remote_release,
                },
                crate::webdriver::WebDriverLaunch {
                    process_control: &driver_process_control,
                    group: firefox_group,
                },
            ),
        })
        .map_err(crate::BrowserError::DriverThread)?;
    Ok(BrowserSession {
        command_tx,
        stop_requested,
        frame_slot,
        video,
        event_rx,
        completion_rx,
        remote_release,
        remote_provider,
        remote_recovery,
        remote_quota_key,
        event_wake,
        committed_url,
        process_control,
        panel_local_id,
        coordination,
    })
}

fn run_loop(
    state: &mut DriverState,
    chrome: &mut DriverProcess,
    link: &mut CdpLink,
    command_rx: &CommandReceiver,
    frame_slot: &Arc<FrameSlot>,
    event_tx: &BrowserEventSender,
) {
    loop {
        let pass_started = Instant::now();
        // 1. Commands from the UI.
        if state.drain_commands(link, command_rx, event_tx, frame_slot) {
            state.flush_http_response_bodies(link, event_tx, frame_slot);
            state.capture_final_url(link, event_tx, frame_slot);
            state.settle_pending_wait_for_shutdown(Instant::now());
            if let Some(target) = state.target_id.as_deref() {
                chrome.close_page(link, target);
            } else {
                let _ = chrome.kill();
            }
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
            state.audit_session_event(crate::BrowserAuditAction::LinkLost);
            let _ = event_tx.send(BrowserEvent::Warning(format!("CDP connection lost: {error}")));
            state.settle_pending_wait_for_shutdown(Instant::now());
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

        state.tick_file_chooser(link, event_tx, frame_slot);
        state.tick_passkeys(link, event_tx, frame_slot);

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
        if stop_for_chrome_exit(state, chrome, event_tx) {
            break;
        }
        let actions = state.tick_signals(event_tx);
        let _ = state.drain_agent_actions(link, event_tx, frame_slot, actions, chrome);
        if stop_for_chrome_exit(state, chrome, event_tx) {
            break;
        }
        state.tick_pending_navigation();
        state.tick_pending_wait(link, event_tx, frame_slot, chrome);
        state.tick_pending_resize(link, event_tx, frame_slot);

        // 6. Chrome process liveness.
        if stop_for_chrome_exit(state, chrome, event_tx) {
            break;
        }

        report_blocked_pass(link, pass_started);
        std::thread::sleep(Duration::from_millis(5));
    }
    state.finish_pending_resize("browser_unavailable", "browser session stopped");
}

/// A pass that blocked on the protocol says so, with the command it waited for.
fn report_blocked_pass(link: &mut CdpLink, pass_started: Instant) {
    if !crate::latency::enabled() {
        return;
    }
    let waited = link.take_blocking_call();
    crate::latency::stall(
        pass_started.elapsed(),
        waited.as_ref().map(|(method, waited)| (method.as_str(), *waited)),
    );
}

fn stop_for_chrome_exit(state: &mut DriverState, chrome: &mut DriverProcess, event_tx: &BrowserEventSender) -> bool {
    let Some(status) = chrome.child_status() else {
        return false;
    };
    state.settle_pending_wait_for_shutdown(Instant::now());
    let _ = event_tx.send(BrowserEvent::Stopped { code: status.code() });
    true
}

#[derive(Debug)]
struct ScrollbarLayoutCache {
    layout: Option<PageScrollState>,
    request_id: Option<u64>,
    refresh_at: Option<Instant>,
    /// Size of the picture that the last layout read described, so a resize is
    /// a reason to read again and an unchanged picture is not.
    frame_size: Option<(u32, u32)>,
    /// A picture arrived since the last layout answer, so the page still paints
    /// and the next slow read is due.
    painted_since_refresh: bool,
}

impl ScrollbarLayoutCache {
    fn new() -> Self {
        Self {
            layout: None,
            request_id: None,
            refresh_at: Some(Instant::now()),
            frame_size: None,
            painted_since_refresh: false,
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
    /// Whether this session adopted the page the panel had before a lost
    /// link, so the page keeps its state and no startup navigation runs.
    resumed_page: bool,
    main_frame_id: Option<String>,
    pending_resize: Option<viewport::PendingResize>,
    viewport_policy: viewport::ViewportPolicy,
    viewport_w: u32,
    viewport_h: u32,
    pending_viewport: Option<(u32, u32)>,
    viewport_retry_at: Option<Instant>,
    pending_viewport_capture_at: Option<Instant>,
    viewport_capture_request_id: Option<u64>,
    /// Headless Chromium paints a native scrollbar that CDP mouse events
    /// cannot operate. Presses in the host overlay gutter become an
    /// engine-owned scroll interaction until the matching release.
    vertical_scrollbar_drag: Option<VerticalScrollbarDrag>,
    scrollbar_layout: ScrollbarLayoutCache,
    /// First visually meaningful command waiting for a published frame.
    /// Additional commands coalesce into the same sample until pixels arrive.
    interaction_started_at: Option<Instant>,
    /// Keep the last good pixels while an explicit navigation has not yet
    /// committed a reachable top-level document.
    retain_frame_during_navigation: bool,
    navigation_failed: bool,
    /// Agent navigation whose typed outcome is settled by page events.
    pending_navigation: Option<crate::navigation::PendingNavigation>,
    pending_wait: Option<crate::wait::PendingWait>,
    /// The top frame started a navigation (reload, history, page-initiated,
    /// or command) whose commit or failure has not arrived yet.
    top_frame_navigating: bool,
    /// In-flight `Page.navigate` request. The reply is routed asynchronously
    /// so a slow destination never blocks frames, input, or coordination.
    navigate_request_id: Option<u64>,
    /// In-flight `Page.startScreencast` request, for the same reason.
    screencast_request_id: Option<u64>,
    /// In-flight `Target.getTargetInfo` question, which never blocks the loop.
    title_request_id: Option<u64>,
    clipboard: ClipboardState,
    native_select: NativeSelectState,
    file_chooser: file_chooser::ChooserState,
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
    /// Counts successful coordination reads; a deferred wait result is
    /// released only under a later epoch than it was observed under.
    signal_epoch: u64,
    handoff_seen: Option<String>,
    audit_sampler: crate::audit::BrowserAuditSampler,
    semantic: SemanticState,
    challenge_loop: crate::challenge::ChallengeLoopDetector,
    http_auth: crate::http_auth::HttpAuthState,
    network: crate::network::NetworkCaptureState,
    video: crate::video::VideoCaptureState,
    /// The panel's download folder and the downloads this session observed.
    downloads: crate::downloads::DownloadState,
    pending_http_bodies: VecDeque<http_bodies::PendingHttpBody>,
    http_body_evidence: http_bodies::HttpBodyEvidenceTable,
    passkeys: Option<webauthn::CdpPasskeys>,
    stop_requested: Arc<AtomicBool>,
}

impl DriverState {
    /// Why this session cannot take a file of this computer: the browser that it
    /// drives runs on another machine, where a path of this computer names no
    /// file.
    pub(super) fn local_file_refusal(&self) -> Option<BrowserControlFailure> {
        self.config.attach.as_ref().map(|_| {
            BrowserControlFailure::new(
                "remote_file_upload_unsupported",
                "This panel drives a browser of another machine, which cannot read a file of this computer",
            )
        })
    }

    /// The disclosure claim of this exact session. A browser that another
    /// process started keeps its own automation switch, so this engine claims
    /// no minimization of it, whatever it establishes over the link.
    fn disclosure_status(&self) -> crate::AutomationDisclosureStatus {
        if self.config.attach.is_some() {
            return self.config.browser.automation_disclosure.borrowed_browser_status();
        }
        self.config
            .browser
            .automation_disclosure
            .ready_status(self.config.browser.backend)
    }

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
            resumed_page: false,
            main_frame_id: None,
            pending_resize: None,
            viewport_policy: viewport::ViewportPolicy::new([config.width, config.height]),
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
            pending_navigation: None,
            pending_wait: None,
            top_frame_navigating: false,
            navigate_request_id: None,
            screencast_request_id: None,
            title_request_id: None,
            clipboard: ClipboardState::default(),
            native_select: NativeSelectState::default(),
            file_chooser: file_chooser::ChooserState::default(),
            // The requested initial URL is not committed state. Chrome starts
            // at about:blank and navigation may fail or be cancelled.
            url: String::new(),
            title: String::new(),
            initial_navigated: false,
            screencast_on: false,
            pending_restart_at: None,
            title_fetch_at: None,
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
            signal_epoch: 0,
            handoff_seen: None,
            audit_sampler: crate::audit::BrowserAuditSampler::default(),
            semantic: SemanticState::default(),
            challenge_loop: crate::challenge::ChallengeLoopDetector::default(),
            http_auth: crate::http_auth::HttpAuthState::default(),
            network: crate::network::NetworkCaptureState::default(),
            // The driver sets the panel folder once it knows whether this
            // browser profile is shared, before any page can download.
            downloads: crate::downloads::DownloadState::default(),
            video: crate::video::VideoCaptureState::new(Arc::clone(&config.video)),
            pending_http_bodies: VecDeque::new(),
            http_body_evidence: http_bodies::HttpBodyEvidenceTable::default(),
            passkeys: webauthn::CdpPasskeys::from_config(config),
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
        let outcome = link.call_and_drain_until(self.call_budget(), method, params, session, || {
            stop_requested.load(Ordering::Acquire)
        });
        for message in outcome.drained {
            self.handle_message(link, event_tx, frame_slot, message);
        }
        outcome.result
    }

    /// The budget of one protocol call of this session: the loopback budget, or
    /// the larger one of a browser that another process or another computer runs.
    pub(super) fn call_budget(&self) -> Duration {
        call::Call::of(&self.config, call::Kind::Change).budget()
    }

    /// One read of this session: it holds the budget of this browser, and a
    /// timeout of a far browser is sent once more.
    pub(super) fn read_call(&self) -> call::Call {
        call::Call::of(&self.config, call::Kind::Read)
    }

    fn send_page_command(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, CdpError> {
        let budget = self.call_budget();
        self.send_page_command_within(link, event_tx, frame_slot, method, params, budget)
    }

    /// A page-session command that may block the driver for at most `timeout`.
    pub(super) fn send_page_command_within(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        method: &str,
        params: &serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, CdpError> {
        let Some(session) = self.session_id.clone() else {
            return Err(CdpError::NoPageSession {
                method: method.to_string(),
            });
        };
        if method.starts_with("Runtime.") {
            self.ensure_page_runtime(link)?;
        }
        let stop_requested = Arc::clone(&self.stop_requested);
        let outcome = link.call_and_drain_until(timeout, method, params, Some(session.as_str()), || {
            stop_requested.load(Ordering::Acquire)
        });
        for message in outcome.drained {
            self.handle_message(link, event_tx, frame_slot, message);
        }
        outcome.result
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
        true
    }

    fn forget_runtime_session(&mut self, session: &str) {
        forget_tracked_runtime_session(
            &mut self.runtime_enable_requested,
            &mut self.runtime_enable_inflight,
            session,
        );
    }

    fn reset_runtime_enable_state(&mut self) {
        self.runtime_enable_requested.clear();
        self.runtime_enable_inflight.clear();
        self.clipboard.pending_capture = false;
        // Passkey caller contexts belong to the Runtime domain of the session.
        self.reset_passkeys();
    }

    fn navigate_to(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        url: &str,
    ) -> Result<(), BrowserControlFailure> {
        let url = normalize_navigation_target(url);
        let Some(session) = self.session_id.clone() else {
            let error = CdpError::NoPageSession {
                method: "Page.navigate".to_string(),
            };
            self.fail_navigation(event_tx, &format!("could not navigate to {url}: {error}"));
            return Err(BrowserControlFailure::new("protocol_error", error.to_string()));
        };
        let _ = frame_slot;
        self.challenge_loop.document_navigation_started();
        self.retain_frame_during_navigation = true;
        self.navigation_failed = false;
        // `Page.navigate` replies only once the destination starts
        // responding, so it is sent without waiting: a slow server must not
        // stall frames, input, or coordination. The reply is consumed by
        // `handle_navigate_response`; the committed URL remains authoritative
        // and arrives via a navigation event.
        match link.send_request("Page.navigate", &serde_json::json!({ "url": &url }), Some(&session)) {
            Ok(request_id) => {
                self.navigate_request_id = Some(request_id);
                self.pending_restart_at = Some(Instant::now());
                let _ = event_tx.send(BrowserEvent::Loading(true));
                Ok(())
            }
            Err(error) => {
                self.fail_navigation(event_tx, &format!("could not navigate to {url}: {error}"));
                Err(BrowserControlFailure::new("protocol_error", error.to_string()))
            }
        }
    }

    /// Consume the asynchronous `Page.navigate` reply: a transport error or
    /// a non-empty `errorText` is a failed navigation.
    pub(super) fn handle_navigate_response(
        &mut self,
        event_tx: &BrowserEventSender,
        id: u64,
        result: Option<&serde_json::Value>,
        error: Option<&crate::cdp::CdpErrorInfo>,
    ) -> bool {
        if self.navigate_request_id != Some(id) {
            return false;
        }
        self.navigate_request_id = None;
        let failure = error.map(ToString::to_string).or_else(|| {
            result
                .and_then(|value| value.get("errorText"))
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        });
        if let Some(failure) = failure {
            self.fail_navigation(event_tx, &format!("could not navigate: {failure}"));
            return true;
        }
        let loader_id = result
            .and_then(|value| value.get("loaderId"))
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty());
        self.attach_navigation_id(loader_id);
        true
    }

    fn fail_navigation(&mut self, event_tx: &BrowserEventSender, message: &str) {
        self.navigation_failed = true;
        self.top_frame_navigating = false;
        self.interaction_started_at = None;
        let _ = event_tx.send(BrowserEvent::NavigationFailed(message.to_string()));
        let _ = event_tx.send(BrowserEvent::Loading(false));
        self.observe_navigation_signal(crate::navigation::NavigationSignal::Failed { message, id: None });
    }
}

fn forget_tracked_runtime_session(requested: &mut HashSet<String>, inflight: &mut HashMap<u64, String>, session: &str) {
    requested.remove(session);
    inflight.retain(|_, tracked| tracked != session);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::is_user_activity;

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
        assert!(!is_user_activity(&BrowserCommand::Video {
            operation: crate::BrowserVideoOperation::Start,
            options: None,
        }));
    }

    #[test]
    fn forgetting_a_runtime_session_drops_requested_and_inflight_entries() {
        let mut requested = HashSet::from(["page".to_string(), "oopif".to_string()]);
        let mut inflight = HashMap::from([(7, "oopif".to_string()), (8, "page".to_string())]);

        forget_tracked_runtime_session(&mut requested, &mut inflight, "oopif");

        assert_eq!(requested, HashSet::from(["page".to_string()]));
        assert_eq!(inflight, HashMap::from([(8, "page".to_string())]));
    }
}
