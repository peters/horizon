//! The per-panel browser driver thread.
//!
//! One thread per browser panel, mirroring how terminal panels run their
//! alacritty event loops: it owns the Chrome process and the browser-level
//! CDP connection, decodes screencast frames into a shared
//! [`FrameSlot`], and reports lightweight [`BrowserEvent`]s over an mpsc
//! channel that the main thread drains each frame. Input and navigation
//! commands travel the other way over [`BrowserCommand`].
//!
//! CDP behaviors encoded here (validated against real Chrome in the L0
//! spike):
//! - flattened sessions: `sessionId` is a string; events carry it at the
//!   top level; commands need it at the top level, and
//!   `Page.screencastFrameAck` needs it in `params` as well.
//! - screencasts are **change-driven**: frames only arrive when the page
//!   repaints, so a static page costs nothing.
//! - a cross-document navigation — especially one triggered by *another*
//!   CDP client (e.g. the `hb` agent CLI) — temporarily breaks the
//!   session's page binding and returns "Not attached to an active page".
//!   Recovery is a backoff-retried `Page.startScreencast` (~100 ms steps),
//!   with a full re-attach as the last resort.
//! - the screencast is implicitly terminated by navigation, so it is
//!   restarted on every top-level `Page.frameNavigated`.

use base64::Engine;

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::browser::cdp::{CdpEvent, CdpLink, CdpMsg};
use crate::browser::frames::FrameSlot;
use crate::browser::manifest::{self, BrowserManifest};
use crate::browser::process::ChromeProcess;
use crate::browser::{BrowserConfig, BrowserInput};

/// What the driver reports to the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum BrowserEvent {
    /// Chrome is up and the page session is attached.
    Ready,
    Title(String),
    UrlChanged(String),
    Loading(bool),
    /// A new decoded frame is available in the panel's frame slot.
    Frame { seq: u64 },
    /// Non-fatal problem surfaced to the panel body.
    Warning(String),
    /// The driver stopped.
    Stopped { code: Option<i32> },
}

/// What the panel/UI asks the driver to do.
#[derive(Clone, Debug)]
pub enum BrowserCommand {
    Navigate(String),
    Reload,
    Back,
    Forward,
    SetViewport { width: u32, height: u32 },
    Input(BrowserInput),
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
}

/// The panel-side handle to a running driver.
pub struct BrowserSession {
    command_tx: mpsc::Sender<BrowserCommand>,
    pub frame_slot: Arc<FrameSlot>,
    pub event_rx: mpsc::Receiver<BrowserEvent>,
}

impl BrowserSession {
    pub fn send(&self, command: BrowserCommand) -> bool {
        self.command_tx.send(command).is_ok()
    }
}

const WS_URL_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const RESTART_BACKOFF: Duration = Duration::from_millis(100);
const MAX_RESTART_ATTEMPTS: u32 = 8;
const MANIFEST_MIN_INTERVAL: Duration = Duration::from_millis(200);

/// Spawn the driver thread. Chrome startup problems are reported through
/// the event channel rather than failing here.
pub fn start_session(config: BrowserSessionConfig) -> Result<BrowserSession, String> {
    let frame_slot = Arc::new(FrameSlot::new());
    let (command_tx, command_rx) = mpsc::channel::<BrowserCommand>();
    let (event_tx, event_rx) = mpsc::channel::<BrowserEvent>();
    let slot = Arc::clone(&frame_slot);
    std::thread::Builder::new()
        .name("browser-driver".into())
        .spawn(move || {
            run_driver(config, event_tx, command_rx, slot);
        })
        .map_err(|e| format!("failed to spawn browser driver: {e}"))?;
    Ok(BrowserSession {
        command_tx,
        frame_slot,
        event_rx,
    })
}

