//! The panel side of the driver that runs beside the browser of a browser host.
//!
//! The panel of a Firefox browser host has no local browser session: the session
//! is on the other machine. What this module owns is the link: one worker thread
//! opens it over `ssh`, hands the pushed pictures to the frame slot of the panel,
//! sends the input of the person and the operations that an agent asked for, and
//! attaches again by itself after a link is lost.
//!
//! The promise of a Firefox host is in the welcome of a link: a link that comes
//! back says that it is the same page, so the text that a person typed is still
//! there. A page that is gone is a new one, and the panel says so in words.
//!
//! The manifest of an agent lives on this computer, so the worker carries its
//! facts to the driver of the host and writes back what the driver learned: the
//! panel of a host looks to an agent like a panel of this computer, which is what
//! the tools of an agent rely on.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use horizon_browser::host_driver::{HostDriverPage, HostDriverReply, HostDriverRequest, HostDriverSignals};
use horizon_browser::{BrowserCommand, BrowserCoordination, BrowserEventWaker, CoordinationState, FrameSlot};
use horizon_browser_control::manifest::ManifestCoordination;
use horizon_browser_control::ssh_forward::Ssh;

use super::HostDriverEngine;
use super::client::{DriverLink, HostDriverHost};
use crate::RECONNECT_RESET_AFTER;
use crate::browser::attached_host::{AttachedHostFailure, PAGE_STATE_LOST};
use crate::browser::{BrowserPanelState, BrowserStatus};
use crate::reconnect_delay;

/// How long one pass of the worker waits for a command of the panel before it
/// looks at the link again.
const PASS_WAIT: Duration = Duration::from_millis(20);

/// How often the worker asks the manifest of this computer for the signals of an
/// agent. The local driver of a panel asks on the same order, and a person does
/// not notice a quarter of a second in a handoff.
const SIGNALS_INTERVAL: Duration = Duration::from_millis(250);

/// The quality of a pushed picture when the settings name none.
pub(super) const DEFAULT_QUALITY: u32 = horizon_browser::host_driver::DEFAULT_HOST_FRAME_QUALITY;

/// What the worker of a host-driver panel reports to the panel.
#[derive(Debug)]
pub enum HostDriverEvent {
    /// The link is up. `resumed` says whether the page is the one of the link
    /// before a lost one.
    Ready { resumed: bool, page: HostDriverPage },
    /// The address, title or loading state of the page changed.
    Page(HostDriverPage),
    /// One plain sentence for the person, such as a page that is not ready.
    Note(String),
    /// The link is lost and the worker waits for its next attempt.
    Reconnecting { attempt: u32, retry_at: Instant },
    /// The link cannot work without work on the host, so the panel waits for
    /// Retry with this reason.
    Failed(AttachedHostFailure),
    /// The worker stopped.
    Stopped,
}

/// One panel's link to the driver of its browser host.
pub(crate) struct HostDriverSlot {
    host: crate::browser::AttachedHost,
    ssh: Ssh,
    commands: mpsc::Sender<(BrowserCommand, Instant)>,
    events: mpsc::Receiver<HostDriverEvent>,
    waker: Arc<Mutex<Option<BrowserEventWaker>>>,
    stop: Arc<AtomicBool>,
    /// The last failure that ended the worker, for the words of the panel.
    failure: Option<AttachedHostFailure>,
    /// Whether the link reached its page at least once, so a loss of it is a
    /// page that the person had.
    reached_ready: bool,
    attempts: u32,
    ready_at: Option<Instant>,
    /// The worker of this link, so a stop waits for it to end.
    worker: Option<std::thread::JoinHandle<()>>,
}

