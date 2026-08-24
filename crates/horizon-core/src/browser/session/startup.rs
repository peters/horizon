//! Chrome process startup and initial CDP target attachment.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crate::browser::BrowserConfig;
use crate::browser::cdp::CdpLink;
use crate::browser::frames::FrameSlot;
use crate::browser::manifest::DriverManifestLifetime;
use crate::browser::process::ChromeProcess;

use super::{
    BrowserCommand, BrowserEvent, BrowserEventSender, BrowserSessionConfig, CALL_TIMEOUT, DriverState, WS_URL_TIMEOUT,
    run_loop,
};

/// Resolves the driver's teardown signal exactly once, on thread exit.
struct DriverCompletion(Option<mpsc::Sender<()>>);

impl Drop for DriverCompletion {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
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
    chrome.kill();
    let _ = event_tx.send(BrowserEvent::Stopped { code: None });
    true
}

struct DriverConnection {
    chrome: ChromeProcess,
    link: CdpLink,
    ws_url: String,
    target_id: String,
    session_id: String,
}

fn initialize_driver(
    config: &BrowserSessionConfig,
    event_tx: &BrowserEventSender,
    stop_requested: &AtomicBool,
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
    let mut chrome = match ChromeProcess::spawn(&launch) {
        Ok(chrome) => chrome,
        Err(error) => {
            let _ = event_tx.send(BrowserEvent::Warning(format!("failed to start chrome: {error}")));
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return None;
        }
    };
    let ws_url = match chrome.wait_ws_url(WS_URL_TIMEOUT, || stop_requested.load(Ordering::Acquire)) {
        Ok(Some(url)) => url,
        Ok(None) => {
            chrome.kill();
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return None;
        }
        Err(error) => {
            let _ = event_tx.send(BrowserEvent::Warning(format!("no DevTools endpoint: {error}")));
            chrome.kill();
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
            chrome.kill();
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return None;
        }
    };
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    initialize_target(config, event_tx, stop_requested, chrome, link, ws_url)
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
    let existing_target = first_page_target(&mut link, stop_requested);
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    let Some(target_id) = existing_target.or_else(|| create_page_target(&mut link, config, stop_requested)) else {
        if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
            return None;
        }
        let _ = event_tx.send(BrowserEvent::Warning("no page target found".to_string()));
        chrome.kill();
        let _ = event_tx.send(BrowserEvent::Stopped { code: None });
        return None;
    };
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
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
        chrome.kill();
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
    })
}

fn call_during_startup(
    link: &mut CdpLink,
    stop_requested: &AtomicBool,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, crate::browser::cdp::CdpError> {
    link.call_and_drain_until(CALL_TIMEOUT, method, params, None, || {
        stop_requested.load(Ordering::Acquire)
    })
    .result
}

pub(super) fn run_driver(
    config: &BrowserSessionConfig,
    event_tx: &BrowserEventSender,
    command_rx: &mpsc::Receiver<BrowserCommand>,
    frame_slot: &Arc<FrameSlot>,
    stop_requested: &Arc<AtomicBool>,
    completion_tx: mpsc::Sender<()>,
) {
    let _completion = DriverCompletion(Some(completion_tx));
    let _manifest_lifetime = DriverManifestLifetime::start(&config.panel_local_id);
    let Some(mut connection) = initialize_driver(config, event_tx, stop_requested) else {
        return;
    };

    let mut state = DriverState::new(config, &connection.ws_url, Arc::clone(stop_requested));
    state.initialize_manifest();
    if !state.attach_setup(
        &mut connection.link,
        event_tx,
        frame_slot,
        &connection.session_id,
        &connection.target_id,
    ) {
        connection.chrome.kill();
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
    connection.chrome.kill();
}

/// First existing `page` target, if any.
fn first_page_target(link: &mut CdpLink, stop_requested: &AtomicBool) -> Option<String> {
    let result = call_during_startup(link, stop_requested, "Target.getTargets", &serde_json::json!({})).ok()?;
    result
        .get("targetInfos")
        .and_then(|t| t.as_array())
        .and_then(|targets| {
            targets
                .iter()
                .find(|t| t.get("type").and_then(serde_json::Value::as_str) == Some("page"))
                .filter(|t| !t.get("attached").and_then(serde_json::Value::as_bool).unwrap_or(false))
        })
        .and_then(|t| t.get("targetId"))
        .and_then(|t| t.as_str())
        .map(str::to_string)
}

/// Create a fresh page target as a fallback (browser opened without one).
fn create_page_target(
    link: &mut CdpLink,
    config: &BrowserSessionConfig,
    stop_requested: &AtomicBool,
) -> Option<String> {
    let url = config
        .initial_url
        .clone()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "about:blank".to_string());
    let result = call_during_startup(
        link,
        stop_requested,
        "Target.createTarget",
        &serde_json::json!({ "url": url }),
    )
    .ok()?;
    result.get("targetId").and_then(|t| t.as_str()).map(str::to_string)
}

fn build_launch(config: &BrowserSessionConfig) -> Result<crate::browser::process::ChromeLaunch, String> {
    let command = crate::browser::process::resolve_binary_or_default(&config.browser.command)
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())?;
    let profile_dir = profile_dir(&config.browser, &config.panel_local_id);
    Ok(crate::browser::process::ChromeLaunch {
        command,
        profile_dir,
        width: config.width,
        height: config.height,
        extra_args: config.browser.extra_args.clone(),
    })
}

pub(in crate::browser) fn profile_dir(config: &BrowserConfig, panel_local_id: &str) -> std::path::PathBuf {
    let home = crate::horizon_home::HorizonHome::resolve();
    // The configured value is a *root*: every panel still gets its own
    // profile directory, or concurrent panels would fight over Chrome's
    // profile lock.
    match &config.profile_root {
        Some(root) => crate::config::Config::expand_tilde(&root.to_string_lossy())
            .join(crate::horizon_home::safe_local_id(panel_local_id)),
        None => home.browser_profile_dir(panel_local_id),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::browser::BrowserConfig;
    use crate::browser::frames::FrameSlot;

    use super::super::{BrowserSessionConfig, start_session};
    use super::profile_dir;

    #[test]
    fn configured_profile_root_confines_unsafe_panel_ids() {
        let config = BrowserConfig {
            profile_root: Some("/tmp/horizon-browser-profiles".into()),
            ..BrowserConfig::default()
        };

        assert_eq!(
            profile_dir(&config, "../outside/profile"),
            std::path::PathBuf::from("/tmp/horizon-browser-profiles/%2e2e2f6f7574736964652f70726f66696c65")
        );
        assert_ne!(profile_dir(&config, "a/b"), profile_dir(&config, "a_b"));
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
        })
        .unwrap_or_else(|error| panic!("start browser session: {error}"));

        let deadline = Instant::now() + Duration::from_secs(2);
        while !started.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(started.exists(), "delayed browser did not start");

        let completion = session.shutdown_signal();
        assert_eq!(completion.recv_timeout(Duration::from_secs(2)), Ok(()));
    }
}
