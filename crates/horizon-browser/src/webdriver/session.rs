use std::collections::{VecDeque, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::challenge::{DocumentCommit, REJECTION_MESSAGE};
use crate::disclosure::COMMON_SIGNAL_PRELOAD_FUNCTION;
use crate::frames::FrameSlot;
use crate::input::{is_activity, is_user_activity};
use crate::process::{ChromeProcessControl, resolve_binary};
use crate::semantic::SemanticState;
use crate::session::{
    BrowserCommand, BrowserEvent, BrowserEventSender, BrowserSessionConfig, CommandReceiver, publish_frame,
};
use crate::websocket::{JsonWsError, JsonWsLink};
use crate::{AutomationDisclosurePolicy, BackendKind, BrowserConfig, PageScrollState};

use super::actions::ActionState;
use super::http::HttpError;
use super::service::{WebDriverService, prepare_profile};

mod coordination;
mod navigation;
mod network;
mod safari;
mod scrollbar;
mod semantic;
mod shutdown;
mod wait;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const PAGE_LOAD_TIMEOUT_MILLIS: u64 = 50_000;
/// Page-load bound for the startup navigation, so the servicing loop starts
/// even when the first page is slow.
const STARTUP_PAGE_LOAD_TIMEOUT_MILLIS: u64 = 10_000;
const NAVIGATION_HTTP_TIMEOUT: Duration = Duration::from_millis(PAGE_LOAD_TIMEOUT_MILLIS + 5_000);
const ACTIVE_FRAME_INTERVAL: Duration = Duration::from_millis(33);
const ACTIVE_WINDOW: Duration = Duration::from_millis(900);
const STATIC_CONFIRMATIONS: u8 = 3;
const MAX_EVENT_BURST: usize = 32;
// WebDriver input is synchronous. Taking a large batch out of the shared
// queue prevents newer hover/wheel events from coalescing while each protocol
// roundtrip runs, and delays frame capture until the whole batch completes.
// Four keeps a complete physical double-click together while bounding the
// time before Firefox can publish another frame.
const MAX_COMMAND_BURST: usize = 4;
const SCROLL_STATE_INTERVAL: Duration = Duration::from_millis(100);
const PAGE_SCROLL_STATE_SCRIPT: &str = "const root = document.scrollingElement || document.documentElement; return { scroll_x: window.scrollX, scroll_y: window.scrollY, viewport_width: window.innerWidth, viewport_height: window.innerHeight, client_width: document.documentElement.clientWidth, client_height: document.documentElement.clientHeight, content_width: root.scrollWidth, content_height: root.scrollHeight };";

struct Completion {
    tx: Option<mpsc::Sender<()>>,
    process: ChromeProcessControl,
}

impl Drop for Completion {
    fn drop(&mut self) {
        self.process.mark_registration_settled();
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
    }
}

struct AdaptiveFrames {
    next_capture: Option<Instant>,
    active_until: Instant,
    last_hash: Option<u64>,
    unchanged: u8,
    interaction_started_at: Option<Instant>,
}

impl AdaptiveFrames {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            next_capture: Some(now),
            active_until: now + ACTIVE_WINDOW,
            last_hash: None,
            unchanged: 0,
            interaction_started_at: None,
        }
    }

    fn demand(&mut self) {
        let now = Instant::now();
        self.active_until = now + ACTIVE_WINDOW;
        self.next_capture = Some(now);
        self.unchanged = 0;
        self.interaction_started_at.get_or_insert(now);
    }

    fn invalidate(&mut self) {
        self.last_hash = None;
        self.demand();
    }

    fn suspend_for_navigation(&mut self) {
        let now = Instant::now();
        self.last_hash = None;
        self.unchanged = 0;
        self.active_until = now + ACTIVE_WINDOW;
        self.next_capture = None;
        self.interaction_started_at.get_or_insert(now);
    }

    fn due(&self, now: Instant) -> bool {
        self.next_capture.is_some_and(|next| now >= next)
    }

    fn completed(&mut self, encoded: &str) -> bool {
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

    fn failed(&mut self) {
        self.next_capture = Some(Instant::now() + Duration::from_millis(250));
    }

    fn published(&mut self) -> Option<Duration> {
        self.interaction_started_at.take().map(|started| started.elapsed())
    }
}