impl HostDriverSlot {
    /// Start the worker that opens the link to `host`. The call returns at once;
    /// the outcome arrives as an event of the panel.
    pub(super) fn connecting(
        ssh: Ssh,
        host: crate::browser::AttachedHost,
        panel_local_id: String,
        frame_slot: Arc<FrameSlot>,
        quality: u32,
    ) -> Self {
        let (commands, command_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let waker: Arc<Mutex<Option<BrowserEventWaker>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_waker = Arc::clone(&waker);
        let worker_host = host.clone();
        let worker_ssh = ssh.clone();
        let worker = std::thread::Builder::new()
            .name("browser-host-driver".to_string())
            .spawn(move || {
                run(
                    worker_ssh,
                    worker_host,
                    &panel_local_id,
                    frame_slot,
                    quality,
                    command_rx,
                    event_tx,
                    worker_stop,
                    worker_waker,
                );
            })
            .ok();
        let mut slot = Self {
            host,
            ssh,
            commands,
            events,
            waker,
            stop,
            failure: None,
            reached_ready: false,
            attempts: 0,
            ready_at: None,
            worker,
        };
        if slot.worker.is_none() {
            slot.failure = Some(AttachedHostFailure::closed(&slot.host));
        }
        slot
    }

    pub(super) fn host(&self) -> &crate::browser::AttachedHost {
        &self.host
    }

    /// The `ssh` program of this panel, for the link that a Retry starts.
    pub(super) fn ssh(&self) -> &Ssh {
        &self.ssh
    }

    /// The next event of this link, when one arrived.
    pub(super) fn next_event(&self) -> Option<HostDriverEvent> {
        self.events.try_recv().ok()
    }

    /// Send one command of the panel to the driver of the host.
    pub(super) fn send(&self, command: BrowserCommand) -> bool {
        self.commands.send((command, Instant::now())).is_ok()
    }

    /// Whether this link takes a command now: the worker exists and has not
    /// stopped.
    pub(super) fn takes_commands(&self) -> bool {
        self.worker.is_some() && !self.stop.load(Ordering::Acquire)
    }

    pub(super) fn set_waker(&self, waker: BrowserEventWaker) {
        *lock(&self.waker) = Some(waker);
    }

    pub(super) fn needs_waker(&self) -> bool {
        self.worker.is_some()
    }

    /// Stop the worker of this link. The browser and the page of the host keep
    /// running, so a panel that comes back attaches to the same page.
    pub(super) fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // The commands of a stopped panel must not reach a page that no person
        // is looking at any more.
        let (commands, _) = mpsc::channel();
        self.commands = commands;
    }

    /// Mark that this link showed its page, so a loss of it is a page that the
    /// person had.
    pub(super) fn reached_ready(&mut self) {
        self.reached_ready = true;
        self.ready_at = Some(Instant::now());
    }

    pub(super) fn failure(&self) -> Option<&AttachedHostFailure> {
        self.failure.as_ref()
    }

    pub(super) fn set_failure(&mut self, failure: Option<AttachedHostFailure>) {
        self.failure = failure;
    }

    pub(super) fn set_attempts(&mut self, attempts: u32) {
        self.attempts = attempts;
    }

    pub(super) fn reached_page(&self) -> bool {
        self.reached_ready
    }
}

