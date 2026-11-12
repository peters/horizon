//! The browser panel whose Chromium runs on another machine.
//!
//! A person names an SSH target and a profile in the settings entry
//! `browser_host`. A panel of that entry is the ordinary browser panel of this
//! computer: the same state, the same widget, the same input, the same frames.
//! Only the browser is elsewhere, and this computer reaches it through one
//! loopback forward (`horizon_browser_control::ssh_forward`).
//!
//! A connect reads the `DevTools` endpoint that the browser of that machine
//! wrote, starts the forward to it and asks that loopback port for its
//! WebSocket endpoint. When that fails, the browser is gone or stale, so the
//! connect starts the browser of that machine (which replaces the endpoint file
//! first) and tries once more. That order is why a panel reuses the browser that
//! an earlier panel left running, and why a dead browser is replaced instead of
//! reported forever.
//!
//! No step of a connect waits on `ssh` on the thread that asks for a panel or
//! for a retry: one worker thread of this module does the work, and the outcome
//! is a state of the panel that the placeholder paints with Retry. The browser
//! of that machine takes no part in the life of a panel: the forward dies with
//! the session, the remote Chromium does not, because a signed-in profile is the
//! reason for the host in the first place.

mod failure;
#[cfg(test)]
mod tests;

pub use failure::{AttachedHostFailure, AttachedHostFailureKind};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::{RECONNECT_RESET_AFTER, reconnect_delay};

use horizon_browser::{BackendKind, BrowserEventWaker, DevtoolsEndpoint};
use horizon_browser_control::ssh_forward::{
    Ssh, SshForward, remote_browser_command, remote_port_command, valid_profile, valid_target,
};

use super::{BrowserPanelState, BrowserStatus};

/// Bound of the read of the endpoint file of the browser host, which is one
/// `ssh` call with the connect and the authentication in it.
const PORT_READ_DEADLINE: Duration = Duration::from_secs(20);
/// Bound of the start of the browser of that machine. The snippet itself waits
/// up to 20 seconds for the endpoint that the browser writes.
const START_DEADLINE: Duration = Duration::from_secs(45);

/// The name of the profile that the browser host uses when the settings name
/// none.
pub const DEFAULT_HOST_PROFILE: &str = "default";

/// What a panel says after it opened a page again by itself. The browser of that
/// machine kept the page of its own accord, but a new connection to it starts a
/// new page, so what the person typed into the old one is gone.
pub const PAGE_STATE_LOST: &str = "The link was lost, so this page is new: text that you had typed into it, its scroll position and its session are gone.";

/// How long one wait of an automatic attempt may hold the worker before it looks
/// at the stop flag again, so a panel that closes during the wait starts no
/// `ssh` call.
const RETRY_POLL: Duration = Duration::from_millis(50);

/// The Chromium of another machine that serves the pages of a panel, and the
/// profile of that Chromium.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachedHost {
    ssh_target: String,
    profile: String,
}

impl AttachedHost {
    /// The host at `ssh_target` with `profile`, or the profile
    /// [`DEFAULT_HOST_PROFILE`] when the caller names none.
    ///
    /// # Errors
    /// `ssh_target` must be one name that `ssh` takes as one argument and not as
    /// an option of its own, and the profile must be one folder name of that
    /// machine.
    pub fn new(ssh_target: impl Into<String>, profile: Option<&str>) -> crate::error::Result<Self> {
        let ssh_target = ssh_target.into().trim().to_string();
        if !valid_target(&ssh_target) {
            return Err(crate::error::Error::Config(format!(
                "`{ssh_target}` is not one SSH name without a leading dash, a space or a control character"
            )));
        }
        let profile = profile
            .map(str::trim)
            .filter(|profile| !profile.is_empty())
            .unwrap_or(DEFAULT_HOST_PROFILE);
        if !valid_profile(profile) {
            return Err(crate::error::Error::Config(format!(
                "`{profile}` is not one profile name of a browser host: 1 to 40 of a-z, 0-9 and '-', with a letter or a digit at each end"
            )));
        }
        Ok(Self {
            ssh_target,
            profile: profile.to_string(),
        })
    }

    /// The SSH target of the host, as `ssh` resolves it.
    #[must_use]
    pub fn ssh_target(&self) -> &str {
        &self.ssh_target
    }

