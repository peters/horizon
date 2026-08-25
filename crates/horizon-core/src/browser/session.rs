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
mod commands;
mod events;
mod lifecycle;
mod manifest_io;
mod startup;

use clipboard::ClipboardState;
pub(super) use startup::profile_dir;
use startup::run_driver;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::browser::cdp::{CdpError, CdpLink};
use crate::browser::frames::FrameSlot;
use crate::browser::process::{ChromeProcess, ChromeProcessControl};
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
    committed_url: CommittedUrl,
    process_control: ChromeProcessControl,
    panel_local_id: String,
}

/// Completion signal paired with an exact Chrome child handle. Normal paths
/// only poll the receiver; a hard application-exit deadline can explicitly
/// terminate/reap the owned process and remove its dead manifest.
pub struct BrowserShutdownSignal {
    completion_rx: mpsc::Receiver<()>,
    driver_complete: AtomicBool,
    process_complete: AtomicBool,
    process_control: ChromeProcessControl,
    panel_local_id: Option<String>,
    profile_cleanup: Mutex<ProfileCleanupState>,
}

enum ProfileCleanupState {
    NotRequired,
    Pending(std::path::PathBuf),
    Running {
        profile_dir: std::path::PathBuf,
        completion_rx: mpsc::Receiver<std::io::Result<()>>,
    },
    RetryPending {
        profile_dir: std::path::PathBuf,
        retry_at: Instant,
    },
}

const PROFILE_CLEANUP_RETRY_DELAY: Duration = Duration::from_secs(1);

impl BrowserShutdownSignal {
    pub(crate) fn with_profile_cleanup(self, profile_dir: std::path::PathBuf) -> Self {
        *self
            .profile_cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ProfileCleanupState::Pending(profile_dir);
        self
    }

    #[must_use]
    pub fn completed_with_profile_cleanup(profile_dir: std::path::PathBuf) -> Self {
        let (completion_tx, completion_rx) = mpsc::channel();
        drop(completion_tx);
        let process_control = ChromeProcessControl::default();
        process_control.mark_registration_settled();
        Self {
            completion_rx,
            driver_complete: AtomicBool::new(true),
            process_complete: AtomicBool::new(true),
            process_control,
            panel_local_id: None,
            profile_cleanup: Mutex::new(ProfileCleanupState::Pending(profile_dir)),
        }
    }

    #[must_use]
    pub(crate) fn is_complete(&self) -> bool {
        self.process_is_complete() && self.profile_cleanup_is_complete()
    }

    #[must_use]
    pub(crate) fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        if !self.wait_driver_completion(timeout) {
            return false;
        }
        while !self.process_control.is_reaped() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !self.process_control.is_reaped() {
            return false;
        }
        self.process_complete.store(true, Ordering::Release);
        self.wait_for_profile_cleanup(deadline.saturating_duration_since(Instant::now()))
    }

    /// Emergency cleanup after the normal driver deadline. Returns only after
    /// Chrome has been reaped (or no child was ever spawned) and any permanent
    /// panel-close profile removal has finished.
    #[must_use]
    pub(crate) fn force_cleanup(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        if !self
            .process_control
            .terminate(deadline.saturating_duration_since(Instant::now()))
        {
            return false;
        }
        // The driver may still be running between Chrome's death and its own
        // exit; wait for it before removing files it owns, or it could
        // recreate the manifest after we delete it.
        if !self.wait_driver_completion(deadline.saturating_duration_since(Instant::now())) {
            return false;
        }
        if let Some(panel_local_id) = &self.panel_local_id
            && !crate::browser::manifest::remove_with_timeout(
                panel_local_id,
                deadline.saturating_duration_since(Instant::now()),
            )
        {
            return false;
        }
        self.process_complete.store(true, Ordering::Release);
        self.retry_failed_profile_cleanup_now();
        self.wait_for_profile_cleanup(deadline.saturating_duration_since(Instant::now()))
    }

    /// Block until the driver thread has exited (its completion signal
    /// fires) or the timeout elapses.
    fn wait_driver_completion(&self, timeout: Duration) -> bool {
        if self.driver_complete.load(Ordering::Acquire) {
            return true;
        }
        let complete = matches!(
            self.completion_rx.recv_timeout(timeout),
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected)
        );
        if complete {
            self.driver_complete.store(true, Ordering::Release);
        }
        complete
    }

    fn process_is_complete(&self) -> bool {
        if self.process_complete.load(Ordering::Acquire) {
            return true;
        }
        let driver_complete = self.driver_complete.load(Ordering::Acquire)
            || matches!(
                self.completion_rx.try_recv(),
                Ok(()) | Err(mpsc::TryRecvError::Disconnected)
            );
        if driver_complete {
            self.driver_complete.store(true, Ordering::Release);
        }
        let complete = driver_complete && self.process_control.is_reaped();
        if complete {
            self.process_complete.store(true, Ordering::Release);
        }
        complete
    }

    fn profile_cleanup_is_complete(&self) -> bool {
        let mut cleanup = self
            .profile_cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        poll_profile_cleanup(&mut cleanup, None)
    }

    fn wait_for_profile_cleanup(&self, timeout: Duration) -> bool {
        let mut cleanup = self
            .profile_cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        poll_profile_cleanup(&mut cleanup, Some(timeout))
    }

    fn retry_failed_profile_cleanup_now(&self) {
        let mut cleanup = self
            .profile_cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::mem::replace(&mut *cleanup, ProfileCleanupState::NotRequired);
        *cleanup = match previous {
            ProfileCleanupState::RetryPending { profile_dir, .. } => ProfileCleanupState::Pending(profile_dir),
            other => other,
        };
    }

    #[cfg(test)]
    pub(crate) fn for_test(completion_rx: mpsc::Receiver<()>) -> Self {
        let process_control = ChromeProcessControl::default();
        process_control.mark_registration_settled();
        Self {
            completion_rx,
            driver_complete: AtomicBool::new(false),
            process_complete: AtomicBool::new(false),
            process_control,
            panel_local_id: None,
            profile_cleanup: Mutex::new(ProfileCleanupState::NotRequired),
        }
    }
}

