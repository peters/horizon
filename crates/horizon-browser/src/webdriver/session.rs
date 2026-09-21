use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::frames::FrameSlot;
use crate::input::{is_activity, is_user_activity};
use crate::process::ChromeProcessControl;
use crate::semantic::SemanticState;
use crate::session::{BrowserCommand, BrowserEvent, BrowserEventSender, BrowserSessionConfig, CommandReceiver};
use crate::websocket::JsonWsLink;

use super::actions::ActionState;
use super::host::DriverHost;

mod bidi;
mod coordination;
mod frames;
pub(super) mod handshake;
mod http_auth;
pub(crate) mod native_select;
mod navigation;
mod network;
mod remote_click;
mod remote_files;
mod safari;
mod scrollbar;
mod semantic;
mod shutdown;
mod startup;
mod viewport;
mod wait;

use frames::AdaptiveFrames;
use handshake::{NewSession, create_webdriver_session, initial_safari_input};
use shutdown::Completion;
use startup::{BidiLink, establish_bidi, firefox_bidi_mode, start_local, start_remote};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const PAGE_LOAD_TIMEOUT_MILLIS: u64 = 50_000;
/// Page-load bound for the startup navigation, so the servicing loop starts
/// even when the first page is slow.
const STARTUP_PAGE_LOAD_TIMEOUT_MILLIS: u64 = 10_000;
const NAVIGATION_HTTP_TIMEOUT: Duration = Duration::from_millis(PAGE_LOAD_TIMEOUT_MILLIS + 5_000);
// WebDriver input is synchronous. Taking a large batch out of the shared
// queue prevents newer hover/wheel events from coalescing while each protocol
// roundtrip runs, and delays frame capture until the whole batch completes.
// Four keeps a complete physical double-click together while bounding the
// time before Firefox can publish another frame.
const MAX_COMMAND_BURST: usize = 4;

#[allow(clippy::struct_excessive_bools)] // independent per-concern driver flags
struct Driver {
    config: BrowserSessionConfig,
    host: DriverHost,
    /// Where `close` records what a remote release established, for the
    /// host's teardown signal.
    remote_release: crate::session::RemoteReleaseReport,
    /// The allocated remote device as the provider's evidence describes
    /// it, for coordination; `None` for a local browser.
    remote_device: Option<String>,
    remote_android_chromium: bool,
    file_transfer: Option<remote_files::Transfer>,
    session_id: String,
    bidi: Option<JsonWsLink>,
    automation_ws: String,
    context_id: Option<String>,
    safari: Option<safari::InputState>,
    actions: ActionState,
    frames: AdaptiveFrames,
    pending_resize: Option<crate::session::viewport::PendingResize>,
    viewport_policy: crate::session::viewport::ViewportPolicy,
    viewport_context: Option<String>,
    viewport_retry: viewport::RestorationRetry,
    scrollbar: scrollbar::State,
    url: String,
    title: String,
    generation: u64,
    retain_frame_during_navigation: bool,
    navigation_failed: bool,
    /// Agent navigation whose typed outcome is settled by `BiDi` events.
    pending_navigation: Option<crate::navigation::PendingNavigation>,
    pending_wait: Option<crate::wait::PendingWait>,
    /// In-flight `browsingContext.navigate` sent without waiting: Firefox
    /// answers it only once the destination responds.
    navigate_request_id: Option<u64>,
    pending_classic_history_start: Option<PendingHistoryStart>,
    refresh_pending_at: Option<Instant>,
    /// Session page-load timeout a bounded classic navigation lowered and the
    /// loop still has to restore, once the typed outcome was published.
    classic_timeout_to_restore: Option<u64>,
    /// Script-observed identity of Safari's current document. Unlike its URL,
    /// this changes on a same-URL reload and lets waits reject replacement
    /// documents even though classic `WebDriver` emits no navigation events.
    classic_document_identity: Option<String>,
    /// A bounded classic navigation (startup or agent action) that is still
    /// loading after its result deadline and must be polled to a document
    /// replacement, URL change, or the page-load cutoff.
    classic_refresh: Option<navigation::ClassicNavigationRefresh>,
    coordination_dirty: bool,
    last_coordination_write: Instant,
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
    firefox_network: Option<network::FirefoxNetworkBridge>,
    pending_http_bodies: VecDeque<(String, Option<String>)>,
    panel_slot: Arc<FrameSlot>,
    native_select: native_select::NativeSelectState,
}

