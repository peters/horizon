//! Chrome process startup and initial CDP target attachment.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::cdp::CdpLink;
use crate::disclosure::{
    CHROMIUM_DISCLOSURE_BOOTSTRAP_URL, CHROMIUM_USER_AGENT_METADATA_EXPRESSION, chromium_user_agent_needs_override,
};
use crate::frames::FrameSlot;
use crate::process::{ChromeError, ChromeProcess, ChromeProcessControl};
use crate::{AutomationDisclosurePolicy, BrowserConfig};

use super::shared::{DriverProcess, SharedBrowserSession, SharedDriverReservation};

use super::{
    BrowserEvent, BrowserEventSender, BrowserSessionConfig, CALL_TIMEOUT, CommandReceiver, DriverState, WS_URL_TIMEOUT,
    run_loop,
};

const DEVTOOLS_PORT_STARTUP_ATTEMPTS: usize = 3;
/// Poll interval while the disclosure target loads its bootstrap page.
const DISCLOSURE_DOCUMENT_POLL: Duration = Duration::from_millis(20);
const DEVTOOLS_PORT_REAP_FAILURE: &str =
    "failed to reap Chromium after a DevTools-port conflict; aborting retry to preserve exact process ownership";

/// Settles process registration and resolves the driver's teardown signal
/// exactly once, on thread exit.
struct DriverCompletion {
    group: Option<SharedDriverReservation>,
    completion_tx: Option<mpsc::Sender<()>>,
    process_control: ChromeProcessControl,
}

impl Drop for DriverCompletion {
    fn drop(&mut self) {
        self.group.take();
        self.process_control.mark_registration_settled();
        if let Some(tx) = self.completion_tx.take() {
            let _ = tx.send(());
        }
    }
}

fn cancel_startup_if_requested(
    requested: &AtomicBool,
    chrome: &mut DriverProcess,
    event_tx: &BrowserEventSender,
) -> bool {
    if !requested.load(Ordering::Acquire) {
        return false;
    }
    let _ = chrome.kill();
    let _ = event_tx.send(BrowserEvent::Stopped { code: None });
    true
}

fn cancel_target_startup(
    stop: &AtomicBool,
    chrome: &mut DriverProcess,
    link: &mut CdpLink,
    target: &str,
    events: &BrowserEventSender,
) -> bool {
    if !stop.load(Ordering::Acquire) {
        return false;
    }
    chrome.close_page(link, target);
    let _ = events.send(BrowserEvent::Stopped { code: None });
    true
}

struct DriverConnection {
    chrome: DriverProcess,
    link: CdpLink,
    ws_url: String,
    target_id: String,
    session_id: String,
    native_user_agent_metadata: Option<serde_json::Value>,
}