    /// The profile of the browser of that machine.
    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }
}

/// The browser of another machine that answers on a loopback port of this
/// computer, with the forward that carries that port.
struct AttachedBrowser {
    forward: SshForward,
    endpoint: DevtoolsEndpoint,
}

/// What one connect of a panel to its browser host found.
enum AttachOutcome {
    /// The browser of that machine answers through a fresh forward.
    Connected(AttachedBrowser),
    /// This computer could not reach it, or it did not start.
    Failed(AttachedHostFailure),
    /// The panel stopped while the connect was underway.
    Cancelled,
}

/// Read the endpoint of the browser of `host`, reuse that browser through a new
/// forward, and start it when it is gone.
fn connect(ssh: &Ssh, host: &AttachedHost, stop: &AtomicBool) -> AttachOutcome {
    if stop.load(Ordering::Acquire) {
        return AttachOutcome::Cancelled;
    }
    let target = host.ssh_target();
    let endpoint_text = match ssh.run_remote(target, &remote_port_command(host.profile()), PORT_READ_DEADLINE) {
        Ok(text) => text,
        Err(error) => return AttachOutcome::Failed(AttachedHostFailure::from_ssh(host, &error)),
    };
    if stop.load(Ordering::Acquire) {
        return AttachOutcome::Cancelled;
    }
    if let Some(remote_port) = port_of(&endpoint_text) {
        match forward_to(ssh, host, remote_port) {
            Ok(browser) => return AttachOutcome::Connected(browser),
            Err(failure) => tracing::info!(
                code = failure.code(),
                profile = host.profile(),
                "the endpoint that the browser host wrote names no browser, so this connect starts one"
            ),
        }
    }
    let started = match ssh.run_remote(target, &remote_browser_command(host.profile(), false), START_DEADLINE) {
        Ok(text) => text,
        Err(error) => return AttachOutcome::Failed(AttachedHostFailure::from_ssh(host, &error)),
    };
    if stop.load(Ordering::Acquire) {
        return AttachOutcome::Cancelled;
    }
    let Some(remote_port) = port_of(&started) else {
        let failure = AttachedHostFailure::not_started(host, "it printed no port");
        return AttachOutcome::Failed(failure);
    };
    match forward_to(ssh, host, remote_port) {
        Ok(browser) => AttachOutcome::Connected(browser),
        Err(failure) => AttachOutcome::Failed(failure),
    }
}

/// One forward to `remote_port` of the host, and the endpoint that answers
/// through it.
fn forward_to(ssh: &Ssh, host: &AttachedHost, remote_port: u16) -> Result<AttachedBrowser, AttachedHostFailure> {
    let forward = ssh
        .start_forward(host.ssh_target(), remote_port)
        .map_err(|error| AttachedHostFailure::from_ssh(host, &error))?;
    tracing::info!(
        target = host.ssh_target(),
        profile = host.profile(),
        local_port = forward.local_port(),
        "the browser of a browser host answers through a loopback forward"
    );
    let endpoint = DevtoolsEndpoint::from_port(forward.local_port()).map_err(|error| match error.waited_out() {
        // A bound that passed over a fresh forward is the link of that machine,
        // not a browser that gave the wrong answer.
        Some(seconds) => AttachedHostFailure::no_endpoint_too_slow(host, seconds, &error.to_string()),
        None => AttachedHostFailure::no_endpoint(host, &error.to_string()),
    })?;
    Ok(AttachedBrowser { forward, endpoint })
}

/// The port of the first line of a `DevToolsActivePort` file, when it is one.
fn port_of(text: &str) -> Option<u16> {
    text.lines().next()?.trim().parse().ok().filter(|port| *port != 0)
}