fn start_profile_cleanup(cleanup: &mut ProfileCleanupState) {
    let previous = std::mem::replace(cleanup, ProfileCleanupState::NotRequired);
    let profile_dir = match previous {
        ProfileCleanupState::Pending(profile_dir) => profile_dir,
        ProfileCleanupState::RetryPending { profile_dir, retry_at } if Instant::now() >= retry_at => profile_dir,
        other => {
            *cleanup = other;
            return;
        }
    };
    let completion_rx = super::schedule_profile_removal(profile_dir.clone());
    *cleanup = ProfileCleanupState::Running {
        profile_dir,
        completion_rx,
    };
}

fn poll_profile_cleanup(cleanup: &mut ProfileCleanupState, timeout: Option<Duration>) -> bool {
    start_profile_cleanup(cleanup);
    let outcome = match cleanup {
        ProfileCleanupState::NotRequired => return true,
        ProfileCleanupState::Pending(_) | ProfileCleanupState::RetryPending { .. } => return false,
        ProfileCleanupState::Running { completion_rx, .. } => match timeout {
            Some(timeout) => match completion_rx.recv_timeout(timeout) {
                Ok(result) => Some(result),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => Some(Err(std::io::Error::other(
                    "browser profile cleanup worker disconnected",
                ))),
            },
            None => match completion_rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(std::io::Error::other(
                    "browser profile cleanup worker disconnected",
                ))),
            },
        },
    };
    let Some(outcome) = outcome else {
        return false;
    };
    match outcome {
        Ok(()) => {
            *cleanup = ProfileCleanupState::NotRequired;
            true
        }
        Err(error) => {
            let ProfileCleanupState::Running { profile_dir, .. } =
                std::mem::replace(cleanup, ProfileCleanupState::NotRequired)
            else {
                return false;
            };
            tracing::warn!(path = %profile_dir.display(), "failed to remove browser profile: {error}");
            *cleanup = ProfileCleanupState::RetryPending {
                profile_dir,
                retry_at: Instant::now() + PROFILE_CLEANUP_RETRY_DELAY,
            };
            false
        }
    }
}

/// Latest URL that the driver has observed Chrome commit. This survives the
/// event receiver being dropped during shutdown so the panel can persist the
/// driver's final state after teardown completes.
#[derive(Clone, Default)]
pub(super) struct CommittedUrl(Arc<Mutex<Option<String>>>);

impl CommittedUrl {
    pub(super) fn publish(&self, url: &str) {
        *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(url.to_string());
    }