fn initialize_driver(
    config: &BrowserSessionConfig,
    event_tx: &BrowserEventSender,
    stop_requested: &AtomicBool,
    process_control: &ChromeProcessControl,
    group: Option<&SharedBrowserSession>,
) -> Option<DriverConnection> {
    let mut launch = match build_launch(config) {
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
    let started = if let Some(group) = group {
        launch.profile_dir = profile_dir(&config.browser, group.profile_id());
        group.acquire(
            &launch,
            config.browser.hide_native_window,
            stop_requested,
            process_control,
        )
    } else {
        start_chrome(&launch, stop_requested, process_control)
            .map(|connection| connection.map(|(process, endpoint)| (DriverProcess::Exclusive(process), endpoint)))
    };
    let (mut chrome, ws_url) = match started {
        Ok(Some(connection)) => connection,
        Ok(None) => {
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return None;
        }
        Err(message) => {
            let _ = event_tx.send(BrowserEvent::Warning(message));
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
            let _ = chrome.kill();
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return None;
        }
    };
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    initialize_target(config, event_tx, stop_requested, chrome, link, ws_url)
}

pub(super) fn start_chrome(
    launch: &crate::process::ChromeLaunch,
    stop_requested: &AtomicBool,
    process_control: &ChromeProcessControl,
) -> Result<Option<(ChromeProcess, String)>, String> {
    run_chrome_startup_attempts(launch.automation_disclosure, || {
        if stop_requested.load(Ordering::Acquire) {
            return Ok(ChromeStartupAttempt::Cancelled);
        }
        let mut chrome = ChromeProcess::spawn(launch, process_control.clone())
            .map_err(|error| format!("failed to start chrome: {error}"))?;
        let outcome = match chrome.wait_ws_url(WS_URL_TIMEOUT, || stop_requested.load(Ordering::Acquire)) {
            Ok(Some(url)) => ChromeStartupAttempt::Connected((chrome, url)),
            Ok(None) => {
                let _ = chrome.kill();
                ChromeStartupAttempt::Cancelled
            }
            Err(error) => ChromeStartupAttempt::Failed {
                error,
                reaped: chrome.kill(),
            },
        };
        Ok(outcome)
    })
}

enum ChromeStartupAttempt<T> {
    Connected(T),
    Cancelled,
    Failed { error: ChromeError, reaped: bool },
}

fn run_chrome_startup_attempts<T>(
    automation_disclosure: AutomationDisclosurePolicy,
    mut run_attempt: impl FnMut() -> Result<ChromeStartupAttempt<T>, String>,
) -> Result<Option<T>, String> {
    for attempt_number in 1..=DEVTOOLS_PORT_STARTUP_ATTEMPTS {
        match run_attempt()? {
            ChromeStartupAttempt::Connected(connection) => return Ok(Some(connection)),
            ChromeStartupAttempt::Cancelled => return Ok(None),
            ChromeStartupAttempt::Failed { error, reaped }
                if automation_disclosure == AutomationDisclosurePolicy::MinimizeCommonSignals
                    && error.is_devtools_port_conflict()
                    && attempt_number < DEVTOOLS_PORT_STARTUP_ATTEMPTS =>
            {
                require_reaped_before_port_retry(reaped)?;
                tracing::warn!(
                    attempt = attempt_number,
                    max_attempts = DEVTOOLS_PORT_STARTUP_ATTEMPTS,
                    "Chromium DevTools port handoff collided; retrying with a fresh reservation"
                );
            }
            ChromeStartupAttempt::Failed { error, .. } => {
                return Err(format!("no DevTools endpoint: {error}"));
            }
        }
    }
    Err("Chromium exhausted its bounded DevTools-port startup attempts".to_string())
}

fn require_reaped_before_port_retry(reaped: bool) -> Result<(), &'static str> {
    if reaped {
        Ok(())
    } else {
        Err(DEVTOOLS_PORT_REAP_FAILURE)
    }
}