/// The live connection of a panel to the browser of another machine: the host,
/// the forward that carries its `DevTools` port, and the connect that is still
/// running when the panel was just created or retried.
pub(super) struct AttachedHostSlot {
    host: AttachedHost,
    ssh: Ssh,
    forward: Option<SshForward>,
    pending: Option<mpsc::Receiver<AttachOutcome>>,
    waker: Arc<Mutex<Option<BrowserEventWaker>>>,
    failure: Option<AttachedHostFailure>,
    /// Whether the session of this connect became ready, so the panel showed the
    /// page of that browser. A link that is lost after that keeps the panel.
    reached_ready: bool,
    /// Tells a running connect that its panel stopped, so no `ssh` call starts
    /// for a panel that is gone.
    stop: Arc<AtomicBool>,
    /// The automatic attempts that this panel has made since its last page, so
    /// a link that drops again waits longer.
    attempts: u32,
    /// When the last session reached its page. A connection that lasted long
    /// enough starts the delays again, as a machine terminal's does.
    ready_at: Option<Instant>,
}

impl AttachedHostSlot {
    /// Start the worker thread that connects to `host`. The call returns at
    /// once; the outcome arrives as a state of the panel.
    fn connecting(ssh: Ssh, host: AttachedHost) -> Self {
        Self::connecting_after(ssh, host, Duration::ZERO, 0)
    }

    /// Start the worker thread that connects to `host` after `after`, as
    /// automatic attempt `attempt`. The call returns at once, and the wait runs
    /// on that worker, so no frame waits for it.
    fn connecting_after(ssh: Ssh, host: AttachedHost, after: Duration, attempt: u32) -> Self {
        let (sender, pending) = mpsc::channel();
        let waker: Arc<Mutex<Option<BrowserEventWaker>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_host = host.clone();
        let worker_stop = Arc::clone(&stop);
        let worker_waker = Arc::clone(&waker);
        let worker_ssh = ssh.clone();
        let mut slot = Self {
            host,
            ssh,
            forward: None,
            pending: None,
            waker,
            failure: None,
            reached_ready: false,
            stop,
            attempts: attempt,
            ready_at: None,
        };
        let worker = std::thread::Builder::new()
            .name("browser-attach".to_string())
            .spawn(move || {
                if !wait_before_start(&worker_stop, after) {
                    return;
                }
                let outcome = connect(&worker_ssh, &worker_host, &worker_stop);
                // A panel that stopped drops its receiver first, so this send
                // fails and its forward dies with the outcome.
                if sender.send(outcome).is_ok()
                    && let Some(waker) = lock(&worker_waker).as_ref()
                {
                    waker();
                }
            });
        match worker {
            Ok(_) => slot.pending = Some(pending),
            Err(error) => {
                slot.failure = Some(AttachedHostFailure::unreachable(
                    &slot.host,
                    &format!("this computer could not start the connect thread: {error}"),
                ));
            }
        }
        slot
    }

    fn host(&self) -> &AttachedHost {
        &self.host
    }

    /// The short line that the panel shows while it connects and when it waits
    /// for a retry.
    fn note(&self) -> String {
        format!(
            "browser of {} in the profile {}, over SSH",
            self.host.ssh_target(),
            self.host.profile()
        )
    }
}

fn lock<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Wait `after` on the worker of an automatic attempt, and give up when the
/// panel stops during the wait. Returns whether the connect may start.
fn wait_before_start(stop: &AtomicBool, after: Duration) -> bool {
    let deadline = Instant::now() + after;
    loop {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        std::thread::sleep(remaining.min(RETRY_POLL));
    }
}

/// The next automatic attempt of a panel that lost its link, with its delay. A
/// connection that lasted [`RECONNECT_RESET_AFTER`] starts the delays again, as
/// a machine terminal of the same link does.
fn next_attempt(attempts: u32, ready_at: Option<Instant>) -> (u32, Duration) {
    let attempts = match ready_at {
        Some(ready_at) if ready_at.elapsed() >= RECONNECT_RESET_AFTER => 0,
        _ => attempts,
    };
    let attempt = attempts.saturating_add(1);
    (attempt, reconnect_delay(attempt))
}

/// The reason that a panel which follows a browser host cannot serve a page of
/// `backend`: that host runs Chromium only, so its profile is not a Firefox or a
/// Safari profile.
#[must_use]
pub const fn attached_host_backend_refusal(backend: BackendKind) -> Option<&'static str> {
    match backend {
        BackendKind::ChromiumCdp => None,
        BackendKind::FirefoxBidi | BackendKind::SafariWebDriver => Some(
            "a browser host serves Chromium only: the page of that host runs in the Chromium of its profile, so run this panel on the browser of this computer instead",
        ),
    }
}

