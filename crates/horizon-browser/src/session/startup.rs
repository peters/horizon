//! Chrome process startup and initial CDP target attachment.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crate::cdp::CdpLink;
use crate::disclosure::{
    CHROMIUM_DISCLOSURE_BOOTSTRAP_URL, CHROMIUM_USER_AGENT_METADATA_EXPRESSION, chromium_user_agent_needs_override,
};
use crate::frames::FrameSlot;
use crate::process::{ChromeProcess, ChromeProcessControl};
use crate::{AutomationDisclosurePolicy, BrowserConfig};

use super::{
    BrowserEvent, BrowserEventSender, BrowserSessionConfig, CALL_TIMEOUT, CommandReceiver, DriverState, WS_URL_TIMEOUT,
    run_loop,
};

const DEVTOOLS_PORT_STARTUP_ATTEMPTS: usize = 3;

/// Settles process registration and resolves the driver's teardown signal
/// exactly once, on thread exit.
struct DriverCompletion {
    completion_tx: Option<mpsc::Sender<()>>,
    process_control: ChromeProcessControl,
}

impl Drop for DriverCompletion {
    fn drop(&mut self) {
        self.process_control.mark_registration_settled();
        if let Some(tx) = self.completion_tx.take() {
            let _ = tx.send(());
        }
    }
}

fn cancel_startup_if_requested(
    requested: &AtomicBool,
    chrome: &mut ChromeProcess,
    event_tx: &BrowserEventSender,
) -> bool {
    if !requested.load(Ordering::Acquire) {
        return false;
    }
    let _ = chrome.kill();
    let _ = event_tx.send(BrowserEvent::Stopped { code: None });
    true
}

struct DriverConnection {
    chrome: ChromeProcess,
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
    let (mut chrome, ws_url) = match start_chrome(&launch, stop_requested, process_control) {
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

fn start_chrome(
    launch: &crate::process::ChromeLaunch,
    stop_requested: &AtomicBool,
    process_control: &ChromeProcessControl,
) -> Result<Option<(ChromeProcess, String)>, String> {
    for attempt in 1..=DEVTOOLS_PORT_STARTUP_ATTEMPTS {
        if stop_requested.load(Ordering::Acquire) {
            return Ok(None);
        }
        let mut chrome = ChromeProcess::spawn(launch, process_control.clone())
            .map_err(|error| format!("failed to start chrome: {error}"))?;
        match chrome.wait_ws_url(WS_URL_TIMEOUT, || stop_requested.load(Ordering::Acquire)) {
            Ok(Some(url)) => return Ok(Some((chrome, url))),
            Ok(None) => {
                let _ = chrome.kill();
                return Ok(None);
            }
            Err(error) if error.is_devtools_port_conflict() && attempt < DEVTOOLS_PORT_STARTUP_ATTEMPTS => {
                tracing::warn!(
                    attempt,
                    max_attempts = DEVTOOLS_PORT_STARTUP_ATTEMPTS,
                    "Chromium DevTools port handoff collided; retrying with a fresh reservation"
                );
                let _ = chrome.kill();
            }
            Err(error) => {
                let _ = chrome.kill();
                return Err(format!("no DevTools endpoint: {error}"));
            }
        }
    }
    Err("Chromium exhausted its bounded DevTools-port startup attempts".to_string())
}

fn initialize_target(
    config: &BrowserSessionConfig,
    event_tx: &BrowserEventSender,
    stop_requested: &AtomicBool,
    mut chrome: ChromeProcess,
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
    let existing_target = first_page_target(&mut link, stop_requested);
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
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    let native_user_agent_metadata =
        match prepare_disclosure_metadata(&mut link, stop_requested, config.browser.automation_disclosure) {
            Ok(metadata) => metadata,
            Err(error) => {
                let _ = event_tx.send(BrowserEvent::Warning(format!(
                    "browser disclosure bootstrap failed: {error}"
                )));
                let _ = chrome.kill();
                let _ = event_tx.send(BrowserEvent::Stopped { code: None });
                return None;
            }
        };
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    let _ = call_during_startup(
        &mut link,
        stop_requested,
        "Target.setAutoAttach",
        &serde_json::json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
    );
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    // setAutoAttach only covers *future* targets: attach explicitly to the
    // page that Chrome opened at startup (creating one if it has none).
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
        let _ = chrome.kill();
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
        native_user_agent_metadata,
    })
}

fn prepare_disclosure_metadata(
    link: &mut CdpLink,
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

    let metadata = read_disclosure_metadata_from_target(link, stop_requested, &target_id);
    let close_result = call_during_startup(
        link,
        stop_requested,
        "Target.closeTarget",
        &serde_json::json!({ "targetId": target_id }),
    )
    .map_err(|error| format!("Target.closeTarget: {error}"))
    .and_then(|result| {
        result
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .is_some_and(|success| success)
            .then_some(())
            .ok_or_else(|| "Target.closeTarget did not close the disclosure target".to_string())
    });
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
    for (method, params) in [
        ("Runtime.enable", serde_json::json!({})),
        (
            "Runtime.evaluate",
            serde_json::json!({
                "expression": CHROMIUM_USER_AGENT_METADATA_EXPRESSION,
                "awaitPromise": true,
                "returnByValue": true,
            }),
        ),
    ] {
        let result = link
            .call_and_drain_until(CALL_TIMEOUT, method, &params, Some(session_id), || {
                stop_requested.load(Ordering::Acquire)
            })
            .result
            .map_err(|error| format!("{method}: {error}"))?;
        if method == "Runtime.evaluate" {
            return Ok(result);
        }
    }
    Err("Runtime.evaluate did not return native user-agent metadata".to_string())
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

pub(super) fn run_driver(
    config: &BrowserSessionConfig,
    event_tx: &BrowserEventSender,
    command_rx: &CommandReceiver,
    frame_slot: &Arc<FrameSlot>,
    stop_requested: &Arc<AtomicBool>,
    completion_tx: mpsc::Sender<()>,
    process_control: ChromeProcessControl,
) {
    let completion_guard = DriverCompletion {
        completion_tx: Some(completion_tx),
        process_control,
    };
    let Some(_coordination_lifetime) = crate::coordination::CoordinationLifetime::start(config) else {
        let _ = event_tx.send(BrowserEvent::Warning(crate::coordination::PREPARE_FAILURE.to_string()));
        let _ = event_tx.send(BrowserEvent::Stopped { code: None });
        return;
    };
    let Some(mut connection) = initialize_driver(config, event_tx, stop_requested, &completion_guard.process_control)
    else {
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
        let _ = connection.chrome.kill();
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
    let _ = connection.chrome.kill();
}

/// First existing `page` target, if any.
fn first_page_target(link: &mut CdpLink, stop_requested: &AtomicBool) -> Option<String> {
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
fn create_page_target(link: &mut CdpLink, stop_requested: &AtomicBool) -> Option<String> {
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