fn initialize_target(
    config: &BrowserSessionConfig,
    event_tx: &BrowserEventSender,
    stop_requested: &AtomicBool,
    mut chrome: DriverProcess,
    mut link: CdpLink,
    ws_url: String,
) -> Option<DriverConnection> {
    let _ = call_during_startup(
        &mut link,
        stop_requested,
        "Target.setDiscoverTargets",
        &serde_json::json!({ "discover": true }),
    );
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    // Resolve the caller page before creating the hidden metadata target so
    // target ordering can never bind the panel to the temporary page.
    let existing_target = if chrome.is_shared() {
        chrome.registered_target()
    } else {
        first_page_target(&mut link, stop_requested)
    };
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    let Some(target_id) = existing_target.or_else(|| create_page_target(&mut link, stop_requested)) else {
        if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
            return None;
        }
        let _ = event_tx.send(BrowserEvent::Warning("no page target found".to_string()));
        let _ = chrome.kill();
        let _ = event_tx.send(BrowserEvent::Stopped { code: None });
        return None;
    };
    chrome.register_target(&target_id);
    if cancel_target_startup(stop_requested, &mut chrome, &mut link, &target_id, event_tx) {
        return None;
    }
    let native_user_agent_metadata =
        match prepare_disclosure_metadata(&mut link, &chrome, stop_requested, config.browser.automation_disclosure) {
            Ok(metadata) => metadata,
            Err(error) => {
                let _ = event_tx.send(BrowserEvent::Warning(format!(
                    "browser disclosure bootstrap failed: {error}"
                )));
                chrome.close_page(&mut link, &target_id);
                let _ = event_tx.send(BrowserEvent::Stopped { code: None });
                return None;
            }
        };
    if cancel_target_startup(stop_requested, &mut chrome, &mut link, &target_id, event_tx) {
        return None;
    }
    // Only attach this page and its children. Browser-wide auto-attachment
    // would expose siblings' targets to this panel's input/capture bridges.
    let Some(session_id) = link
        .call_and_drain_until(
            CALL_TIMEOUT,
            "Target.attachToTarget",
            &serde_json::json!({ "targetId": target_id, "flatten": true }),
            None,
            || stop_requested.load(Ordering::Acquire),
        )
        .result
        .ok()
        .and_then(|result| result.get("sessionId").and_then(|s| s.as_str()).map(str::to_string))
    else {
        let _ = event_tx.send(BrowserEvent::Warning("initial attach failed".to_string()));
        chrome.close_page(&mut link, &target_id);
        let _ = event_tx.send(BrowserEvent::Stopped { code: None });
        return None;
    };
    if cancel_target_startup(stop_requested, &mut chrome, &mut link, &target_id, event_tx) {
        return None;
    }
    Some(DriverConnection {
        chrome,
        link,
        ws_url,
        target_id,
        session_id,
        native_user_agent_metadata,
    })
}