fn lock<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The worker of one panel's link: open, serve, attach again.
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn run(
    ssh: Ssh,
    host: crate::browser::AttachedHost,
    panel_local_id: &str,
    frame_slot: Arc<FrameSlot>,
    quality: u32,
    commands: mpsc::Receiver<(BrowserCommand, Instant)>,
    events: mpsc::Sender<HostDriverEvent>,
    stop: Arc<AtomicBool>,
    waker: Arc<Mutex<Option<BrowserEventWaker>>>,
) {
    let Some(driver) = host.driver() else {
        let _ = events.send(HostDriverEvent::Failed(AttachedHostFailure::refused(
            &host,
            "host_driver_engine",
            "a Chromium browser host is reached through its DevTools port and needs no driver",
        )));
        return;
    };
    let driver_host =
        match HostDriverHost::new(driver.ssh_target(), driver.profile(), driver.engine(), driver.command()) {
            Ok(driver_host) => driver_host,
            Err(error) => {
                let _ = events.send(HostDriverEvent::Failed(AttachedHostFailure::refused(
                    &host,
                    "host_driver_name",
                    &error.to_string(),
                )));
                return;
            }
        };
    let manifest = ManifestCoordination::default();
    let mut probe = super::latency::LinkProbe::new();
    let mut attempt = 0u32;
    let mut ready_at: Option<Instant> = None;
    loop {
        if stop.load(Ordering::Acquire) {
            let _ = events.send(HostDriverEvent::Stopped);
            return;
        }
        match DriverLink::open(&ssh, &driver_host, quality) {
            Ok(mut link) => {
                let resumed = link.welcome().page.resumed();
                let page = link.initial_page().cloned().unwrap_or_else(|| HostDriverPage {
                    resumed,
                    loading: true,
                    ..HostDriverPage::default()
                });
                let _ = events.send(HostDriverEvent::Ready { resumed, page });
                wake(&waker);
                ready_at = Some(Instant::now());
                match serve(
                    &mut link,
                    panel_local_id,
                    &frame_slot,
                    &commands,
                    &events,
                    &stop,
                    &waker,
                    &manifest,
                    &mut probe,
                ) {
                    PumpOutcome::Stopped => {
                        let _ = events.send(HostDriverEvent::Stopped);
                        return;
                    }
                    PumpOutcome::Lost(reason) => {
                        tracing::info!(
                            target = host.ssh_target(),
                            profile = host.profile(),
                            reason = reason.as_deref().unwrap_or_default(),
                            "the link to the browser host driver is lost, so the panel attaches again"
                        );
                    }
                }
            }
            Err(failure) => {
                // A link that never worked needs a person, unless this panel is
                // already trying again by itself and the host was merely
                // unreachable: then one more attempt is what a cut link expects.
                let retry = attempt > 0 && failure.kind() == crate::browser::AttachedHostFailureKind::Unreachable;
                if !retry {
                    let _ = events.send(HostDriverEvent::Failed(failure));
                    wake(&waker);
                    return;
                }
            }
        }
        if stop.load(Ordering::Acquire) {
            let _ = events.send(HostDriverEvent::Stopped);
            return;
        }
        if ready_at.is_some_and(|ready_at| ready_at.elapsed() >= RECONNECT_RESET_AFTER) {
            ready_at = None;
            attempt = 0;
        }
        attempt = attempt.saturating_add(1);
        let delay = reconnect_delay(attempt);
        let _ = events.send(HostDriverEvent::Reconnecting {
            attempt,
            retry_at: Instant::now() + delay,
        });
        wake(&waker);
        if !wait_unless_stopped(&stop, delay) {
            let _ = events.send(HostDriverEvent::Stopped);
            return;
        }
    }
}

fn wake(waker: &Arc<Mutex<Option<BrowserEventWaker>>>) {
    if let Some(waker) = lock(waker).as_ref() {
        waker();
    }
}

fn wait_unless_stopped(stop: &AtomicBool, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        std::thread::sleep(remaining.min(PASS_WAIT));
    }
}

/// Why the pump of one link ended.
enum PumpOutcome {
    Stopped,
    Lost(Option<String>),
}