impl BrowserPanelState {
    /// Create a panel whose Chromium runs on another machine.
    ///
    /// This call returns at once. The forward to that machine and the endpoint
    /// of its browser are read on a worker thread, so a host that drops packets
    /// never holds the caller. The outcome is a state of the panel: `status`
    /// names the reason, and a retry connects again.
    #[must_use]
    pub fn start_attached_host(
        panel_local_id: impl Into<String>,
        config: &super::BrowserConfig,
        initial_url: Option<String>,
        host: &AttachedHost,
    ) -> Self {
        Self::start_attached_host_with(Ssh::default(), panel_local_id, config, initial_url, host)
    }

    /// [`Self::start_attached_host`] with the `ssh` of a test: another program,
    /// another bound, and a readiness probe that no real forward answers.
    #[doc(hidden)]
    #[must_use]
    pub fn start_attached_host_with(
        ssh: Ssh,
        panel_local_id: impl Into<String>,
        config: &super::BrowserConfig,
        initial_url: Option<String>,
        host: &AttachedHost,
    ) -> Self {
        let mut state = Self::inert();
        state.panel_local_id = panel_local_id.into();
        state.config = config.clone();
        // A browser host serves the Chromium of its profile and nothing else.
        state.config.backend = BackendKind::ChromiumCdp;
        state.status = BrowserStatus::Starting;
        state.loading = true;
        let requested_url = initial_url.map(|url| horizon_browser::normalize_navigation_target(&url));
        state.requested_url.clone_from(&requested_url);
        let slot = AttachedHostSlot::connecting(ssh, host.clone());
        if let Some(failure) = slot.failure.clone() {
            state.attached_failed(failure);
        }
        state.remote_status = Some(slot.note());
        state.attached = Some(slot);
        state
    }

    /// A panel of `host` whose connect is done, for a test that has no machine
    /// to connect to. It holds no forward, runs no `ssh` and has no session.
    #[doc(hidden)]
    #[must_use]
    pub fn attached_host_for_tests(host: &AttachedHost, last_url: &str) -> Self {
        let mut state = Self::inert();
        state.panel_local_id = "attached-for-tests".to_string();
        state.url = Some(last_url.to_string());
        state.requested_url = Some(last_url.to_string());
        state.status = BrowserStatus::Starting;
        state.remote_status = Some(format!(
            "browser of {} in the profile {}, over SSH",
            host.ssh_target(),
            host.profile()
        ));
        state.attached = Some(AttachedHostSlot {
            host: host.clone(),
            ssh: Ssh::default(),
            forward: None,
            pending: None,
            waker: Arc::default(),
            failure: None,
            reached_ready: false,
            // No connect runs for this slot, so a stop finds it stopped and no
            // `ssh` call starts for it.
            stop: Arc::new(AtomicBool::new(true)),
            attempts: 0,
            ready_at: None,
        });
        state
    }

    /// A panel of `host` that lost its browser after it showed a page, for a
    /// test that has no machine to lose. Only the failure state, its last
    /// address and its Retry matter.
    #[doc(hidden)]
    #[must_use]
    pub fn lost_attached_host_for_tests(host: &AttachedHost, last_url: &str, reason: &str) -> Self {
        let mut state = Self::attached_host_for_tests(host, last_url);
        state.attached_reached_ready();
        state.attached_failed(AttachedHostFailure::connection_lost(host, reason));
        state
    }

    /// Whether the Chromium of this panel runs on another machine.
    #[must_use]
    pub fn is_attached(&self) -> bool {
        self.attached.is_some()
    }

    /// The SSH target of the machine that runs the Chromium of this panel.
    #[must_use]
    pub fn attached_host_target(&self) -> Option<&str> {
        self.attached.as_ref().map(|slot| slot.host.ssh_target())
    }

    /// The profile of the Chromium of this panel on that machine.
    #[must_use]
    pub fn attached_host_profile(&self) -> Option<&str> {
        self.attached.as_ref().map(|slot| slot.host.profile())
    }

    /// Why the last connect of this panel failed. `None` while it is connected
    /// or still connecting.
    #[must_use]
    pub fn attached_host_failure(&self) -> Option<&AttachedHostFailure> {
        self.attached.as_ref().and_then(|slot| slot.failure.as_ref())
    }