fn prepare_disclosure_metadata(
    link: &mut CdpLink,
    chrome: &DriverProcess,
    stop_requested: &AtomicBool,
    policy: AutomationDisclosurePolicy,
) -> Result<Option<serde_json::Value>, String> {
    if policy == AutomationDisclosurePolicy::BrowserDefault {
        return Ok(None);
    }
    let version = call_during_startup(link, stop_requested, "Browser.getVersion", &serde_json::json!({}))
        .map_err(|error| format!("Browser.getVersion: {error}"))?;
    if !chromium_user_agent_needs_override(&version).map_err(str::to_string)? {
        return Ok(None);
    }
    let created = call_during_startup(
        link,
        stop_requested,
        "Target.createTarget",
        &serde_json::json!({
            "url": CHROMIUM_DISCLOSURE_BOOTSTRAP_URL,
            "background": true,
        }),
    )
    .map_err(|error| format!("Target.createTarget: {error}"))?;
    let target_id = created
        .get("targetId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Target.createTarget omitted targetId".to_string())?
        .to_string();

    chrome.register_target(&target_id);
    let metadata = read_disclosure_metadata_from_target(link, stop_requested, &target_id);
    let close_result = link
        .call_and_drain(
            CALL_TIMEOUT,
            "Target.closeTarget",
            &serde_json::json!({ "targetId": target_id }),
            None,
        )
        .result
        .map_err(|error| format!("Target.closeTarget: {error}"))
        .and_then(|result| {
            result
                .get("success")
                .and_then(serde_json::Value::as_bool)
                .is_some_and(|success| success)
                .then_some(())
                .ok_or_else(|| "Target.closeTarget did not close the disclosure target".to_string())
        });
    if close_result.is_ok() {
        chrome.forget_closed_target(&target_id);
    }
    match (metadata, close_result) {
        (Ok(metadata), Ok(())) => Ok(Some(metadata)),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

fn read_disclosure_metadata_from_target(
    link: &mut CdpLink,
    stop_requested: &AtomicBool,
    target_id: &str,
) -> Result<serde_json::Value, String> {
    let attached = call_during_startup(
        link,
        stop_requested,
        "Target.attachToTarget",
        &serde_json::json!({ "targetId": target_id, "flatten": true }),
    )
    .map_err(|error| format!("Target.attachToTarget: {error}"))?;
    let session_id = attached
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Target.attachToTarget omitted sessionId".to_string())?;
    call_in_target(
        link,
        stop_requested,
        session_id,
        CALL_TIMEOUT,
        "Runtime.enable",
        &serde_json::json!({}),
    )?;
    wait_for_disclosure_document(link, stop_requested, session_id, CALL_TIMEOUT)?;
    call_in_target(
        link,
        stop_requested,
        session_id,
        CALL_TIMEOUT,
        "Runtime.evaluate",
        &serde_json::json!({
            "expression": CHROMIUM_USER_AGENT_METADATA_EXPRESSION,
            "awaitPromise": true,
            "returnByValue": true,
        }),
    )
}

/// `Target.createTarget` can answer before the new target commits its URL. Until then the
/// target shows its initial `about:blank` document, which is not a secure context, so
/// `navigator.userAgentData` is absent there. Read the metadata only from the bootstrap page.
fn wait_for_disclosure_document(
    link: &mut CdpLink,
    stop_requested: &AtomicBool,
    session_id: &str,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut shown = "no document URL".to_string();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "the disclosure target did not show {CHROMIUM_DISCLOSURE_BOOTSTRAP_URL}; last result: {shown}"
            ));
        }
        // Each read gets only the remaining time, so the wait ends at the deadline.
        let location = call_in_target(
            link,
            stop_requested,
            session_id,
            remaining.min(CALL_TIMEOUT),
            "Runtime.evaluate",
            &serde_json::json!({ "expression": "location.href", "returnByValue": true }),
        );
        shown = match location {
            Ok(result) => result
                .pointer("/result/value")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("no document URL")
                .to_string(),
            Err(error) => error,
        };
        if shown == CHROMIUM_DISCLOSURE_BOOTSTRAP_URL {
            return Ok(());
        }
        if stop_requested.load(Ordering::Acquire) {
            return Err("browser startup was cancelled".to_string());
        }
        std::thread::sleep(DISCLOSURE_DOCUMENT_POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn call_in_target(
    link: &mut CdpLink,
    stop_requested: &AtomicBool,
    session_id: &str,
    timeout: Duration,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    link.call_and_drain_until(timeout, method, params, Some(session_id), || {
        stop_requested.load(Ordering::Acquire)
    })
    .result
    .map_err(|error| format!("{method}: {error}"))
}

fn call_during_startup(
    link: &mut CdpLink,
    stop_requested: &AtomicBool,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, crate::cdp::CdpError> {
    link.call_and_drain_until(CALL_TIMEOUT, method, params, None, || {
        stop_requested.load(Ordering::Acquire)
    })
    .result
}

pub(super) struct DriverLaunch {
    pub(super) completion_tx: mpsc::Sender<()>,
    pub(super) process_control: ChromeProcessControl,
    pub(super) group: Option<SharedDriverReservation>,
}

pub(super) fn run_driver(
    config: &BrowserSessionConfig,
    event_tx: &BrowserEventSender,
    command_rx: &CommandReceiver,
    frame_slot: &Arc<FrameSlot>,
    stop_requested: &Arc<AtomicBool>,
    launch: DriverLaunch,
) {
    let DriverLaunch {
        completion_tx,
        process_control,
        group,
    } = launch;
    let completion_guard = DriverCompletion {
        completion_tx: Some(completion_tx),
        process_control,
        group,
    };
    let Some(_coordination_lifetime) = crate::coordination::CoordinationLifetime::start(config) else {
        let _ = event_tx.send(BrowserEvent::Warning(crate::coordination::PREPARE_FAILURE.to_string()));
        let _ = event_tx.send(BrowserEvent::Stopped { code: None });
        return;
    };
    let Some(mut connection) = initialize_driver(
        config,
        event_tx,
        stop_requested,
        &completion_guard.process_control,
        completion_guard.group.as_ref().map(|reservation| &reservation.session),
    ) else {
        return;
    };

    let mut state = DriverState::new(
        config,
        &connection.ws_url,
        connection.native_user_agent_metadata.take(),
        Arc::clone(stop_requested),
    );
    state.initialize_manifest();
    if !state.attach_setup(
        &mut connection.link,
        event_tx,
        frame_slot,
        &connection.session_id,
        &connection.target_id,
    ) {
        connection
            .chrome
            .close_page(&mut connection.link, &connection.target_id);
        let _ = event_tx.send(BrowserEvent::Stopped { code: None });
        return;
    }

    run_loop(
        &mut state,
        &mut connection.chrome,
        &mut connection.link,
        command_rx,
        frame_slot,
        event_tx,
    );
    connection
        .chrome
        .close_page(&mut connection.link, &connection.target_id);
}

/// First existing `page` target, if any.
pub(super) fn first_page_target(link: &mut CdpLink, stop_requested: &AtomicBool) -> Option<String> {
    let result = call_during_startup(link, stop_requested, "Target.getTargets", &serde_json::json!({})).ok()?;
    result
        .get("targetInfos")
        .and_then(|t| t.as_array())
        .and_then(|targets| first_available_page_target_id(targets))
        .map(str::to_string)
}

fn first_available_page_target_id(targets: &[serde_json::Value]) -> Option<&str> {
    targets
        .iter()
        .find(|target| {
            target.get("type").and_then(serde_json::Value::as_str) == Some("page")
                && !target
                    .get("attached")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
        })
        .and_then(|target| target.get("targetId"))
        .and_then(serde_json::Value::as_str)
}

/// Create a fresh page target as a fallback (browser opened without one).
pub(super) fn create_page_target(link: &mut CdpLink, stop_requested: &AtomicBool) -> Option<String> {
    let result = call_during_startup(
        link,
        stop_requested,
        "Target.createTarget",
        &serde_json::json!({ "url": "about:blank" }),
    )
    .ok()?;
    result.get("targetId").and_then(|t| t.as_str()).map(str::to_string)
}

fn build_launch(config: &BrowserSessionConfig) -> Result<crate::process::ChromeLaunch, String> {
    let command = crate::process::resolve_binary_or_default(&config.browser.command)
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())?;
    let profile_dir = profile_dir(&config.browser, &config.panel_local_id);
    Ok(crate::process::ChromeLaunch {
        command,
        profile_dir,
        width: config.width,
        height: config.height,
        headless: config.browser.headless,
        extra_args: config.browser.extra_args.clone(),
        automation_disclosure: config.browser.automation_disclosure,
    })
}

pub(crate) fn profile_dir(config: &BrowserConfig, panel_local_id: &str) -> std::path::PathBuf {
    config.profile_dir(panel_local_id)
}

#[cfg(test)]
mod retry_tests {
    use crate::AutomationDisclosurePolicy;
    use crate::process::ChromeError;

    use super::{ChromeStartupAttempt, DEVTOOLS_PORT_REAP_FAILURE, DEVTOOLS_PORT_STARTUP_ATTEMPTS};

    fn port_conflict() -> ChromeError {
        ChromeError::NoDevtools("Cannot start http server for devtools.".to_string())
    }

    #[test]
    fn minimized_startup_retries_with_fresh_attempts_and_stops_at_the_bound() {
        let mut attempts = 0;
        let connection = super::run_chrome_startup_attempts(AutomationDisclosurePolicy::MinimizeCommonSignals, || {
            attempts += 1;
            if attempts < DEVTOOLS_PORT_STARTUP_ATTEMPTS {
                Ok(ChromeStartupAttempt::Failed {
                    error: port_conflict(),
                    reaped: true,
                })
            } else {
                Ok(ChromeStartupAttempt::Connected(attempts))
            }
        })
        .unwrap_or_else(|error| panic!("retry startup: {error}"));

        assert_eq!(connection, Some(DEVTOOLS_PORT_STARTUP_ATTEMPTS));
        assert_eq!(attempts, DEVTOOLS_PORT_STARTUP_ATTEMPTS);

        let mut bounded_attempts = 0;
        let Err(error) =
            super::run_chrome_startup_attempts::<()>(AutomationDisclosurePolicy::MinimizeCommonSignals, || {
                bounded_attempts += 1;
                Ok(ChromeStartupAttempt::Failed {
                    error: port_conflict(),
                    reaped: true,
                })
            })
        else {
            panic!("conflicting startup unexpectedly succeeded");
        };

        assert!(error.contains("Cannot start http server for devtools"));
        assert_eq!(bounded_attempts, DEVTOOLS_PORT_STARTUP_ATTEMPTS);
    }

    #[test]
    fn minimized_startup_aborts_before_retry_when_reap_fails() {
        let mut attempts = 0;
        let Err(error) =
            super::run_chrome_startup_attempts::<()>(AutomationDisclosurePolicy::MinimizeCommonSignals, || {
                attempts += 1;
                Ok(ChromeStartupAttempt::Failed {
                    error: port_conflict(),
                    reaped: false,
                })
            })
        else {
            panic!("unreaped startup unexpectedly succeeded");
        };

        assert_eq!(error, DEVTOOLS_PORT_REAP_FAILURE);
        assert_eq!(attempts, 1);
    }

    #[test]
    fn browser_default_does_not_retry_devtools_conflicts() {
        let mut attempts = 0;
        let Err(error) = super::run_chrome_startup_attempts::<()>(AutomationDisclosurePolicy::BrowserDefault, || {
            attempts += 1;
            Ok(ChromeStartupAttempt::Failed {
                error: port_conflict(),
                reaped: true,
            })
        }) else {
            panic!("browser-default startup unexpectedly succeeded");
        };

        assert!(error.contains("Cannot start http server for devtools"));
        assert_eq!(attempts, 1);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::BrowserConfig;
    use crate::frames::FrameSlot;

    use super::super::{BrowserSessionConfig, start_session};
    use super::{first_available_page_target_id, profile_dir};

    #[test]
    fn target_selection_skips_attached_pages() {
        let targets = serde_json::json!([
            { "targetId": "attached", "type": "page", "attached": true },
            { "targetId": "worker", "type": "service_worker", "attached": false },
            { "targetId": "available", "type": "page", "attached": false }
        ]);

        assert_eq!(
            first_available_page_target_id(targets.as_array().expect("target list")),
            Some("available")
        );
    }

    #[test]
    fn configured_profile_root_confines_unsafe_panel_ids() {
        let config = BrowserConfig {
            profile_root: Some("/tmp/horizon-browser-profiles".into()),
            ..BrowserConfig::default()
        };

        assert_eq!(
            profile_dir(&config, "../outside/profile"),
            std::path::PathBuf::from("/tmp/horizon-browser-profiles/%2e2e2f6f7574736964652f70726f66696c65/chromium")
        );
        assert_ne!(profile_dir(&config, "a/b"), profile_dir(&config, "a_b"));
        assert_ne!(profile_dir(&config, "Panel-A"), profile_dir(&config, "panel-a"));
        let firefox = BrowserConfig {
            backend: crate::BackendKind::FirefoxBidi,
            ..config.clone()
        };
        assert_ne!(profile_dir(&config, "panel"), profile_dir(&firefox, "panel"));
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
            coordination: None,
            capture_directory: None,
            video: Arc::new(crate::VideoCaptureHandle::default()),
            remote: None,
        })
        .unwrap_or_else(|error| panic!("start browser session: {error}"));

        let deadline = Instant::now() + Duration::from_secs(2);
        while !started.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(started.exists(), "delayed browser did not start");

        let completion = session.shutdown_signal();
        assert!(completion.wait(Duration::from_secs(2)));
    }
}