fn run_driver(
    config: BrowserSessionConfig,
    event_tx: mpsc::Sender<BrowserEvent>,
    command_rx: mpsc::Receiver<BrowserCommand>,
    frame_slot: Arc<FrameSlot>,
) {
    let launch = match build_launch(&config) {
        Ok(launch) => launch,
        Err(message) => {
            let _ = event_tx.send(BrowserEvent::Warning(message));
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return;
        }
    };

    let mut chrome = match ChromeProcess::spawn(&launch) {
        Ok(chrome) => chrome,
        Err(error) => {
            let _ = event_tx.send(BrowserEvent::Warning(format!("failed to start chrome: {error}")));
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return;
        }
    };
    let ws_url = match chrome.wait_ws_url(WS_URL_TIMEOUT) {
        Ok(url) => url,
        Err(error) => {
            let _ = event_tx.send(BrowserEvent::Warning(format!("no DevTools endpoint: {error}")));
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return;
        }
    };
    let mut link = match CdpLink::connect(&ws_url) {
        Ok(link) => link,
        Err(error) => {
            let _ = event_tx.send(BrowserEvent::Warning(format!("CDP connect failed: {error}")));
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return;
        }
    };

    let _ = link.call_and_drain(CALL_TIMEOUT, "Target.setDiscoverTargets", serde_json::json!({ "discover": true }), None);
    let _ = link.call_and_drain(
        CALL_TIMEOUT,
        "Target.setAutoAttach",
        serde_json::json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
        None,
    );

    let mut state = DriverState::new(&config, &ws_url);
    state.write_manifest(true);

    'outer: loop {
        // 1. Commands from the UI.
        while let Ok(command) = command_rx.try_recv() {
            match command {
                BrowserCommand::Stop => {
                    chrome.kill();
                    state.remove_manifest();
                    let _ = event_tx.send(BrowserEvent::Stopped { code: None });
                    break 'outer;
                }
                BrowserCommand::Navigate(url) => {
                    state.url = url.clone();
                    state.pending_restart_at = Some(Instant::now());
                    state.send_page_command(&mut link, "Page.navigate", serde_json::json!({ "url": url }));
                    state.write_manifest(false);
                    let _ = event_tx.send(BrowserEvent::UrlChanged(state.url.clone()));
                    let _ = event_tx.send(BrowserEvent::Loading(true));
                }
                BrowserCommand::Reload => {
                    state.pending_restart_at = Some(Instant::now());
                    state.send_page_command(&mut link, "Page.reload", serde_json::json!({}));
                }
                BrowserCommand::Back => {
                    state.pending_restart_at = Some(Instant::now());
                    state.send_page_command(&mut link, "History.back", serde_json::json!({}));
                }
                BrowserCommand::Forward => {
                    state.pending_restart_at = Some(Instant::now());
                    state.send_page_command(&mut link, "History.forward", serde_json::json!({}));
                }
                BrowserCommand::SetViewport { width, height } => {
                    if width > 0
                        && height > 0
                        && (width, height) != (state.viewport_w, state.viewport_h)
                    {
                        state.viewport_w = width;
                        state.viewport_h = height;
                        state.send_page_command(
                            &mut link,
                            "Emulation.setDeviceMetricsOverride",
                            serde_json::json!({
                                "width": width,
                                "height": height,
                                "deviceScaleFactor": 1,
                                "mobile": false,
                            }),
                        );
                    }
                }
                BrowserCommand::Input(input) => {
                    let (method, params) = input.cdp();
                    state.send_page_command_fire(&mut link, method, params);
                }
            }
        }

        // 2. CDP messages: responses and events.
        match link.read_one() {
            Ok(Some(message)) => state.handle_message(&mut link, &event_tx, &frame_slot, message),
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
                break 'outer;
            }
        }

        // 3. Backoff-retried screencast restart / re-attach.
        state.pending_restart_tick(&mut link);

        // 4. Chrome process liveness.
        if let Some(status) = chrome.child_status() {
            state.remove_manifest();
            let _ = event_tx.send(BrowserEvent::Stopped { code: status.code() });
            break 'outer;
        }

        std::thread::sleep(Duration::from_millis(5));
    }
    chrome.kill();
}

fn build_launch(config: &BrowserSessionConfig) -> Result<crate::browser::process::ChromeLaunch, String> {
    let command = crate::browser::process::resolve_binary_or_default(&config.browser.command)
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())?;
    let home = crate::horizon_home::HorizonHome::resolve();
    let profile_dir = config
        .browser
        .profile_root
        .clone()
        .unwrap_or_else(|| home.browser_profile_dir(&config.panel_local_id));
    Ok(crate::browser::process::ChromeLaunch {
        command,
        profile_dir,
        width: config.width,
        height: config.height,
        extra_args: config.browser.extra_args.clone(),
    })
}