    pub(super) fn snapshot(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
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
    committed_url: CommittedUrl,
}

impl BrowserEventSender {
    fn send(&self, event: BrowserEvent) -> Result<(), mpsc::SendError<BrowserEvent>> {
        if let BrowserEvent::UrlChanged(url) = &event {
            self.committed_url.publish(url);
        }
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
    pub(crate) fn shutdown_signal(self) -> BrowserShutdownSignal {
        self.stop_requested.store(true, Ordering::Release);
        let _ = self.command_tx.send(BrowserCommand::Stop);
        self.frame_slot.release_notification();
        BrowserShutdownSignal {
            completion_rx: self.completion_rx,
            driver_complete: AtomicBool::new(false),
            process_complete: AtomicBool::new(false),
            process_control: self.process_control,
            panel_local_id: Some(self.panel_local_id),
            profile_cleanup: Mutex::new(ProfileCleanupState::NotRequired),
        }
    }

    /// Return the existing teardown-completion signal after the driver has
    /// already announced `Stopped`; no second stop request is necessary.
    #[must_use]
    pub(crate) fn completion_signal(self) -> BrowserShutdownSignal {
        self.frame_slot.release_notification();
        BrowserShutdownSignal {
            completion_rx: self.completion_rx,
            driver_complete: AtomicBool::new(false),
            process_complete: AtomicBool::new(false),
            process_control: self.process_control,
            panel_local_id: Some(self.panel_local_id),
            profile_cleanup: Mutex::new(ProfileCleanupState::NotRequired),
        }
    }

    pub(super) fn committed_url(&self) -> CommittedUrl {
        self.committed_url.clone()
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
const VIEWPORT_RETRY_DELAY: Duration = Duration::from_millis(100);
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
/// The launch config is resolved on the calling thread (profile root
/// anchoring, quality clamping) so direct callers of this low-level entry
/// point behave identically to `BrowserPanelState::start` instead of
/// resolving a relative profile root against the driver thread's working
/// directory.
///
/// # Errors
/// Fails when the launch config cannot be resolved or the OS thread cannot
/// be spawned.
pub fn start_session(config: BrowserSessionConfig) -> Result<BrowserSession, String> {
    let mut config = config;
    config.browser = config.browser.resolved_for_launch().map_err(|err| err.to_string())?;
    let frame_slot = config.frame_slot.clone();
    let (command_tx, command_rx) = mpsc::channel::<BrowserCommand>();
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
                driver_process_control,
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
        committed_url,
        process_control,
        panel_local_id,
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
                let _ = chrome.kill();
                let _ = event_tx.send(BrowserEvent::Stopped { code: None });
                break;
            }
        }

        // 3. Flush a pending throttled manifest write (the loop always
        //    iterates, so a quiet page still gets its url/title flushed).
        state.write_manifest(false);

        // 3b. Delayed post-load title fetch.
        state.tick_title_fetch(link, event_tx, frame_slot);

        // 3c. Retry a transiently rejected viewport command, then capture one
        //     fresh frame after the resize burst settles.
        state.tick_viewport_resize(link, event_tx, frame_slot);
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
    pending_viewport: Option<(u32, u32)>,
    viewport_retry_at: Option<Instant>,
    pending_viewport_capture_at: Option<Instant>,
    viewport_capture_request_id: Option<u64>,
    clipboard: ClipboardState,
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
    handoff_seen: Option<String>,
    stop_requested: Arc<AtomicBool>,
}

impl DriverState {
    fn new(config: &BrowserSessionConfig, browser_ws: &str, stop_requested: Arc<AtomicBool>) -> Self {
        Self {
            config: config.clone(),
            browser_ws: browser_ws.to_string(),
            session_id: None,
            target_id: None,
            main_frame_id: None,
            viewport_w: config.width,
            viewport_h: config.height,
            pending_viewport: None,
            viewport_retry_at: None,
            pending_viewport_capture_at: None,
            viewport_capture_request_id: None,
            clipboard: ClipboardState::default(),
            // The requested initial URL is not committed state. Chrome starts
            // at about:blank and navigation may fail or be cancelled.
            url: String::new(),
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

fn normalized_committed_url(url: &str) -> &str {
    if url.is_empty() || url == "about:blank" {
        ""
    } else {
        url
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
        let sender = BrowserEventSender {
            tx,
            wake,
            committed_url: CommittedUrl::default(),
        };

        assert!(sender.send(BrowserEvent::Ready).is_ok());
        assert_eq!(rx.recv(), Ok(BrowserEvent::Ready));
        assert_eq!(wake_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn committed_url_survives_a_dropped_event_receiver() {
        let committed_url = CommittedUrl::default();
        let (tx, rx) = mpsc::channel();
        let sender = BrowserEventSender {
            tx,
            wake: BrowserEventWake::default(),
            committed_url: committed_url.clone(),
        };
        drop(rx);

        assert!(
            sender
                .send(BrowserEvent::UrlChanged("https://example.com/final".to_string()))
                .is_err()
        );
        assert_eq!(committed_url.snapshot().as_deref(), Some("https://example.com/final"));
    }

    #[test]
    fn blank_target_is_an_empty_committed_url() {
        assert_eq!(normalized_committed_url("about:blank"), "");
        assert_eq!(normalized_committed_url(""), "");
        assert_eq!(normalized_committed_url("https://example.com"), "https://example.com");
    }
}