#[cfg(test)]
mod disclosure_document_tests {
    use std::net::TcpListener;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use serde_json::{Value, json};
    use tungstenite::Message;

    use crate::cdp::CdpLink;
    use crate::disclosure::{CHROMIUM_DISCLOSURE_BOOTSTRAP_URL, CHROMIUM_USER_AGENT_METADATA_EXPRESSION};

    /// A target that shows its initial `about:blank` document for `blank_reads` location reads,
    /// as Chromium does when `Target.createTarget` answers before the navigation commits.
    fn target(blank_reads: usize) -> (CdpLink, std::thread::JoinHandle<Vec<Value>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let address = listener.local_addr().expect("mock address");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept mock");
            let mut socket = tungstenite::accept(stream).expect("mock handshake");
            let mut commands = Vec::new();
            let mut location_reads = 0;
            while let Ok(Message::Text(text)) = socket.read() {
                let command: Value = serde_json::from_str(&text).expect("command JSON");
                let result = match (command["method"].as_str(), command["params"]["expression"].as_str()) {
                    (Some("Target.attachToTarget"), _) => json!({ "sessionId": "bootstrap" }),
                    (Some("Runtime.evaluate"), Some("location.href")) => {
                        location_reads += 1;
                        let href = if location_reads > blank_reads {
                            CHROMIUM_DISCLOSURE_BOOTSTRAP_URL
                        } else {
                            "about:blank"
                        };
                        json!({ "result": { "type": "string", "value": href } })
                    }
                    (Some("Runtime.evaluate"), _) if location_reads > blank_reads => {
                        json!({ "result": { "type": "object", "value": { "platform": "Linux" } } })
                    }
                    // Outside a secure context the metadata expression yields null.
                    (Some("Runtime.evaluate"), _) => {
                        json!({ "result": { "type": "object", "subtype": "null", "value": null } })
                    }
                    _ => json!({}),
                };
                commands.push(command.clone());
                let reply = json!({ "id": command["id"], "result": result });
                socket.send(Message::Text(reply.to_string().into())).expect("respond");
            }
            commands
        });
        let link = CdpLink::connect(&format!("ws://{address}/")).expect("connect mock");
        (link, server)
    }

    fn reads_of(commands: &[Value], expression: &str) -> usize {
        commands
            .iter()
            .filter(|command| command["params"]["expression"] == expression)
            .count()
    }

    #[test]
    fn metadata_is_read_only_after_the_target_shows_the_bootstrap_page() {
        let (mut link, server) = target(2);
        let metadata =
            super::read_disclosure_metadata_from_target(&mut link, &AtomicBool::new(false), "bootstrap-target")
                .unwrap_or_else(|error| panic!("read metadata: {error}"));
        drop(link);
        let commands = server.join().expect("mock server");

        assert_eq!(metadata["result"]["value"]["platform"], "Linux");
        assert_eq!(reads_of(&commands, "location.href"), 3);
        assert_eq!(reads_of(&commands, CHROMIUM_USER_AGENT_METADATA_EXPRESSION), 1);
        assert_eq!(
            commands.last().map(|command| &command["params"]["expression"]),
            Some(&json!(CHROMIUM_USER_AGENT_METADATA_EXPRESSION))
        );
    }

    #[test]
    fn a_silent_target_does_not_extend_the_wait_past_its_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let address = listener.local_addr().expect("mock address");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept mock");
            let mut socket = tungstenite::accept(stream).expect("mock handshake");
            // Read each command and never answer.
            while let Ok(Message::Text(_)) = socket.read() {}
        });
        let mut link = CdpLink::connect(&format!("ws://{address}/")).expect("connect mock");
        let started = std::time::Instant::now();
        let result = super::wait_for_disclosure_document(
            &mut link,
            &AtomicBool::new(false),
            "bootstrap",
            Duration::from_millis(300),
        );
        let elapsed = started.elapsed();
        drop(link);
        server.join().expect("mock server");

        assert!(result.is_err());
        assert!(elapsed < Duration::from_secs(2), "the wait took {elapsed:?}");
    }

    #[test]
    fn a_target_that_never_shows_the_bootstrap_page_fails_without_reading_metadata() {
        let (mut link, server) = target(usize::MAX);
        let Err(error) = super::wait_for_disclosure_document(
            &mut link,
            &AtomicBool::new(false),
            "bootstrap",
            Duration::from_millis(100),
        ) else {
            panic!("the initial document was accepted as the bootstrap page");
        };
        drop(link);
        let commands = server.join().expect("mock server");

        assert!(error.contains("about:blank"), "{error}");
        assert_eq!(reads_of(&commands, CHROMIUM_USER_AGENT_METADATA_EXPRESSION), 0);
    }
}