struct PendingHistoryStart {
    url: String,
    expires_at: Instant,
}

pub(crate) struct WebDriverLaunch<'a> {
    pub(crate) process_control: &'a ChromeProcessControl,
    pub(crate) group: Option<super::FirefoxReservation>,
}

pub(crate) fn run_webdriver(
    config: &BrowserSessionConfig,
    event_tx: &BrowserEventSender,
    command_rx: &CommandReceiver,
    frame_slot: &Arc<FrameSlot>,
    stop_requested: &Arc<AtomicBool>,
    teardown: crate::session::DriverTeardown,
    launch: WebDriverLaunch<'_>,
) {
    let crate::session::DriverTeardown {
        completion: completion_tx,
        remote_release,
    } = teardown;
    let process_control = launch.process_control;
    let completion = Completion::new(
        completion_tx,
        config.remote.as_ref().map(|r| r.recovery.clone()),
        Arc::clone(&remote_release),
        process_control.clone(),
        launch.group,
    );
    let Some(_coordination_lifetime) = crate::coordination::CoordinationLifetime::start(config) else {
        let _ = event_tx.send(BrowserEvent::Warning(crate::coordination::PREPARE_FAILURE.to_string()));
        let _ = event_tx.send(BrowserEvent::Stopped { code: None });
        return;
    };
    let mut driver = match Driver::start(
        config,
        process_control,
        stop_requested,
        frame_slot,
        event_tx,
        remote_release,
        completion.group.as_ref().map(|reservation| &reservation.group),
    ) {
        Ok(driver) => driver,
        Err(error) => {
            let _ = event_tx.send(BrowserEvent::Warning(error));
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return;
        }
    };
    driver.prepare_ready(config, frame_slot, event_tx);

    while !stop_requested.load(Ordering::Acquire) {
        let mut stop = false;
        let batch = command_rx.drain(MAX_COMMAND_BURST);
        for command in batch.commands {
            driver.audit_user_command(&command);
            if driver.run_command(command, event_tx, true).is_ok_and(|stop| stop) {
                stop = true;
                break;
            }
        }
        stop |= batch.disconnected;
        if stop {
            break;
        }
        if driver.finish_if_service_exited(event_tx) {
            return;
        }
        driver.tick_safari_input(event_tx);
        for request in driver.tick_coordination(event_tx) {
            // A blocking action later in the batch must not delay the typed
            // timeout of a navigation or wait dispatched earlier in it, and a
            // navigation that hit its bound earlier in the batch must have its
            // session timeout restored and page state refreshed before the
            // next request runs.
            driver.tick_pending_navigation();
            driver.tick_pending_wait(stop_requested);
            driver.tick_classic_timeout_restore();
            driver.tick_page_state_refresh(event_tx);
            driver.service_browser_request(&request, event_tx, stop_requested);
            if driver.finish_if_service_exited(event_tx) {
                return;
            }
        }
        if let Err(error) = driver.drain_bidi_events(event_tx) {
            tracing::warn!(backend = ?driver.config.browser.backend, "BiDi event pump failed: {error}");
            if driver.firefox_bidi() {
                let _ = event_tx.send(BrowserEvent::Warning(format!("Firefox BiDi disconnected: {error}")));
                break;
            }
            driver.disable_optional_bidi(frame_slot, event_tx);
        }
        driver.tick_firefox_http_response_bodies(event_tx);
        if let Some(message) = driver.challenge_loop.take_rejection() {
            let _ = event_tx.send(BrowserEvent::NavigationFailed(message.to_string()));
        }
        if driver.frames.due(Instant::now()) {
            driver.capture_frame(frame_slot, event_tx);
        }
        driver.tick_classic_timeout_restore();
        driver.tick_page_state_refresh(event_tx);
        driver.tick_pending_navigation();
        driver.tick_pending_wait(stop_requested);
        driver.tick_pending_resize(event_tx, stop_requested);
        driver.write_coordination(false);
        if driver.finish_if_service_exited(event_tx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    driver.finish_pending_resize("browser_unavailable", "browser session stopped");
    driver.settle_pending_wait_for_shutdown(Instant::now());
    driver.close(event_tx);
    let _ = event_tx.send(BrowserEvent::Stopped { code: None });
}

impl Driver {
    fn prepare_ready(&mut self, config: &BrowserSessionConfig, frame_slot: &FrameSlot, event_tx: &BrowserEventSender) {
        if self.firefox_bidi() {
            self.set_viewport(config.width, config.height, event_tx);
        }
        self.initialize_coordination();
        self.initialize_classic_document_identity();
        let capabilities = self.active_capabilities();
        frame_slot.publish_backend_capabilities(capabilities);
        let _ = event_tx.send(BrowserEvent::BackendReady(capabilities));
        let startup_navigation_pending = config
            .initial_url
            .as_deref()
            .filter(|url| !url.is_empty() && *url != "about:blank")
            .is_some_and(|url| self.navigate_initial(url, event_tx));
        // `Ready` means the servicing loop below is about to run: commands and
        // agent actions are only usable from here on, so it must not be
        // published before the (bounded) startup navigation returned. The host
        // resets its loading flag on `Ready`, so a startup navigation that is
        // still running is reported as loading again right after it.
        let _ = event_tx.send(BrowserEvent::Ready);
        if startup_navigation_pending {
            let _ = event_tx.send(BrowserEvent::Loading(true));
        }
    }

    fn service_browser_request(
        &mut self,
        request: &crate::AgentAction,
        events: &BrowserEventSender,
        stop: &AtomicBool,
    ) {
        if matches!(request.action, crate::BrowserControlAction::Resize { .. }) {
            self.begin_resize(request);
        } else {
            self.service_agent_request(request, events, stop);
        }
    }

    fn finish_if_service_exited(&mut self, events: &BrowserEventSender) -> bool {
        if !self.stop_for_service_exit(events) {
            return false;
        }
        self.finish_pending_resize("browser_unavailable", "browser session stopped");
        true
    }

    fn start(
        config: &BrowserSessionConfig,
        process_control: &ChromeProcessControl,
        stop_requested: &AtomicBool,
        frame_slot: &Arc<FrameSlot>,
        event_tx: &BrowserEventSender,
        remote_release: crate::session::RemoteReleaseReport,
        group: Option<&super::SharedFirefoxSession>,
    ) -> Result<Self, String> {
        let (mut host, session, remote_device, file_transfer) = if let Some(request) = &config.remote {
            let (host, session, device) = start_remote(request, event_tx, &remote_release, stop_requested)?;
            let transfer = remote_files::Transfer::for_provider(
                request.adapter,
                device
                    .os_name
                    .as_deref()
                    .or_else(|| session.capabilities["platformName"].as_str()),
            );
            (host, session, Some(device.summary()), transfer)
        } else {
            let (host, session) = if let Some(group) = group {
                let mut shared_config = config.clone();
                group.profile_id().clone_into(&mut shared_config.panel_local_id);
                group.acquire(&shared_config.browser, process_control, stop_requested, |control| {
                    start_local(&shared_config, control, stop_requested)
                })?
            } else {
                start_local(config, process_control, stop_requested)?
            };
            (host, session, None, None)
        };
        let NewSession {
            id: session_id,
            capabilities,
        } = session;
        let BidiLink {
            bidi,
            context_id,
            automation_ws,
        } = establish_bidi(config, &mut host, &session_id, &capabilities, stop_requested)?;
        let safari = if host.is_remote() {
            None
        } else {
            initial_safari_input(host.transport(), &session_id, config.browser.backend)?
        };
        Ok(Self {
            config: config.clone(),
            host,
            remote_release,
            remote_device,
            file_transfer,
            remote_android_chromium: remote_click::uses_visual_viewport(config.remote.is_some(), &capabilities),
            session_id,
            bidi,
            automation_ws,
            context_id,
            safari,
            actions: ActionState::default(),
            frames: AdaptiveFrames::new(),
            pending_resize: None,
            viewport_policy: crate::session::viewport::ViewportPolicy::new([config.width, config.height]),
            viewport_context: None,
            viewport_retry: viewport::RestorationRetry::default(),
            scrollbar: scrollbar::State::new(),
            url: String::new(),
            title: String::new(),
            generation: 0,
            retain_frame_during_navigation: false,
            navigation_failed: false,
            pending_navigation: None,
            pending_wait: None,
            navigate_request_id: None,
            pending_classic_history_start: None,
            refresh_pending_at: None,
            classic_timeout_to_restore: None,
            classic_document_identity: None,
            classic_refresh: None,
            coordination_dirty: true,
            last_coordination_write: Instant::now(),
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
            video: crate::video::VideoCaptureState::new(Arc::clone(&config.video)),
            firefox_network: None,
            pending_http_bodies: VecDeque::new(),
            panel_slot: Arc::clone(frame_slot),
            native_select: native_select::NativeSelectState::default(),
        })
    }

    /// A user or agent acted on the page: the remote idle clock restarts.
    /// System commands (viewport, video, handoff bookkeeping) and frame
    /// polling never count.
    pub(super) fn note_remote_activity(&mut self) {
        if let Some(remote) = self.host.remote() {
            remote.note_activity(Instant::now());
        }
    }

    /// Whether this session drives Firefox over `BiDi`. A remote session never
    /// does, whatever local backend is configured: every remote request is
    /// classic `WebDriver`.
    fn firefox_bidi(&self) -> bool {
        firefox_bidi_mode(&self.config, &self.host)
    }

    fn run_command(
        &mut self,
        command: BrowserCommand,
        events: &BrowserEventSender,
        user: bool,
    ) -> Result<bool, String> {
        if user && is_user_activity(&command) {
            self.note_remote_activity();
            self.stamp_user_active();
        }
        match command {
            BrowserCommand::Navigate(url) => self.navigate(&url, events).map(|()| false),
            BrowserCommand::Reload => self.reload(events).map(|()| false),
            BrowserCommand::Back => self.traverse(-1, events).map(|()| false),
            BrowserCommand::Forward => self.traverse(1, events).map(|()| false),
            BrowserCommand::SetViewport { width, height } => {
                if self.viewport_policy.follow_host([width, height]) {
                    self.set_viewport(width, height, events);
                }
                Ok(false)
            }
            BrowserCommand::Input(input) => self.perform_input(input, events).map(|()| false),
            BrowserCommand::NativeSelectChoose { index } => {
                self.apply_native_select_choice(events, index).map(|()| false)
            }
            BrowserCommand::NativeSelectDismiss => {
                self.dismiss_native_select(events);
                self.send_escape_to_page(events);
                Ok(false)
            }
            BrowserCommand::HandoffDone => {
                self.resolve_handoff(events);
                Ok(false)
            }
            BrowserCommand::Video { operation, options } => {
                if let Err(error) = self.video_action(&crate::new_action_id(), operation, options.as_ref()) {
                    let _ = events.send(BrowserEvent::VideoFailed(format!("{}: {}", error.code, error.message)));
                }
                Ok(false)
            }
            BrowserCommand::Stop => Ok(true),
        }
    }

    fn active_capabilities(&self) -> crate::ActiveBackendCapabilities {
        // The backend names the browser family; what the session can do is a
        // property of where it runs, and a remote grid offers less than any
        // local browser of that family.
        let capabilities = if self.host.is_remote() {
            crate::BackendCapabilities::remote_session()
        } else {
            self.config.browser.backend.capabilities()
        };
        crate::ActiveBackendCapabilities {
            backend: self.config.browser.backend,
            capabilities,
            bidi: self.bidi.is_some(),
            automation_disclosure: self
                .config
                .browser
                .automation_disclosure
                .ready_status(self.config.browser.backend),
        }
    }

    fn disable_optional_bidi(&mut self, frame_slot: &FrameSlot, event_tx: &BrowserEventSender) {
        self.bidi = None;
        self.automation_ws.clear();
        self.context_id = None;
        self.coordination_dirty = true;
        let capabilities = self.active_capabilities();
        frame_slot.publish_backend_capabilities(capabilities);
        let _ = event_tx.send(BrowserEvent::BackendReady(capabilities));
    }

    fn set_viewport(&mut self, width: u32, height: u32, event_tx: &BrowserEventSender) {
        self.advance_generation();
        if self.host.is_remote() {
            // A physical device is never resized because its panel was; the
            // panel maps the device's own viewport instead.
            self.frames.demand();
            return;
        }
        let result = if self.firefox_bidi() {
            self.call_bidi(
                "browsingContext.setViewport",
                &json!({
                    "context": self.context_id,
                    "viewport": { "width": width, "height": height },
                }),
                event_tx,
            )
            .map(|_| ())
        } else {
            // Safari sizes the decorated window while screenshots contain only its viewport.
            let (width, height) = self
                .classic_post(
                    "execute/sync",
                    &json!({ "script": safari::WINDOW_CHROME_SCRIPT, "args": [] }),
                )
                .map_or((width, height), |response| {
                    safari::window_rect(&response, width, height)
                });
            self.classic_post("window/rect", &json!({ "width": width, "height": height }))
                .map(|_| ())
        };
        if let Err(error) = result {
            tracing::warn!("WebDriver viewport update failed: {error}");
        } else if !self.retain_frame_during_navigation {
            self.frames.demand();
        }
    }

    fn perform_input(&mut self, input: crate::BrowserInput, event_tx: &BrowserEventSender) -> Result<(), String> {
        let activity = is_activity(&input);
        if activity {
            self.pending_classic_history_start = None;
        }
        if self.handle_scrollbar_input(&input)? {
            return Ok(());
        }
        self.capture_teach_input(&input);
        let probe_input = input.clone();
        let (result, demand_frame) = if self.firefox_bidi() {
            let mut payload = self.actions.payload(input);
            payload["context"] = json!(self.context_id);
            (
                self.call_bidi("input.performActions", &payload, event_tx).map(|_| ()),
                activity,
            )
        } else if self.safari.is_some() {
            return self.queue_safari_input(input);
        } else {
            let payload = self.actions.payload(input);
            (self.classic_post("actions", &payload).map(|_| ()), activity)
        };
        if let Err(error) = &result {
            tracing::warn!("WebDriver input failed: {error}");
        }
        if demand_frame && !self.retain_frame_during_navigation {
            self.scrollbar.refresh_at = Instant::now();
            self.frames.demand();
        }
        if result.is_ok() {
            self.note_native_select_input(&probe_input, event_tx)?;
        }
        result
    }

    fn advance_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.scrollbar.reset(&self.config.frame_slot);
        let _ = self.panel_slot.clear_native_select_popup();
        self.native_select = native_select::NativeSelectState::default();
        if !self.retain_frame_during_navigation {
            self.frames.invalidate();
        }
    }
}

fn webdriver_value(response: &Value) -> Option<&Value> {
    response.get("value").or(Some(response))
}

fn classic_navigation_committed(response: &Value, requested: &str, previous: &str) -> bool {
    webdriver_value(response)
        .and_then(Value::as_str)
        .is_some_and(|committed| {
            !committed.is_empty()
                && (committed != "about:blank" || requested == committed)
                && (committed != previous || requested == previous)
        })
}

fn normalize_url(url: &str) -> &str {
    if url == "about:blank" { "" } else { url }
}

fn consume_pending_history_start(pending: &mut Option<PendingHistoryStart>, url: Option<&str>, now: Instant) -> bool {
    let Some(pending) = pending.take() else {
        return false;
    };
    now <= pending.expires_at && url == Some(pending.url.as_str())
}

#[cfg(test)]
mod tests {
    use super::super::remote::{RemoteReleaseOutcome, RemoteSessionEvent, RemoteStartFailure};
    use std::sync::atomic::AtomicBool;

    use super::startup::{start_failure_outcome, start_remote};

    #[test]
    fn a_remote_start_that_never_reaches_the_provider_establishes_never_allocated() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let event_tx = crate::session::BrowserEventSender {
            tx,
            wake: crate::session::BrowserEventWake::default(),
            committed_url: crate::session::CommittedUrl::default(),
        };
        let request = super::super::remote::RemoteSessionRequest {
            adapter: horizon_browser_protocol::remote::RemoteAdapterKind::Webdriver,
            recovery: crate::RemoteAllocation::default(),
            endpoint: "http://grid.example.net/wd/hub".to_string(),
            authorization: None,
            capabilities: serde_json::json!({}),
            allocation_timeout: std::time::Duration::from_millis(100),
            max_session: std::time::Duration::from_secs(1),
            idle_release: std::time::Duration::from_secs(1),
            label: "ios".to_string(),
            provider: "grid".to_string(),
            quota_key: "grid-key".to_string(),
            browser: crate::BackendKind::SafariWebDriver,
            device: horizon_browser_protocol::remote::DeviceRequirement::default(),
            evidence: super::super::remote::identity::DeviceEvidenceSource::Capabilities,
        };
        let report =
            crate::session::RemoteReleaseReport::new(std::sync::Mutex::new(Some(RemoteReleaseOutcome::NeverAllocated)));
        let running = AtomicBool::new(false);
        let error = start_remote(&request, &event_tx, &report, &running)
            .err()
            .expect("plain HTTP is refused");
        assert!(error.contains("endpoint rejected"), "{error}");
        assert_eq!(
            *report.lock().expect("report"),
            Some(RemoteReleaseOutcome::NeverAllocated),
            "no provider request was made, so nothing is held"
        );

        // A panel closed before the driver reaches the provider never asks it.
        let stopped = AtomicBool::new(true);
        let error = start_remote(&request, &event_tx, &report, &stopped)
            .err()
            .expect("stopped first");
        assert!(error.contains("closed first"), "{error}");
        assert_eq!(
            *report.lock().expect("report"),
            Some(RemoteReleaseOutcome::NeverAllocated)
        );
    }

    #[test]
    fn only_a_refusal_or_a_confirmed_immediate_release_frees_the_slot() {
        let refused = RemoteStartFailure::AllocationFailed {
            error: "session not created".into(),
            message: "no device".into(),
        };
        let (established, event) = start_failure_outcome(&refused, "ios".into());
        assert_eq!(established, Some(RemoteReleaseOutcome::NeverAllocated));
        assert!(matches!(
            event,
            RemoteSessionEvent::AllocationFailed {
                refusal: super::super::remote::AllocationRefusal::DeviceUnavailable,
                ..
            }
        ));
        let unauthorized = RemoteStartFailure::AllocationFailed {
            error: "Invalid username or password".into(),
            message: "Invalid username or password".into(),
        };
        let (_, event) = start_failure_outcome(&unauthorized, "ios".into());
        assert!(matches!(
            event,
            RemoteSessionEvent::AllocationFailed {
                refusal: super::super::remote::AllocationRefusal::Authentication,
                ..
            }
        ));

        let unknown = RemoteStartFailure::AllocationUnknown {
            reason: "timeout".into(),
        };
        let (established, event) = start_failure_outcome(&unknown, "ios".into());
        assert_eq!(established, None, "an unknown allocation keeps holding the slot");
        assert!(matches!(event, RemoteSessionEvent::AllocationUnknown { .. }));

        let released = RemoteStartFailure::Unenforceable {
            released: RemoteReleaseOutcome::Released,
        };
        let (established, event) = start_failure_outcome(&released, "ios".into());
        assert_eq!(established, Some(RemoteReleaseOutcome::Released));
        assert!(matches!(event, RemoteSessionEvent::AllocationFailed { .. }));

        let rejected = RemoteStartFailure::IdentityRejected {
            reason: "device model is iPhone 15, target requires iPhone 16".into(),
            released: RemoteReleaseOutcome::Released,
        };
        let (established, event) = start_failure_outcome(&rejected, "ios".into());
        assert_eq!(established, Some(RemoteReleaseOutcome::Released));
        assert!(
            matches!(
                event,
                RemoteSessionEvent::DeviceRejected {
                    released: RemoteReleaseOutcome::Released,
                    ..
                }
            ),
            "a rejected device is its own terminal event"
        );

        for outcome in [
            RemoteReleaseOutcome::ReleaseUnknown {
                attempts: 3,
                reason: "timeout".into(),
            },
            RemoteReleaseOutcome::Failed {
                error: "unknown error".into(),
                message: "busy".into(),
            },
        ] {
            let unenforceable = RemoteStartFailure::Unenforceable {
                released: outcome.clone(),
            };
            let (established, event) = start_failure_outcome(&unenforceable, "ios".into());
            assert_eq!(established, Some(outcome), "the immediate release outcome is preserved");
            assert!(
                matches!(event, RemoteSessionEvent::AllocationUnknown { .. }),
                "an unreleased allocation is reported as unknown, never as failed"
            );
        }
    }

    #[test]
    fn only_timeouts_keep_a_bounded_classic_navigation_running() {
        use super::navigation::classic_error_is_page_load_timeout;
        assert!(classic_error_is_page_load_timeout(
            "WebDriver timeout: Timed out waiting for page load"
        ));
        assert!(classic_error_is_page_load_timeout(
            "WebDriver HTTP I/O: Resource temporarily unavailable (os error 11)"
        ));
        assert!(classic_error_is_page_load_timeout(
            "WebDriver HTTP I/O: connection timed out"
        ));
        assert!(!classic_error_is_page_load_timeout(
            "WebDriver unknown error: net::ERR_NAME_NOT_RESOLVED"
        ));
        assert!(!classic_error_is_page_load_timeout(
            "browser did not commit a reachable URL"
        ));
    }

    use super::bidi::{
        base_bidi_events, bidi_event_targets_context, bidi_navigation_complete, bidi_navigation_failed,
        bidi_subscription_params,
    };
    use super::frames::{AdaptiveFrames, capture_is_current};
    use super::handshake::{
        new_session_capabilities, parse_new_session_response, safe_session_id, validate_firefox_args,
    };
    use super::{
        PAGE_LOAD_TIMEOUT_MILLIS, PendingHistoryStart, classic_navigation_committed, consume_pending_history_start,
    };
    use crate::{BackendKind, BrowserConfig};

    #[test]
    fn session_ids_cannot_escape_webdriver_routes() {
        assert!(safe_session_id("abc-123_def"));
        assert!(!safe_session_id("../session"));
        assert!(!safe_session_id("contains/slash"));
    }

    #[test]
    fn classic_navigation_requires_a_committed_url() {
        assert!(classic_navigation_committed(
            &serde_json::json!({ "value": "https://example.test/next" }),
            "https://example.test/next",
            "https://example.test/previous",
        ));
        assert!(classic_navigation_committed(
            &serde_json::json!({ "value": "https://example.test/current" }),
            "https://example.test/current",
            "https://example.test/current",
        ));
        assert!(!classic_navigation_committed(
            &serde_json::json!({ "value": "https://example.test/previous" }),
            "https://unreachable.test/",
            "https://example.test/previous",
        ));
        assert!(!classic_navigation_committed(
            &serde_json::json!({ "value": "" }),
            "https://unreachable.test/",
            "https://example.test/previous",
        ));
        assert!(!classic_navigation_committed(
            &serde_json::json!({ "value": "about:blank" }),
            "https://unreachable.test/",
            "https://example.test/previous",
        ));
    }

    #[test]
    fn new_session_parser_preserves_negotiated_bidi_capability() {
        let response = serde_json::json!({
            "value": {
                "sessionId": "safe-session_1",
                "capabilities": {
                    "browserName": "safari",
                    "webSocketUrl": "ws://127.0.0.1:9223/session/safe-session_1"
                }
            }
        });
        let parsed = parse_new_session_response(&response).expect("session response");
        assert_eq!(parsed.id, "safe-session_1");
        assert_eq!(
            parsed.capabilities["webSocketUrl"],
            "ws://127.0.0.1:9223/session/safe-session_1"
        );
    }

    #[test]
    fn static_adaptive_frames_stop_after_confirmation() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut frames = AdaptiveFrames::new();
        frames.active_until = std::time::Instant::now();
        let mut hasher = DefaultHasher::new();
        "same".hash(&mut hasher);
        frames.last_hash = Some(hasher.finish());
        assert!(!frames.completed("same"));
        assert!(!frames.completed("same"));
        assert!(!frames.completed("same"));
        assert!(frames.next_capture.is_none());
    }

    #[test]
    fn stale_captures_are_rejected_across_generation_and_context_changes() {
        assert!(capture_is_current(4, 4, Some("context-a"), Some("context-a")));
        assert!(!capture_is_current(4, 5, Some("context-a"), Some("context-a")));
        assert!(!capture_is_current(4, 4, Some("context-a"), Some("context-b")));
        assert!(!capture_is_current(4, 4, Some("context-a"), None));
    }

    #[test]
    fn navigation_suspends_capture_until_a_fresh_event_demands_it() {
        let mut frames = AdaptiveFrames::new();
        frames.suspend_for_navigation();
        assert!(frames.next_capture.is_none());
        assert!(frames.interaction_started_at.is_some());

        frames.invalidate();
        assert!(frames.next_capture.is_some());
        assert!(frames.completed("same"));
    }

    #[test]
    fn bidi_navigation_outcomes_are_classified_before_frame_capture() {
        assert!(bidi_navigation_failed("browsingContext.navigationFailed"));
        assert!(bidi_navigation_complete("browsingContext.domContentLoaded"));
        assert!(bidi_navigation_complete("browsingContext.fragmentNavigated"));
        assert!(!bidi_navigation_complete("browsingContext.navigationStarted"));
    }

    #[test]
    fn bidi_navigation_events_only_mutate_the_bound_top_level_context() {
        let top = Some("top");
        assert!(bidi_event_targets_context(
            "browsingContext.navigationStarted",
            &serde_json::json!({ "context": "top" }),
            top,
        ));
        assert!(!bidi_event_targets_context(
            "browsingContext.domContentLoaded",
            &serde_json::json!({ "context": "challenge-iframe" }),
            top,
        ));
        assert!(bidi_event_targets_context(
            "network.responseStarted",
            &serde_json::json!({ "context": "challenge-iframe" }),
            top,
        ));
    }

    #[test]
    fn firefox_challenge_responses_use_a_context_scoped_subscription() {
        let baseline = bidi_subscription_params(&base_bidi_events(), None);
        assert!(!baseline["events"].as_array().is_some_and(|events| {
            events
                .iter()
                .any(|event| event.as_str() == Some("network.responseStarted"))
        }));
        assert!(baseline.get("contexts").is_none());

        let responses = bidi_subscription_params(&["network.responseStarted"], Some("top-context"));
        assert_eq!(responses["events"], serde_json::json!(["network.responseStarted"]));
        assert_eq!(responses["contexts"], serde_json::json!(["top-context"]));
    }

    #[test]
    fn matching_late_history_start_is_consumed_only_within_its_deadline() {
        let now = std::time::Instant::now();
        let mut pending = Some(PendingHistoryStart {
            url: "https://example.test/previous".to_string(),
            expires_at: now + std::time::Duration::from_secs(1),
        });
        assert!(consume_pending_history_start(
            &mut pending,
            Some("https://example.test/previous"),
            now,
        ));
        assert!(pending.is_none());

        let mut different_url = Some(PendingHistoryStart {
            url: "https://example.test/previous".to_string(),
            expires_at: now + std::time::Duration::from_secs(1),
        });
        assert!(!consume_pending_history_start(
            &mut different_url,
            Some("https://example.test/new"),
            now,
        ));
        assert!(different_url.is_none());

        let mut expired = Some(PendingHistoryStart {
            url: "https://example.test/previous".to_string(),
            expires_at: now,
        });
        assert!(!consume_pending_history_start(
            &mut expired,
            Some("https://example.test/previous"),
            now + std::time::Duration::from_millis(1),
        ));
        assert!(expired.is_none());
    }

    #[test]
    fn managed_firefox_arguments_cannot_be_overridden() {
        assert!(validate_firefox_args(&["--remote-debugging-port=1".to_string()]).is_err());
        assert!(validate_firefox_args(&["-profile".to_string()]).is_err());
        assert!(validate_firefox_args(&["--private-window".to_string()]).is_ok());
    }

    #[test]
    fn firefox_screenshot_session_keeps_scrollbars_visible() {
        let Some(profile_root) = tempfile::tempdir().ok() else {
            panic!("temporary profile root should be available");
        };
        let config = BrowserConfig {
            backend: BackendKind::FirefoxBidi,
            profile_root: Some(profile_root.path().to_path_buf()),
            ..BrowserConfig::default()
        };
        let capabilities = new_session_capabilities(&config, "panel", true).unwrap_or_default();
        let prefs = &capabilities["moz:firefoxOptions"]["prefs"];

        assert_eq!(capabilities["moz:firefoxOptions"]["args"][0], "-headless");
        assert_eq!(prefs["widget.gtk.overlay-scrollbars.enabled"], false);
        assert_eq!(prefs["ui.useOverlayScrollbars"], 0);

        let visible = new_session_capabilities(
            &BrowserConfig {
                headless: false,
                ..config
            },
            "panel",
            true,
        )
        .unwrap_or_default();
        assert!(
            visible["moz:firefoxOptions"]["args"]
                .as_array()
                .is_some_and(|args| args.iter().all(|argument| argument != "-headless"))
        );
    }

    #[test]
    fn safari_bidi_capability_can_fall_back_to_classic() {
        let config = BrowserConfig {
            backend: BackendKind::SafariWebDriver,
            ..BrowserConfig::default()
        };
        let with_bidi = new_session_capabilities(&config, "panel", true).unwrap_or_default();
        let classic = new_session_capabilities(&config, "panel", false).unwrap_or_default();

        assert_eq!(with_bidi["webSocketUrl"], true);
        assert!(classic.get("webSocketUrl").is_none());
    }

    #[test]
    fn webdriver_navigation_is_bounded_and_uses_supported_strategy() {
        let Some(profile_root) = tempfile::tempdir().ok() else {
            panic!("temporary profile root should be available");
        };
        for backend in [BackendKind::FirefoxBidi, BackendKind::SafariWebDriver] {
            let config = BrowserConfig {
                backend,
                profile_root: Some(profile_root.path().to_path_buf()),
                ..BrowserConfig::default()
            };
            let capabilities = new_session_capabilities(&config, "panel", true).unwrap_or_default();

            assert_eq!(capabilities["timeouts"]["pageLoad"], PAGE_LOAD_TIMEOUT_MILLIS);
            if backend == BackendKind::FirefoxBidi {
                assert_eq!(capabilities["pageLoadStrategy"], "eager");
            } else {
                assert!(capabilities.get("pageLoadStrategy").is_none());
            }
        }
    }
}