struct Driver {
    config: BrowserSessionConfig,
    service: WebDriverService,
    session_id: String,
    bidi: Option<JsonWsLink>,
    automation_ws: String,
    context_id: Option<String>,
    safari: Option<safari::InputState>,
    actions: ActionState,
    frames: AdaptiveFrames,
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
    network: crate::network::NetworkCaptureState,
    firefox_network: Option<network::FirefoxNetworkBridge>,
    pending_http_bodies: VecDeque<(String, Option<String>)>,
}

struct PendingHistoryStart {
    url: String,
    expires_at: Instant,
}

struct NewSession {
    id: String,
    capabilities: Value,
}

pub(crate) fn run_webdriver(
    config: &BrowserSessionConfig,
    event_tx: &BrowserEventSender,
    command_rx: &CommandReceiver,
    frame_slot: &Arc<FrameSlot>,
    stop_requested: &Arc<AtomicBool>,
    completion_tx: mpsc::Sender<()>,
    process_control: &ChromeProcessControl,
) {
    let _completion = Completion {
        tx: Some(completion_tx),
        process: process_control.clone(),
    };
    let Some(_coordination_lifetime) = crate::coordination::CoordinationLifetime::start(config) else {
        let _ = event_tx.send(BrowserEvent::Warning(crate::coordination::PREPARE_FAILURE.to_string()));
        let _ = event_tx.send(BrowserEvent::Stopped { code: None });
        return;
    };
    let mut driver = match Driver::start(config, process_control, stop_requested) {
        Ok(driver) => driver,
        Err(error) => {
            let _ = event_tx.send(BrowserEvent::Warning(error));
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return;
        }
    };
    driver.initialize_coordination();
    driver.initialize_classic_document_identity();
    let capabilities = driver.active_capabilities();
    frame_slot.publish_backend_capabilities(capabilities);
    let _ = event_tx.send(BrowserEvent::BackendReady(capabilities));
    let startup_navigation_pending = config
        .initial_url
        .as_deref()
        .filter(|url| !url.is_empty() && *url != "about:blank")
        .is_some_and(|url| driver.navigate_initial(url, event_tx));
    // `Ready` means the servicing loop below is about to run: commands and
    // agent actions are only usable from here on, so it must not be
    // published before the (bounded) startup navigation returned. The host
    // resets its loading flag on `Ready`, so a startup navigation that is
    // still running is reported as loading again right after it.
    let _ = event_tx.send(BrowserEvent::Ready);
    if startup_navigation_pending {
        let _ = event_tx.send(BrowserEvent::Loading(true));
    }

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
        driver.tick_safari_input(event_tx);
        for request in driver.tick_coordination(event_tx) {
            // A blocking action later in the batch must not delay the typed
            // timeout of a navigation or wait dispatched earlier in it, and a
            // navigation that hit its bound earlier in the batch must have its
            // session timeout restored and page state refreshed before the
            // next request runs.
            driver.tick_pending_navigation();
            driver.tick_pending_wait();
            driver.tick_classic_timeout_restore();
            driver.tick_page_state_refresh(event_tx);
            driver.service_agent_request(&request, event_tx);
        }
        if let Err(error) = driver.drain_bidi_events(event_tx) {
            tracing::warn!(backend = ?driver.config.browser.backend, "BiDi event pump failed: {error}");
            if driver.config.browser.backend == BackendKind::FirefoxBidi {
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
        driver.tick_pending_wait();
        driver.write_coordination(false);
        if let Some(status) = driver.service.process.child_status() {
            driver.settle_pending_wait_for_shutdown(Instant::now());
            let _ = event_tx.send(BrowserEvent::Stopped { code: status.code() });
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    driver.settle_pending_wait_for_shutdown(Instant::now());
    driver.close(event_tx);
    let _ = event_tx.send(BrowserEvent::Stopped { code: None });
}

impl Driver {
    fn start(
        config: &BrowserSessionConfig,
        process_control: &ChromeProcessControl,
        stop_requested: &AtomicBool,
    ) -> Result<Self, String> {
        let service = WebDriverService::start(&config.browser, process_control, || {
            stop_requested.load(Ordering::Acquire)
        })?;
        let response = match create_webdriver_session(&service, config, true) {
            Ok(response) => response,
            Err(error)
                if config.browser.backend == BackendKind::SafariWebDriver
                    && error.is_unsupported_websocket_capability() =>
            {
                create_webdriver_session(&service, config, false)
                    .map_err(|error| format!("failed to create classic Safari WebDriver session: {error}"))?
            }
            Err(error) => return Err(format!("failed to create WebDriver session: {error}")),
        };
        let NewSession {
            id: session_id,
            capabilities,
        } = parse_new_session_response(&response)?;
        let ws_url = capabilities.get("webSocketUrl").and_then(Value::as_str);
        let mut bidi = match ws_url {
            Some(url) => match connect_bidi_with_startup_retry(url, stop_requested) {
                Ok(link) => Some(link),
                Err(error) if config.browser.backend == BackendKind::SafariWebDriver => {
                    tracing::warn!("Safari BiDi endpoint was unavailable; continuing with classic WebDriver: {error}");
                    None
                }
                Err(error) => return Err(error),
            },
            None => None,
        };
        if config.browser.backend == BackendKind::FirefoxBidi && bidi.is_none() {
            service.delete_session(&session_id);
            return Err("Firefox did not return the required WebDriver BiDi webSocketUrl".to_string());
        }
        let mut context_id = bidi.as_mut().and_then(discover_context);
        if config.browser.backend == BackendKind::FirefoxBidi && context_id.is_none() {
            service.delete_session(&session_id);
            return Err("Firefox BiDi returned no top-level browsing context".to_string());
        }
        if config.browser.backend == BackendKind::FirefoxBidi
            && config.browser.automation_disclosure == AutomationDisclosurePolicy::MinimizeCommonSignals
            && let Some(link) = bidi.as_mut()
            && let Err(error) = install_common_signal_preload(link)
        {
            service.delete_session(&session_id);
            return Err(format!(
                "Firefox could not install pre-document automation disclosure minimization: {error}"
            ));
        }
        if let Some(link) = bidi.as_mut()
            && let Err(error) = subscribe(link, config.browser.backend, context_id.as_deref())
        {
            if config.browser.backend == BackendKind::FirefoxBidi {
                service.delete_session(&session_id);
                return Err(format!("Firefox BiDi event subscription failed: {error}"));
            }
            bidi = None;
            context_id = None;
        }
        let automation_ws = bidi.as_ref().and(ws_url).unwrap_or_default().to_string();
        let safari = initial_safari_input(&service, &session_id, config.browser.backend)?;
        Ok(Self {
            config: config.clone(),
            service,
            session_id,
            bidi,
            automation_ws,
            context_id,
            safari,
            actions: ActionState::default(),
            frames: AdaptiveFrames::new(),
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
            network: crate::network::NetworkCaptureState::default(),
            firefox_network: None,
            pending_http_bodies: VecDeque::new(),
        })
    }

    fn run_command(
        &mut self,
        command: BrowserCommand,
        events: &BrowserEventSender,
        user: bool,
    ) -> Result<bool, String> {
        if user && is_user_activity(&command) {
            self.stamp_user_active();
        }
        match command {
            BrowserCommand::Navigate(url) => self.navigate(&url, events).map(|()| false),
            BrowserCommand::Reload => self.reload(events).map(|()| false),
            BrowserCommand::Back => self.traverse(-1, events).map(|()| false),
            BrowserCommand::Forward => self.traverse(1, events).map(|()| false),
            BrowserCommand::SetViewport { width, height } => {
                self.set_viewport(width, height, events);
                Ok(false)
            }
            BrowserCommand::Input(input) => self.perform_input(input, events).map(|()| false),
            BrowserCommand::HandoffDone => {
                self.resolve_handoff(events);
                Ok(false)
            }
            BrowserCommand::Stop => Ok(true),
        }
    }

    fn active_capabilities(&self) -> crate::ActiveBackendCapabilities {
        crate::ActiveBackendCapabilities {
            backend: self.config.browser.backend,
            capabilities: self.config.browser.backend.capabilities(),
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
        let result = if self.config.browser.backend == BackendKind::FirefoxBidi {
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
        if self.config.browser.backend == BackendKind::SafariWebDriver && self.handle_safari_scrollbar_input(&input)? {
            return Ok(());
        }
        let (result, demand_frame) = if self.config.browser.backend == BackendKind::FirefoxBidi {
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
        result
    }

    fn capture_frame(&mut self, frame_slot: &FrameSlot, event_tx: &BrowserEventSender) {
        frame_slot.record_capture_request();
        let generation = self.generation;
        let context_id = self.context_id.clone();
        let result = if self.config.browser.backend == BackendKind::FirefoxBidi {
            self.classic_get("screenshot")
                .and_then(|response| {
                    webdriver_value(&response)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| "WebDriver screenshot response had no data".to_string())
                })
                .map(|data| (data, false))
        } else {
            self.classic_get("screenshot")
                .and_then(|response| {
                    webdriver_value(&response)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| "WebDriver screenshot response had no data".to_string())
                })
                .map(|data| (data, false))
        };
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
        if self.refresh_page_scroll_state(frame_slot) {
            // A page can scroll over visually identical pixels. Wake the host
            // even when the screenshot hash did not change so a scrollbar
            // overlay still follows the browser's authoritative position.
            event_tx.wake_ui();
        }
    }

    fn refresh_page_scroll_state(&mut self, frame_slot: &FrameSlot) -> bool {
        let now = Instant::now();
        if now < self.scrollbar.refresh_at {
            return false;
        }
        self.scrollbar.refresh_at = now + SCROLL_STATE_INTERVAL;
        let Ok(response) = self.classic_post(
            "execute/sync",
            &json!({ "script": PAGE_SCROLL_STATE_SCRIPT, "args": [] }),
        ) else {
            return false;
        };
        let Some(value) = webdriver_value(&response).cloned() else {
            return false;
        };
        let Ok(state) = serde_json::from_value::<PageScrollState>(value) else {
            return false;
        };
        self.scrollbar.sample(state);
        frame_slot.publish_page_scroll_state(state)
    }

    fn call_bidi(&mut self, method: &str, params: &Value, event_tx: &BrowserEventSender) -> Result<Value, String> {
        let link = self.bidi.as_mut().ok_or_else(|| "BiDi is unavailable".to_string())?;
        let outcome = link.call(COMMAND_TIMEOUT, method, params);
        for event in outcome.events {
            self.handle_bidi_event(&event, event_tx);
        }
        outcome.result.map_err(|error| error.to_string())
    }

    fn drain_bidi_events(&mut self, event_tx: &BrowserEventSender) -> Result<(), String> {
        let Some(link) = self.bidi.as_mut() else {
            return Ok(());
        };
        let events = link.drain(MAX_EVENT_BURST).map_err(|error| error.to_string())?;
        for event in events {
            self.handle_bidi_event(&event, event_tx);
        }
        Ok(())
    }

    fn handle_bidi_event(&mut self, event: &Value, event_tx: &BrowserEventSender) {
        if let Some(id) = event.get("id").and_then(Value::as_u64)
            && self.navigate_request_id == Some(id)
        {
            self.handle_bidi_navigate_response(event, event_tx);
            return;
        }
        if self.handle_network_bidi_event(event) {
            return;
        }
        let method = event.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = event.get("params").unwrap_or(&Value::Null);
        if let Some(context) = params.get("context").and_then(Value::as_str)
            && self.context_id.is_none()
        {
            self.context_id = Some(context.to_string());
        }
        if !bidi_event_targets_context(method, params, self.context_id.as_deref()) {
            return;
        }
        if method.ends_with("navigationStarted") {
            if consume_pending_history_start(
                &mut self.pending_classic_history_start,
                params.get("url").and_then(Value::as_str),
                Instant::now(),
            ) {
                return;
            }
            self.begin_navigation();
            let _ = event_tx.send(BrowserEvent::Loading(true));
            return;
        }
        if bidi_navigation_failed(method) {
            let navigation = params.get("navigation").and_then(Value::as_str);
            let url = params
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("the requested page");
            let message = format!("could not navigate to {url}");
            if let Some(pending) = self.pending_navigation.as_ref() {
                if !pending.correlates(navigation) {
                    // A superseded navigation failing late must not poison the
                    // state of the navigation that replaced it.
                    tracing::debug!(target: "browser", navigation, "ignoring failure of a superseded navigation");
                    return;
                }
                if pending.attribution_is_pending(navigation) {
                    // The dispatch reply has not named this navigation yet.
                    // Hold the failure without changing page-wide state; the
                    // reply either attributes it to this action or discards it
                    // as a late failure from the navigation it replaced.
                    self.observe_navigation_signal(crate::navigation::NavigationSignal::Failed {
                        message: &message,
                        id: navigation,
                    });
                    return;
                }
            }
            self.apply_navigation_failure_state(event_tx, &message);
            self.observe_navigation_signal(crate::navigation::NavigationSignal::Failed {
                message: &message,
                id: navigation,
            });
            return;
        }
        let navigation_complete = bidi_navigation_complete(method);
        if navigation_complete && !self.navigation_failed {
            let committed_url = params
                .get("url")
                .and_then(Value::as_str)
                .map_or_else(|| self.url.clone(), str::to_string);
            let previous_url = self.url.clone();
            let document_commit = self
                .challenge_loop
                .document_committed(&committed_url, params.get("navigation").and_then(Value::as_str));
            if committed_url != self.url {
                self.url = committed_url;
                self.coordination_dirty = true;
            }
            if document_commit == DocumentCommit::Recovered || previous_url != self.url {
                let _ = event_tx.send(BrowserEvent::UrlChanged(self.url.clone()));
            }
            if document_commit == DocumentCommit::Rejected && previous_url != self.url {
                let _ = event_tx.send(BrowserEvent::NavigationFailed(REJECTION_MESSAGE.to_string()));
            }
            self.retain_frame_during_navigation = false;
            let _ = event_tx.send(BrowserEvent::Loading(false));
            self.frames.demand();
            self.refresh_pending_at = Some(Instant::now() + Duration::from_millis(50));
            self.settle_navigation_from_bidi(method, params.get("navigation").and_then(Value::as_str));
        } else if method.ends_with("contextDestroyed") {
            let destroyed = params.get("context").and_then(Value::as_str);
            if destroyed == self.context_id.as_deref() {
                self.context_id = None;
                self.advance_generation();
            }
        }
    }

    fn advance_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.scrollbar.reset();
        if !self.retain_frame_during_navigation {
            self.frames.invalidate();
        }
    }
}

fn initial_safari_input(
    service: &WebDriverService,
    session_id: &str,
    backend: BackendKind,
) -> Result<Option<safari::InputState>, String> {
    if backend != BackendKind::SafariWebDriver {
        return Ok(None);
    }
    let response = service
        .http
        .get(&format!("/session/{session_id}/window"))
        .map_err(|error| format!("failed to read Safari window handle: {error}"))?;
    safari::InputState::from_window_response(&response).map(Some)
}

fn create_webdriver_session(
    service: &WebDriverService,
    config: &BrowserSessionConfig,
    request_bidi: bool,
) -> Result<Value, HttpError> {
    let capabilities = new_session_capabilities(&config.browser, &config.panel_local_id, request_bidi)
        .map_err(|error| HttpError::InvalidResponse(format!("invalid session capabilities: {error}")))?;
    service
        .http
        .post("/session", &json!({ "capabilities": { "alwaysMatch": capabilities } }))
}

fn new_session_capabilities(config: &BrowserConfig, panel_local_id: &str, request_bidi: bool) -> Result<Value, String> {
    match config.backend {
        BackendKind::FirefoxBidi => {
            validate_firefox_args(&config.extra_args)?;
            let profile = config.profile_dir(panel_local_id);
            prepare_profile(&profile)?;
            let mut options = Map::new();
            let mut args = Vec::with_capacity(4 + config.extra_args.len());
            if config.headless {
                args.push("-headless".to_string());
            }
            args.extend([
                "-no-remote".to_string(),
                "-profile".to_string(),
                profile.to_string_lossy().to_string(),
            ]);
            args.extend(config.extra_args.iter().cloned());
            options.insert("args".to_string(), json!(args));
            // Headless Firefox otherwise inherits GTK overlay scrollbars,
            // which fade completely out of screenshots and leave a streamed
            // browser panel with no visible drag target.
            options.insert(
                "prefs".to_string(),
                json!({
                    "widget.gtk.overlay-scrollbars.enabled": false,
                    "ui.useOverlayScrollbars": 0,
                }),
            );
            if let Some(command) = &config.firefox_command {
                let binary = resolve_binary(command).map_err(|error| error.to_string())?;
                options.insert("binary".to_string(), json!(binary));
            }
            Ok(json!({
                "browserName": "firefox",
                "webSocketUrl": true,
                "acceptInsecureCerts": false,
                "pageLoadStrategy": "eager",
                "timeouts": { "pageLoad": PAGE_LOAD_TIMEOUT_MILLIS },
                "moz:firefoxOptions": Value::Object(options),
            }))
        }
        BackendKind::SafariWebDriver => {
            let mut capabilities = Map::new();
            capabilities.insert("browserName".to_string(), json!("safari"));
            capabilities.insert("acceptInsecureCerts".to_string(), json!(false));
            capabilities.insert("timeouts".to_string(), json!({ "pageLoad": PAGE_LOAD_TIMEOUT_MILLIS }));
            if request_bidi {
                capabilities.insert("webSocketUrl".to_string(), json!(true));
            }
            Ok(Value::Object(capabilities))
        }
        BackendKind::ChromiumCdp => Err("Chromium does not create a WebDriver session".to_string()),
    }
}

fn validate_firefox_args(arguments: &[String]) -> Result<(), String> {
    for argument in arguments {
        let normalized = argument.trim_start_matches('-').to_ascii_lowercase();
        if matches!(normalized.as_str(), "profile" | "p" | "marionette") || normalized.starts_with("remote-debugging-")
        {
            return Err(format!(
                "browser.extra_args cannot override managed Firefox argument {argument:?}"
            ));
        }
    }
    Ok(())
}

fn discover_context(link: &mut JsonWsLink) -> Option<String> {
    let outcome = link.call(COMMAND_TIMEOUT, "browsingContext.getTree", &json!({ "maxDepth": 0 }));
    outcome.result.ok().and_then(|result| {
        result
            .get("contexts")?
            .as_array()?
            .first()?
            .get("context")?
            .as_str()
            .map(str::to_string)
    })
}

fn connect_bidi_with_startup_retry(url: &str, stop_requested: &AtomicBool) -> Result<JsonWsLink, String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match JsonWsLink::connect(url) {
            Ok(link) => return Ok(link),
            Err(JsonWsError::InvalidUrl(_)) => {
                return Err("browser returned an invalid non-loopback BiDi endpoint".to_string());
            }
            Err(error) => {
                if stop_requested.load(Ordering::Acquire) || Instant::now() >= deadline {
                    return Err(format!("failed to connect returned BiDi endpoint: {error}"));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn subscribe(link: &mut JsonWsLink, backend: BackendKind, context_id: Option<&str>) -> Result<(), String> {
    subscribe_bidi_events(link, &base_bidi_events(), None)?;
    if backend == BackendKind::FirefoxBidi {
        let context = context_id.ok_or_else(|| "Firefox BiDi returned no top-level browsing context".to_string())?;
        subscribe_bidi_events(link, &["network.responseStarted"], Some(context))?;
    }
    Ok(())
}

fn subscribe_bidi_events(link: &mut JsonWsLink, events: &[&str], context: Option<&str>) -> Result<(), String> {
    let params = bidi_subscription_params(events, context);
    link.call(COMMAND_TIMEOUT, "session.subscribe", &params)
        .result
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn bidi_subscription_params(events: &[&str], context: Option<&str>) -> Value {
    let mut params = json!({ "events": events });
    if let Some(context) = context {
        params["contexts"] = json!([context]);
    }
    params
}

fn base_bidi_events() -> Vec<&'static str> {
    vec![
        "browsingContext.contextCreated",
        "browsingContext.contextDestroyed",
        "browsingContext.navigationStarted",
        "browsingContext.navigationFailed",
        "browsingContext.fragmentNavigated",
        "browsingContext.domContentLoaded",
        "browsingContext.load",
    ]
}

fn install_common_signal_preload(link: &mut JsonWsLink) -> Result<(), String> {
    link.call(
        COMMAND_TIMEOUT,
        "script.addPreloadScript",
        &json!({ "functionDeclaration": COMMON_SIGNAL_PRELOAD_FUNCTION }),
    )
    .result
    .map(|_| ())
    .map_err(|error| error.to_string())
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

fn safe_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn parse_new_session_response(response: &Value) -> Result<NewSession, String> {
    let value = response.get("value").unwrap_or(response);
    let id = value
        .get("sessionId")
        .or_else(|| response.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|id| safe_session_id(id))
        .ok_or_else(|| "WebDriver returned no safe session id".to_string())?
        .to_string();
    let capabilities = value
        .get("capabilities")
        .or_else(|| response.get("capabilities"))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(NewSession { id, capabilities })
}

fn normalize_url(url: &str) -> &str {
    if url == "about:blank" { "" } else { url }
}

fn capture_is_current(
    capture_generation: u64,
    current_generation: u64,
    capture_context: Option<&str>,
    current_context: Option<&str>,
) -> bool {
    capture_generation == current_generation && capture_context == current_context
}

fn bidi_navigation_complete(method: &str) -> bool {
    method.ends_with("domContentLoaded") || method.ends_with("fragmentNavigated") || method.ends_with("load")
}

fn bidi_navigation_failed(method: &str) -> bool {
    method.ends_with("navigationFailed")
}

fn bidi_event_targets_context(method: &str, params: &Value, context_id: Option<&str>) -> bool {
    let context_scoped = method.ends_with("navigationStarted")
        || bidi_navigation_failed(method)
        || bidi_navigation_complete(method)
        || method.ends_with("contextDestroyed");
    !context_scoped || params.get("context").and_then(Value::as_str) == context_id
}

fn consume_pending_history_start(pending: &mut Option<PendingHistoryStart>, url: Option<&str>, now: Instant) -> bool {
    let Some(pending) = pending.take() else {
        return false;
    };
    now <= pending.expires_at && url == Some(pending.url.as_str())
}

#[cfg(test)]
mod tests {
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

    use super::{
        AdaptiveFrames, PAGE_LOAD_TIMEOUT_MILLIS, PendingHistoryStart, base_bidi_events, bidi_event_targets_context,
        bidi_navigation_complete, bidi_navigation_failed, bidi_subscription_params, capture_is_current,
        classic_navigation_committed, consume_pending_history_start, new_session_capabilities,
        parse_new_session_response, safe_session_id, validate_firefox_args,
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