/// Driver-side state machine for one page session.
#[derive(Debug)]
struct DriverState {
    config: BrowserSessionConfig,
    browser_ws: String,
    session_id: Option<String>,
    target_id: Option<String>,
    viewport_w: u32,
    viewport_h: u32,
    url: String,
    title: String,
    initial_navigated: bool,
    screencast_on: bool,
    pending_restart_at: Option<Instant>,
    restart_attempts: u32,
    pending_reattach: bool,
    reattach_in_flight: bool,
    last_manifest_write: Instant,
    manifest_dirty: bool,
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
            viewport_w: config.width,
            viewport_h: config.height,
            url,
            title: String::new(),
            initial_navigated: false,
            screencast_on: false,
            pending_restart_at: None,
            restart_attempts: 0,
            pending_reattach: false,
            reattach_in_flight: false,
            last_manifest_write: Instant::now() - MANIFEST_MIN_INTERVAL,
            manifest_dirty: true,
        }
    }

    fn send_page_command(&self, link: &mut CdpLink, method: &str, params: serde_json::Value) {
        if let Some(ref session) = self.session_id
            && let Err(error) = link.call_and_drain(CALL_TIMEOUT, method, params, Some(session))
        {
            tracing::debug!(target: "browser", "cdp {method} failed: {error}");
        }
    }

    fn send_page_command_fire(&self, link: &mut CdpLink, method: &str, params: serde_json::Value) {
        if let Some(ref session) = self.session_id {
            let _ = link.send_fire(method, params, Some(session));
        }
    }

    fn screencast_params(&self) -> serde_json::Value {
        serde_json::json!({
            "format": "jpeg",
            "quality": self.config.browser.quality,
            "maxFPS": self.config.browser.max_fps,
            "everyNthFrame": 1,
        })
    }

    fn attach_setup(&mut self, link: &mut CdpLink, session: &str, target: &str) {
        self.session_id = Some(session.to_string());
        self.target_id = Some(target.to_string());
        let _ = link.call_and_drain(CALL_TIMEOUT, "Page.enable", serde_json::json!({}), Some(session));
        let _ = link.call_and_drain(
            CALL_TIMEOUT,
            "Emulation.setDeviceMetricsOverride",
            serde_json::json!({
                "width": self.viewport_w,
                "height": self.viewport_h,
                "deviceScaleFactor": 1,
                "mobile": false,
            }),
            Some(session),
        );
        self.start_screencast(link);
    }

    fn start_screencast(&mut self, link: &mut CdpLink) {
        let Some(ref session) = self.session_id else {
            return;
        };
        match link.call_and_drain(CALL_TIMEOUT, "Page.startScreencast", self.screencast_params(), Some(session)) {
            Ok(_) => {
                self.screencast_on = true;
                self.restart_attempts = 0;
                self.pending_restart_at = None;
            }
            Err(error) => self.note_screencast_failure(&error.to_string()),
        }
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
            self.pending_restart_at = Some(Instant::now() + RESTART_BACKOFF);
        }
    }

    fn pending_restart_tick(&mut self, link: &mut CdpLink) {
        if self.pending_reattach && !self.reattach_in_flight {
            self.pending_reattach = false;
            if let Some(ref target) = self.target_id {
                self.reattach_in_flight = true;
                self.session_id = None;
                if let Err(error) = link.send_request(
                    "Target.attachToTarget",
                    serde_json::json!({ "targetId": target, "flatten": true }),
                    None,
                ) {
                    tracing::warn!(target: "browser", "re-attach request failed: {error}");
                    self.reattach_in_flight = false;
                    self.pending_restart_at = Some(Instant::now() + RESTART_BACKOFF);
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
            self.start_screencast(link);
        }
    }

    fn handle_message(
        &mut self,
        link: &mut CdpLink,
        event_tx: &mpsc::Sender<BrowserEvent>,
        frame_slot: &Arc<FrameSlot>,
        message: CdpMsg,
    ) {
        if message.event().is_some() {
            let event = message.event().unwrap();
            self.handle_event(link, event_tx, frame_slot, event);
            return;
        }
        let CdpMsg::Response {
            id,
            result,
            error,
            ..
        } = message
        else {
            return;
        };
        if self.reattach_in_flight {
            self.reattach_in_flight = false;
            match error {
                Some(error) => {
                    tracing::warn!(target: "browser", "re-attach rejected: {error}");
                    self.pending_restart_at = Some(Instant::now() + RESTART_BACKOFF);
                }
                None => {
                    if let Some(session) = result.as_ref().and_then(|r| r.get("sessionId")).and_then(|s| s.as_str())
                        && let Some(target) = self.target_id.clone()
                    {
                        self.attach_setup(link, session, &target);
                    }
                }
            }
            return;
        }
        if let Some(error) = error {
            tracing::debug!(target: "browser", "cdp response error id={id}: {error}");
        }
    }

    fn handle_event(
        &mut self,
        link: &mut CdpLink,
        event_tx: &mpsc::Sender<BrowserEvent>,
        frame_slot: &Arc<FrameSlot>,
        event: CdpEvent<'_>,
    ) {
        let on_page_session = event.session_id.is_some_and(|s| Some(s) == self.session_id.as_deref());
        match event.method {
            "Target.attachedToTarget" => {
                let Some(session) = event.session_id else {
                    return;
                };
                let target_info = event.params.get("targetInfo");
                let target_type = target_info.and_then(|t| t.get("type")).and_then(|t| t.as_str()).unwrap_or("");
                let Some(target_id) = target_info.and_then(|t| t.get("targetId")).and_then(|t| t.as_str()) else {
                    return;
                };
                if target_type != "page" {
                    return;
                }
                self.attach_setup(link, session, target_id);
                self.write_manifest(true);
                let _ = event_tx.send(BrowserEvent::Ready);
                if let Some(ref initial) = self.config.initial_url
                    && initial != "about:blank"
                    && !self.initial_navigated
                {
                    self.initial_navigated = true;
                    self.send_page_command(link, "Page.navigate", serde_json::json!({ "url": initial }));
                    self.pending_restart_at = Some(Instant::now());
                }
            }
            "Target.detachedFromTarget" => {
                if on_page_session {
                    self.session_id = None;
                    self.screencast_on = false;
                }
            }
            "Target.targetDestroyed" => {
                let destroyed = event.params.get("targetId").and_then(|t| t.as_str());
                if self.target_id.as_deref() == destroyed {
                    self.session_id = None;
                    self.target_id = None;
                    self.screencast_on = false;
                }
            }
            "Page.frameNavigated" => {
                if !on_page_session {
                    return;
                }
                if let Some(url) = event
                    .params
                    .get("frame")
                    .and_then(|f| f.get("url"))
                    .and_then(|u| u.as_str())
                    && !url.is_empty()
                {
                    self.url = url.to_string();
                }
                self.pending_restart_at = Some(Instant::now());
                self.write_manifest(false);
                let _ = event_tx.send(BrowserEvent::UrlChanged(self.url.clone()));
                let _ = event_tx.send(BrowserEvent::Loading(true));
            }
            "Page.loadEventFired" => {
                if on_page_session {
                    let _ = event_tx.send(BrowserEvent::Loading(false));
                }
            }
            "Page.titleUpdated" => {
                if !on_page_session {
                    return;
                }
                if let Some(title) = event.params.get("title").and_then(|t| t.as_str()) {
                    self.title = title.to_string();
                    self.write_manifest(false);
                    let _ = event_tx.send(BrowserEvent::Title(title.to_string()));
                }
            }
            "Page.screencastFrame" => {
                if !on_page_session {
                    return;
                }
                let Some(data) = event.params.get("data").and_then(|d| d.as_str()) else {
                    return;
                };
                let Ok(jpeg) = base64::engine::general_purpose::STANDARD.decode(data) else {
                    return;
                };
                // Ack so the stream continues: params.sessionId echoes the
                // frame's session identifier, the top-level sessionId scopes
                // the call (flattened sessions).
                let frame_session = event
                    .params
                    .get("sessionId")
                    .cloned()
                    .or_else(|| event.session_id.map(|s| serde_json::Value::String(s.to_string())));
                let ack_params = match frame_session {
                    Some(value) => serde_json::json!({ "sessionId": value }),
                    None => serde_json::json!({}),
                };
                let _ = link.send_fire("Page.screencastFrameAck", ack_params, event.session_id);
                if let Some(seq) = frame_slot.store_jpeg(&jpeg) {
                    let _ = event_tx.send(BrowserEvent::Frame { seq });
                }
            }
            _ => {}
        }
    }

    /// Persist the shared manifest, preserving agent-owned fields.
    fn write_manifest(&mut self, force: bool) {
        if !force && !self.manifest_dirty {
            return;
        }
        if !force && self.last_manifest_write.elapsed() < MANIFEST_MIN_INTERVAL {
            self.manifest_dirty = true;
            return;
        }
        self.manifest_dirty = false;
        self.last_manifest_write = Instant::now();
        let local_id = &self.config.panel_local_id;
        let existing = manifest::read(local_id);
        if existing.is_none() && !force {
            // No manifest yet (agent may create it later); skip the write.
            self.manifest_dirty = true;
            return;
        }
        let mut manifest = existing.unwrap_or(BrowserManifest {
            panel_local_id: local_id.clone(),
            ..Default::default()
        });
        manifest.browser_ws = self.browser_ws.clone();
        manifest.target_id = self.target_id.clone().unwrap_or_default();
        manifest.url = self.url.clone();
        manifest.title = self.title.clone();
        manifest.updated_at = manifest::now_millis();
        let _ = manifest::write(&manifest);
    }

    fn remove_manifest(&self) {
        manifest::remove(&self.config.panel_local_id);
    }
}