/// One live link: the commands of the panel out, the answers of the driver in.
#[allow(clippy::too_many_arguments)]
fn serve(
    link: &mut DriverLink,
    panel_local_id: &str,
    frame_slot: &FrameSlot,
    commands: &mpsc::Receiver<(BrowserCommand, Instant)>,
    events: &mpsc::Sender<HostDriverEvent>,
    stop: &AtomicBool,
    waker: &Arc<Mutex<Option<BrowserEventWaker>>>,
    manifest: &ManifestCoordination,
    probe: &mut super::latency::LinkProbe,
) -> PumpOutcome {
    let mut last_signals = Instant::now();
    loop {
        let pass_started = Instant::now();
        if stop.load(Ordering::Acquire) {
            return PumpOutcome::Stopped;
        }
        // The input of the person, and the operations that the tools of an agent
        // asked for, reach the page of the host in their order.
        match commands.recv_timeout(PASS_WAIT) {
            Ok((BrowserCommand::Stop, _)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return PumpOutcome::Stopped;
            }
            Ok((command, queued_at)) => {
                let method = request_method(&command);
                if let Some(request) = request_of(command) {
                    probe.queued(queued_at);
                    probe.taken();
                    let written = link.request(&request);
                    probe.written(method);
                    if written.is_err() {
                        return PumpOutcome::Lost(None);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if last_signals.elapsed() >= SIGNALS_INTERVAL {
            last_signals = Instant::now();
            if let Ok(signals) = manifest.signals(panel_local_id) {
                let signals = HostDriverSignals {
                    owner: signals.owner,
                    handoff: signals
                        .handoff
                        .map(|handoff| horizon_browser::host_driver::HostDriverHandoff {
                            request_id: handoff.request_id,
                            reason: handoff.reason,
                        }),
                    actions: signals.actions,
                };
                if link.request(&HostDriverRequest::Signals { signals }).is_err() {
                    return PumpOutcome::Lost(None);
                }
            }
        }
        match link.recv(Duration::ZERO) {
            Ok(reply) => {
                let received_at = Instant::now();
                if !apply_reply(
                    link,
                    reply,
                    panel_local_id,
                    frame_slot,
                    events,
                    waker,
                    manifest,
                    probe,
                    received_at,
                ) {
                    return PumpOutcome::Lost(None);
                }
            }
            Err(None) => {}
            Err(Some(failure)) => {
                let _ = events.send(HostDriverEvent::Failed(failure));
                return PumpOutcome::Lost(None);
            }
        }
        if probe.enabled() {
            let waited = link.take_pass_wait();
            probe.stall(
                pass_started.elapsed(),
                waited.as_ref().map(|(name, waited)| (*name, *waited)),
            );
        }
    }
}

/// The name of the wait that one command of the panel writes, for a stall line.
fn request_method(command: &BrowserCommand) -> &'static str {
    match command {
        BrowserCommand::Input(_) => "input",
        BrowserCommand::Navigate(_) => "navigate",
        BrowserCommand::Reload => "reload",
        BrowserCommand::Back => "back",
        BrowserCommand::Forward => "forward",
        BrowserCommand::SetViewport { .. } => "viewport",
        BrowserCommand::DropFiles { .. }
        | BrowserCommand::Video { .. }
        | BrowserCommand::Orientation { .. }
        | BrowserCommand::NativeSelectChoose { .. }
        | BrowserCommand::NativeSelectDismiss
        | BrowserCommand::HandoffDone
        | BrowserCommand::Stop => "other",
    }
}

/// Apply one answer of the driver. Returns whether the link may go on.
#[allow(clippy::too_many_arguments)]
fn apply_reply(
    link: &mut DriverLink,
    reply: HostDriverReply,
    panel_local_id: &str,
    frame_slot: &FrameSlot,
    events: &mpsc::Sender<HostDriverEvent>,
    waker: &Arc<Mutex<Option<BrowserEventWaker>>>,
    manifest: &ManifestCoordination,
    probe: &mut super::latency::LinkProbe,
    received_at: Instant,
) -> bool {
    match reply {
        HostDriverReply::Frame(frame) => {
            let decoded_at = Instant::now();
            let ack = link.accept_frame(frame_slot, &frame);
            probe.frame_handed(
                frame.seq,
                frame.data.len(),
                (frame.width, frame.height),
                received_at,
                decoded_at,
                true,
            );
            if let Some(ack) = ack
                && link.request(&ack).is_err()
            {
                return false;
            }
            wake(waker);
        }
        HostDriverReply::Page(page) => {
            let _ = events.send(HostDriverEvent::Page(page));
            wake(waker);
        }
        HostDriverReply::Refusal { message, fatal, .. } => {
            if fatal {
                return false;
            }
            let _ = events.send(HostDriverEvent::Note(message));
            wake(waker);
        }
        HostDriverReply::Coordination(state) => {
            let state = CoordinationState {
                backend: horizon_browser::BackendKind::FirefoxBidi,
                automation_disclosure: state.automation_disclosure,
                browser_ws: String::new(),
                target_id: state.target_id,
                page_resumed: state.page_resumed,
                url: state.url,
                title: state.title,
                remote_target: None,
                remote_device: None,
                remote_file_upload: false,
                remote_orientation: None,
                file_chooser: state.file_chooser,
                passkey_pending: state.passkey_pending,
            };
            // A manifest that this computer cannot write is a reason to stop a
            // link: an agent would drive a page whose owner nobody records.
            if update_manifest(manifest, panel_local_id, &state).is_err() {
                return false;
            }
        }
        HostDriverReply::Action(result) => {
            if manifest.complete_action(panel_local_id, &result.result).is_err() {
                return false;
            }
        }
        HostDriverReply::Audit(entry) => {
            if manifest.record_action(panel_local_id, &entry).is_err() {
                return false;
            }
        }
        HostDriverReply::HandoffAck { request_id } => {
            let _ = manifest.acknowledge_handoff(panel_local_id, &request_id);
        }
        HostDriverReply::Welcome(_) => {}
        HostDriverReply::Closed => return false,
    }
    true
}

/// Write the state of the page of the host into the manifest of this computer.
fn update_manifest(
    manifest: &ManifestCoordination,
    panel_local_id: &str,
    state: &CoordinationState,
) -> std::io::Result<()> {
    // The first state of a panel of a host initializes the entry; a later one
    // updates it. Both are an update for a manifest that already holds the
    // entry, and `initialize` is what writes the fields that a driver owns.
    match manifest.update(panel_local_id, state) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => manifest.initialize(panel_local_id, state),
        Err(error) => Err(error),
    }
}

/// The request of this computer that one command of the panel becomes, when the
/// link carries it.
fn request_of(command: BrowserCommand) -> Option<HostDriverRequest> {
    match command {
        BrowserCommand::Navigate(url) => Some(HostDriverRequest::Navigate { url }),
        BrowserCommand::Reload => Some(HostDriverRequest::Reload),
        BrowserCommand::Back => Some(HostDriverRequest::Back),
        BrowserCommand::Forward => Some(HostDriverRequest::Forward),
        BrowserCommand::SetViewport { width, height } => Some(HostDriverRequest::Viewport { width, height }),
        BrowserCommand::Input(input) => Some(HostDriverRequest::Input { input }),
        // A page of another machine cannot take a file of this one, and the
        // panel says so where the command arrives; the operations that a host
        // driver does not serve are those of a browser that is not here.
        BrowserCommand::DropFiles { .. }
        | BrowserCommand::Video { .. }
        | BrowserCommand::Orientation { .. }
        | BrowserCommand::NativeSelectChoose { .. }
        | BrowserCommand::NativeSelectDismiss
        | BrowserCommand::HandoffDone
        | BrowserCommand::Stop => None,
    }
}

impl BrowserPanelState {
    /// Create a panel whose Firefox runs on a browser host, and start the worker
    /// that opens the link to the driver beside it.
    ///
    /// This call returns at once: the `ssh` connect, the authentication and the
    /// start of the driver on that machine happen on the worker, so a host that
    /// drops packets never holds the caller. The outcome is a state of the panel.
    #[must_use]
    pub fn start_host_driver(
        panel_local_id: impl Into<String>,
        config: &super::super::BrowserConfig,
        initial_url: Option<String>,
        host: &crate::browser::AttachedHost,
    ) -> Self {
        Self::start_host_driver_with(Ssh::default(), panel_local_id, config, initial_url, host)
    }

    /// [`Self::start_host_driver`] with the `ssh` of a test.
    #[doc(hidden)]
    #[must_use]
    pub fn start_host_driver_with(
        ssh: Ssh,
        panel_local_id: impl Into<String>,
        config: &super::super::BrowserConfig,
        initial_url: Option<String>,
        host: &crate::browser::AttachedHost,
    ) -> Self {
        let mut state = Self::inert();
        let panel_local_id = panel_local_id.into();
        state.panel_local_id.clone_from(&panel_local_id);
        state.config = config.clone();
        state.config.backend = horizon_browser::BackendKind::FirefoxBidi;
        state.status = BrowserStatus::Starting;
        state.loading = true;
        let requested_url = initial_url.map(|url| horizon_browser::normalize_navigation_target(&url));
        state.requested_url.clone_from(&requested_url);
        let slot = HostDriverSlot::connecting(
            ssh,
            host.clone(),
            panel_local_id,
            Arc::clone(&state.frame_slot),
            DEFAULT_QUALITY,
        );
        if let Some(failure) = slot.failure().cloned() {
            state.status = BrowserStatus::Error {
                message: failure.message().to_string(),
            };
            state.loading = false;
        }
        state.remote_status = Some(slot.note());
        state.host_driver = Some(slot);
        // The first request of a panel is the address that it was created with.
        if let Some(url) = requested_url {
            state.send(BrowserCommand::Navigate(url));
        }
        state
    }

    /// A panel of `host` that lost its link after it showed a page, for a test
    /// that has no machine to lose.
    #[doc(hidden)]
    #[must_use]
    pub fn lost_host_driver_for_tests(host: &crate::browser::AttachedHost, last_url: &str, reason: &str) -> Self {
        let mut state = Self::start_host_driver_with(
            Ssh::default(),
            "host-driver-for-tests",
            &super::super::BrowserConfig::default(),
            Some(last_url.to_string()),
            host,
        );
        state.url = Some(last_url.to_string());
        state.host_driver_ready();
        state.host_driver_lost(&AttachedHostFailure::connection_lost(host, reason));
        state
    }

    /// Whether the browser of this panel runs behind the driver of a host.
    #[must_use]
    pub fn is_host_driver(&self) -> bool {
        self.host_driver.is_some()
    }

    /// Why the last link to the driver of the host failed.
    #[must_use]
    pub fn host_driver_failure(&self) -> Option<&AttachedHostFailure> {
        self.host_driver.as_ref().and_then(HostDriverSlot::failure)
    }

    /// Take the events of the link of this panel and apply them.
    pub(crate) fn poll_host_driver(&mut self) -> bool {
        let mut changed = false;
        let mut events = Vec::new();
        if let Some(slot) = self.host_driver.as_ref() {
            while let Some(event) = slot.next_event() {
                events.push(event);
            }
        }
        for event in events {
            changed |= self.apply_host_driver_event(event);
        }
        changed
    }

    /// Whether this panel keeps its page after a lost link, so nothing cleans it
    /// up: the reason stays where the page was and Retry connects again.
    #[must_use]
    pub(crate) fn host_driver_kept_after(&self) -> bool {
        self.host_driver
            .as_ref()
            .is_some_and(|slot| slot.failure().is_some() && slot.reached_page())
    }

    pub(crate) fn host_driver_waker_needed(&self) -> bool {
        self.host_driver.as_ref().is_some_and(HostDriverSlot::needs_waker)
    }

    pub(crate) fn set_host_driver_waker(&self, waker: BrowserEventWaker) {
        if let Some(slot) = self.host_driver.as_ref() {
            slot.set_waker(waker);
        }
    }

    pub(crate) fn host_driver_takes_commands(&self) -> bool {
        self.host_driver.as_ref().is_some_and(HostDriverSlot::takes_commands)
    }

    pub(crate) fn send_host_driver_command(&self, command: BrowserCommand) -> bool {
        self.host_driver.as_ref().is_some_and(|slot| slot.send(command))
    }

    /// Retry of a panel of a browser host: a new link of the same host and
    /// profile, at the last address that this panel showed or at the navigation
    /// that a person typed while the link was down.
    pub(crate) fn relaunch_host_driver(&mut self) {
        let Some(old) = self.host_driver.take() else {
            return;
        };
        let host = old.host().clone();
        let ssh = old.ssh().clone();
        // Dropping the slot ends its worker, so a link that is still opening
        // cannot deliver to a panel that has a new one.
        drop(old);
        self.frame_slot.clear();
        self.navigation_error = None;
        self.page_resumed = false;
        self.loading = true;
        self.status = BrowserStatus::Starting;
        let target = self.relaunch_target();
        let slot = HostDriverSlot::connecting(
            ssh,
            host.clone(),
            self.panel_local_id.clone(),
            Arc::clone(&self.frame_slot),
            DEFAULT_QUALITY,
        );
        if let Some(failure) = slot.failure().cloned() {
            self.status = BrowserStatus::Error {
                message: failure.message().to_string(),
            };
            self.loading = false;
        }
        self.remote_status = Some(slot.note());
        self.host_driver = Some(slot);
        if let Some(url) = target {
            self.send(BrowserCommand::Navigate(url));
        }
    }

    /// Apply one event of the link, for a test of the panel that has no
    /// machine.
    #[doc(hidden)]
    pub fn apply_host_driver_event_for_tests(&mut self, event: HostDriverEvent) {
        self.apply_host_driver_event(event);
    }

    fn apply_host_driver_event(&mut self, event: HostDriverEvent) -> bool {
        match event {
            HostDriverEvent::Ready { resumed, page } => {
                let attempt = self.host_driver.as_ref().map_or(0, |slot| slot.attempts);
                self.host_driver_ready();
                self.apply_host_page(page);
                if attempt > 0 {
                    // An automatic attempt after a lost link: the page is either
                    // the one the person had or a new one, and a new one keeps
                    // nothing of the old.
                    self.apply_page_attached(resumed);
                    self.note_page_state_lost();
                }
                if resumed {
                    tracing::info!(
                        target = self.attached_host_target().unwrap_or_default(),
                        "the browser host driver attached to the page of the link before"
                    );
                }
            }
            HostDriverEvent::Page(page) => {
                self.loading = page.loading;
                self.apply_host_page(page);
            }
            HostDriverEvent::Note(note) => {
                self.remote_status = Some(note);
            }
            HostDriverEvent::Reconnecting { attempt, retry_at } => {
                self.status = BrowserStatus::Reconnecting { attempt, retry_at };
                self.loading = true;
                self.frame_slot.clear();
                if let Some(slot) = self.host_driver.as_mut() {
                    slot.set_attempts(attempt);
                }
            }
            HostDriverEvent::Failed(failure) => {
                self.host_driver_lost(&failure);
            }
            HostDriverEvent::Stopped => {
                self.loading = false;
            }
        }
        true
    }

    fn apply_host_page(&mut self, page: HostDriverPage) {
        if page.url.is_empty() && page.title.is_empty() {
            return;
        }
        if !page.url.is_empty() {
            self.url = Some(page.url.clone());
            self.navigation_error = None;
        }
        self.title = page.title;
        self.page_resumed = page.resumed;
        self.loading = page.loading;
    }

    fn host_driver_ready(&mut self) {
        if let Some(slot) = self.host_driver.as_mut() {
            slot.reached_ready();
            slot.set_attempts(0);
            slot.set_failure(None);
        }
        self.status = BrowserStatus::Ready;
        self.loading = false;
        self.navigation_error = None;
    }

    /// A link that ended: the panel keeps the page that it had, says why, and
    /// the worker attaches again by itself.
    fn host_driver_lost(&mut self, failure: &AttachedHostFailure) {
        let kept = self.host_driver.as_ref().is_some_and(HostDriverSlot::reached_page);
        tracing::warn!(
            code = failure.code(),
            target = self.attached_host_target().unwrap_or_default(),
            profile = self.attached_host_profile().unwrap_or_default(),
            detail = failure.detail().unwrap_or_default(),
            "a host-driver browser panel lost its link"
        );
        if let Some(slot) = self.host_driver.as_mut() {
            slot.set_failure(Some(failure.clone()));
        }
        self.frame_slot.clear();
        self.loading = false;
        if kept && failure.kind() == crate::browser::AttachedHostFailureKind::Unreachable {
            // The worker tries again by itself; the panel says that the page is
            // kept until it does.
            self.remote_status = Some(failure.message().to_string());
            return;
        }
        self.status = BrowserStatus::Error {
            message: failure.message().to_string(),
        };
        if !kept {
            self.navigation_error = None;
        }
    }

    /// The lost-link notice of a panel of a host driver, when its page is gone
    /// with the link and the next one opens a new page.
    pub(crate) fn note_page_state_lost(&mut self) {
        if self.is_host_driver() && !self.page_resumed {
            self.navigation_error = Some(PAGE_STATE_LOST.to_string());
        }
    }

    /// Stop the link of this panel. The browser and the page of the host keep
    /// running, so the next panel of that profile attaches to the same page.
    pub(crate) fn shutdown_host_driver(&mut self) {
        if let Some(slot) = self.host_driver.as_mut() {
            slot.stop();
        }
    }

    /// The SSH target of the machine whose driver runs the browser of this panel.
    #[must_use]
    pub fn host_driver_target(&self) -> Option<&str> {
        self.host_driver.as_ref().map(|slot| slot.host().ssh_target())
    }

    /// The profile of the browser of that machine.
    #[must_use]
    pub fn host_driver_profile(&self) -> Option<&str> {
        self.host_driver.as_ref().map(|slot| slot.host().profile())
    }

    /// The engine of the browser of that machine.
    #[must_use]
    pub fn host_driver_engine(&self) -> Option<HostDriverEngine> {
        self.host_driver.as_ref().and_then(|slot| slot.host().engine().driver())
    }

    /// The browser host of this panel, with the engine and the command that
    /// serve it, for the state that a restart reads back.
    #[must_use]
    pub(crate) fn attached_host_state(&self) -> Option<crate::runtime_state::BrowserHostState> {
        self.host_driver
            .as_ref()
            .map(|slot| crate::runtime_state::BrowserHostState {
                ssh_target: slot.host().ssh_target().to_string(),
                profile: Some(slot.host().profile().to_string()),
                engine: Some(slot.host().engine()),
                command: slot.host().command().map(str::to_string),
            })
    }
}

impl HostDriverSlot {
    /// The short line that the panel shows while it connects.
    pub(super) fn note(&self) -> String {
        format!(
            "Firefox of {} in the profile {}, through the driver of that machine",
            self.host.ssh_target(),
            self.host.profile()
        )
    }
}