    /// The endpoint of the connect that the next session must drive. The panel
    /// keeps it until the session starts, because the session of an attached
    /// browser is the ordinary session with that endpoint.
    pub(super) fn take_attach_endpoint(&mut self) -> Option<DevtoolsEndpoint> {
        self.attach_endpoint.take()
    }
}

impl BrowserPanelState {
    /// Replace the connect of a panel whose browser runs on another machine. The
    /// new session opens at the last page that this panel showed or at the
    /// navigation that a person typed while the panel was down; it opens a new
    /// page, because a lost link cannot keep the old one.
    pub(super) fn relaunch_attached(&mut self) {
        let Some(slot) = self.attached.take() else {
            return;
        };
        let host = slot.host().clone();
        let ssh = slot.ssh.clone();
        // Dropping the slot kills the forward of the session that just ended and
        // drops its receiver, so a connect that is still running cannot deliver.
        drop(slot);
        self.frame_slot.clear();
        self.navigation_error = None;
        self.attach_endpoint = None;
        self.loading = true;
        self.status = BrowserStatus::Starting;
        let slot = AttachedHostSlot::connecting(ssh, host);
        if let Some(failure) = slot.failure.clone() {
            self.attached_failed(failure);
        }
        self.remote_status = Some(slot.note());
        self.attached = Some(slot);
    }

    /// The automatic attempt that this panel waits for after a lost link, with
    /// the instant of that attempt.
    #[must_use]
    pub fn attached_retry(&self) -> Option<(u32, Instant)> {
        match &self.status {
            BrowserStatus::Reconnecting { attempt, retry_at } => Some((*attempt, *retry_at)),
            _ => None,
        }
    }

    /// Replace the connect of a panel that lost its link with the next automatic
    /// attempt, and report that attempt as the state of the panel. The browser
    /// of that machine keeps running, so the attempt reads its endpoint and
    /// opens the last address again in a new page.
    fn start_automatic_attempt(&mut self, failure: Option<AttachedHostFailure>) {
        let Some(slot) = self.attached.as_mut() else {
            return;
        };
        let (attempt, after) = next_attempt(slot.attempts, slot.ready_at);
        if slot.stop.load(Ordering::Acquire) {
            // This panel has no connect to replace, so nothing may start an
            // `ssh` call for it. The attempt stays the state of the panel.
            slot.attempts = attempt;
            slot.failure = failure;
            self.status = BrowserStatus::Reconnecting {
                attempt,
                retry_at: Instant::now() + after,
            };
            return;
        }
        let host = slot.host().clone();
        let ssh = slot.ssh.clone();
        let slot = self.attached.take();
        // Dropping the old slot ends its forward and drops its receiver, so a
        // connect that is still running cannot deliver while this attempt waits.
        drop(slot);
        let mut next = AttachedHostSlot::connecting_after(ssh, host, after, attempt);
        next.failure.clone_from(&failure);
        if let Some(failure) = &failure {
            tracing::warn!(
                code = failure.code(),
                target = next.host.ssh_target(),
                profile = next.host.profile(),
                attempt,
                after = ?after,
                "the link to a browser host is lost, so the panel tries again by itself"
            );
        }
        self.attach_endpoint = None;
        self.frame_slot.clear();
        self.loading = true;
        self.remote_status = Some(next.note());
        self.status = BrowserStatus::Reconnecting {
            attempt,
            retry_at: Instant::now() + after,
        };
        self.attached = Some(next);
    }

    /// Stop the connect and the forward of this panel, and keep the host that it
    /// reports, so a retry still reaches the same machine.
    pub(super) fn shutdown_attached(&mut self) {
        self.attach_endpoint = None;
        let Some(slot) = self.attached.as_mut() else {
            return;
        };
        slot.stop.store(true, Ordering::Release);
        slot.failure = None;
        slot.pending = None;
        slot.forward = None;
    }

    /// End the forward of the session that just ended. The remote Chromium keeps
    /// running, and the next retry reuses it.
    pub(super) fn release_attached_forward(&mut self) {
        if let Some(slot) = self.attached.as_mut() {
            slot.forward = None;
        }
    }

    /// Take the outcome of the connect of this panel, when it arrived, and start
    /// the session of that browser. Returns whether the panel changed.
    pub(super) fn poll_attached(&mut self) -> bool {
        let outcome = {
            let Some(slot) = self.attached.as_ref() else {
                return false;
            };
            let Some(pending) = slot.pending.as_ref() else {
                return false;
            };
            match pending.try_recv() {
                Ok(outcome) => outcome,
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => {
                    AttachOutcome::Failed(AttachedHostFailure::closed(slot.host()))
                }
            }
        };
        match outcome {
            AttachOutcome::Connected(browser) => {
                let automatic = self.attached.as_ref().is_some_and(|slot| slot.attempts > 0);
                if let Some(slot) = self.attached.as_mut() {
                    slot.pending = None;
                    slot.failure = None;
                    slot.forward = Some(browser.forward);
                }
                self.attach_endpoint = Some(browser.endpoint);
                self.loading = true;
                self.status = BrowserStatus::Starting;
                self.launch_session(self.relaunch_target());
                if automatic {
                    // The browser of that machine kept running, but a new link
                    // opens a new page of the last address: what the person had
                    // in the old page is gone, and the panel says so.
                    self.navigation_error = Some(PAGE_STATE_LOST.to_string());
                }
                true
            }
            AttachOutcome::Failed(failure) => {
                self.attached_failed(failure);
                true
            }
            AttachOutcome::Cancelled => false,
        }
    }

    fn attached_failed(&mut self, failure: AttachedHostFailure) {
        // An automatic attempt that could not reach the host continues the
        // sequence: a cut ends every attempt, and the browser of that machine is
        // still there behind the link. Another reason needs a person, so it ends
        // at the present failure with Retry.
        if failure.kind() == AttachedHostFailureKind::Unreachable
            && self.attached.as_ref().is_some_and(|slot| slot.attempts > 0)
        {
            self.start_automatic_attempt(Some(failure));
            return;
        }
        tracing::warn!(
            code = failure.code(),
            target = self.attached_host_target().unwrap_or_default(),
            profile = self.attached_host_profile().unwrap_or_default(),
            detail = failure.detail().unwrap_or_default(),
            "a browser panel could not attach to its browser host"
        );
        self.frame_slot.clear();
        self.loading = false;
        self.navigation_error = None;
        self.status = BrowserStatus::Error {
            message: failure.message().to_string(),
        };
        if let Some(slot) = self.attached.as_mut() {
            slot.pending = None;
            slot.failure = Some(failure);
        }
    }

    /// The panel of a browser host that lost its browser after it was ready, or
    /// that never reached it. Such a panel keeps the last address that it
    /// showed and the person's Retry connects again, so no cleanup takes it
    /// away: the reason stays where the page was.
    pub(super) fn attached_kept_after(&self) -> bool {
        self.attached
            .as_ref()
            .is_some_and(|slot| slot.failure.is_some() && slot.reached_ready)
    }

    /// Remember that the session of this connect showed a page, so a later lost
    /// link is a page that the person had.
    pub(super) fn attached_reached_ready(&mut self) {
        if let Some(slot) = self.attached.as_mut() {
            slot.reached_ready = true;
            slot.ready_at = Some(Instant::now());
        }
    }

    /// Put a panel that lost the browser of its host into the state of the
    /// automatic attempt that follows, in the words of the lost connection. A
    /// panel that never showed a page ends as it did before: its create reports
    /// the failure, so nothing stays for a page the person never read.
    ///
    /// The endpoint and the SSH route of this panel did not change, so the
    /// attempt reaches the same browser again and opens the last address in a
    /// new page.
    pub(super) fn attached_lost_after_ready(&mut self, reason: &str) {
        let Some(host) = self
            .attached
            .as_ref()
            .filter(|slot| slot.reached_ready)
            .map(|slot| slot.host().clone())
        else {
            return;
        };
        self.start_automatic_attempt(Some(AttachedHostFailure::connection_lost(&host, reason)));
    }

    pub(super) fn needs_attached_waker(&self) -> bool {
        self.attached.as_ref().is_some_and(|slot| slot.pending.is_some())
    }

    pub(super) fn set_attached_waker(&self, waker: BrowserEventWaker) {
        if let Some(slot) = self.attached.as_ref() {
            *lock(&slot.waker) = Some(waker);
        }
    }
}
