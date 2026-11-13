//! Chrome process startup and initial CDP target attachment.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use crate::cdp::CdpLink;
use crate::frames::FrameSlot;
use crate::process::{ChromeError, ChromeProcess, ChromeProcessControl};
use crate::{AutomationDisclosurePolicy, BrowserConfig};

use super::shared::{AttachedBrowser, DriverProcess, SharedBrowserSession, SharedDriverReservation};

mod disclosure;
mod page;

use super::{
    BrowserEvent, BrowserEventSender, BrowserSessionConfig, CALL_TIMEOUT, CommandReceiver, DriverState, WS_URL_TIMEOUT,
    run_loop,
};
pub(crate) use page::{caller_page, create_page_target, first_page_target};

const DEVTOOLS_PORT_STARTUP_ATTEMPTS: usize = 3;
/// Poll interval while the disclosure target loads its bootstrap page.
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
    /// Whether `target_id` is the page the panel drove before a lost link, so
    /// its state is intact and no startup navigation may run.
    resumed_page: bool,
    native_user_agent_metadata: Option<serde_json::Value>,
    /// Whether this browser accepted the panel download folder and reports
    /// the downloads it saves there.
    downloads_reporting: bool,
    /// Why this session reports no downloads, when it knows. The downloads
    /// answer states it; it is no reason to stop the page.
    downloads_note: Option<String>,
}

fn initialize_driver(
    config: &BrowserSessionConfig,
    event_tx: &BrowserEventSender,
    stop_requested: &AtomicBool,
    process_control: &ChromeProcessControl,
    group: Option<&SharedBrowserSession>,
) -> Option<DriverConnection> {
    if let Some(endpoint) = &config.attach {
        return initialize_attached(config, endpoint, event_tx, stop_requested);
    }
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
    let link = match CdpLink::connect_with_timeout(&ws_url, super::DRIVER_READ_TIMEOUT) {
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
    // A panel that shares a browser profile shares its download folder too:
    // the folder is one browser-wide setting, and the profile is the unit
    // that already names the shared identity.
    let downloads_dir = config
        .browser
        .downloads_dir(group.map_or(config.panel_local_id.as_str(), SharedBrowserSession::profile_id));
    initialize_target(
        config,
        event_tx,
        stop_requested,
        chrome,
        link,
        ws_url,
        Some(&downloads_dir),
    )
}

/// Connect to the browser that another process started, without a launch, a
/// profile folder or a download folder: this session holds nothing of that
/// browser except the page target it creates.
fn initialize_attached(
    config: &BrowserSessionConfig,
    endpoint: &crate::DevtoolsEndpoint,
    event_tx: &BrowserEventSender,
    stop_requested: &AtomicBool,
) -> Option<DriverConnection> {
    let ws_url = endpoint.ws_url().to_string();
    let mut chrome = DriverProcess::Attached(AttachedBrowser::new());
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    let link = match CdpLink::connect_with_timeout(&ws_url, super::DRIVER_READ_TIMEOUT) {
        Ok(link) => link,
        Err(error) => {
            let _ = event_tx.send(BrowserEvent::Warning(format!("CDP connect failed: {error}")));
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            return None;
        }
    };
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    initialize_target(config, event_tx, stop_requested, chrome, link, ws_url, None)
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
    downloads_dir: Option<&std::path::Path>,
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
    // A browser of another process already holds the pages of its person: this
    // session takes a page of a sibling only when it created one before, and
    // otherwise creates its own.
    let (resumed_page, existing_target) = caller_page(config, &chrome, &mut link, stop_requested);
    if cancel_startup_if_requested(stop_requested, &mut chrome, event_tx) {
        return None;
    }
    let background = config.browser.background_windows && !config.browser.headless && !chrome.is_attached();
    let Some(target_id) = existing_target.or_else(|| create_page_target(&mut link, stop_requested, background)) else {
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
    let native_user_agent_metadata = match disclosure::prepare_disclosure_metadata(
        &mut link,
        &chrome,
        stop_requested,
        config.browser.automation_disclosure,
    ) {
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
    let (downloads_reporting, downloads_note) = if let Some(directory) = downloads_dir {
        (
            configure_downloads(&mut link, event_tx, stop_requested, directory),
            None,
        )
    } else {
        // A refusal or an inapplicable setting is the answer to the downloads
        // question of this panel, not the failure of the session: the page of
        // that browser is fine, and a warning here used to replace a ready
        // page with a fatal error.
        (false, Some(crate::ATTACHED_DOWNLOADS_GAP.to_string()))
    };
    Some(DriverConnection {
        chrome,
        link,
        ws_url,
        target_id,
        session_id,
        resumed_page,
        native_user_agent_metadata,
        downloads_reporting,
        downloads_note,
    })
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

/// Save this panel's downloads into its own folder, and report each one the
/// browser saves there.
///
/// The same browser-wide setting is what lifts Chromium's limit of one
/// page-initiated download per document: with the default behavior Chromium
/// drops every later download and reports nothing, which looked like a page
/// that saved a file when it did not. A browser that refuses the setting keeps
/// the previous behavior, so the panel states that it reports no downloads
/// instead of claiming an empty folder means no download happened.
fn configure_downloads(
    link: &mut CdpLink,
    event_tx: &BrowserEventSender,
    stop_requested: &AtomicBool,
    directory: &std::path::Path,
) -> bool {
    if let Err(error) = crate::downloads::prepare_directory(directory) {
        tracing::warn!(target: "browser", "failed to prepare the download folder: {error}");
    }
    let params = serde_json::json!({
        "behavior": "allow",
        "downloadPath": directory.to_string_lossy(),
        "eventsEnabled": true,
    });
    match call_during_startup(link, stop_requested, "Browser.setDownloadBehavior", &params) {
        Ok(_) => true,
        Err(error) => {
            tracing::warn!(target: "browser", "browser refused the download folder: {error}");
            let _ = event_tx.send(BrowserEvent::Warning(format!(
                "this browser saves downloads to its own folder and reports none: {error}"
            )));
            false
        }
    }
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
    let downloads_directory = config.browser.downloads_dir(
        completion_guard
            .group
            .as_ref()
            .map_or(config.panel_local_id.as_str(), |reservation| {
                SharedBrowserSession::profile_id(&reservation.session)
            }),
    );
    let downloads = crate::downloads::DownloadState::new(
        downloads_directory.to_string_lossy().to_string(),
        connection.downloads_reporting,
    );
    let downloads = match connection.downloads_note.take() {
        Some(note) => downloads.with_note(note),
        None => downloads,
    };
    state.set_downloads(downloads);
    state.initialize_manifest();
    if !state.attach_setup(
        &mut connection.link,
        event_tx,
        frame_slot,
        &connection.session_id,
        &connection.target_id,
        connection.resumed_page,
    ) {
        connection
            .chrome
            .close_page(&mut connection.link, &connection.target_id);
        let _ = event_tx.send(BrowserEvent::Stopped { code: None });
        return;
    }

    if connection.resumed_page {
        state.audit_session_event(crate::BrowserAuditAction::PageResumed);
    }
    let _ = event_tx.send(BrowserEvent::PageAttached {
        target_id: connection.target_id.clone(),
        resumed: connection.resumed_page,
    });
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

/// The Chromium switch of the egress record, when the browser profile names one.
///
/// # Errors
/// The session states an egress and also carries an argument that selects or
/// bypasses a proxy, so one of the two would win in silence.
fn egress_argument(config: &BrowserConfig) -> Option<String> {
    config.egress.as_ref().map(crate::Egress::chromium_argument)
}

fn build_launch(config: &BrowserSessionConfig) -> Result<crate::process::ChromeLaunch, String> {
    // The section follows the server, so its command can be the Chromium of
    // another computer: this computer then starts the browser that it finds.
    let command = crate::here::program("browser.command", config.browser.command.as_deref());
    let command = crate::process::resolve_binary_or_default(&command)
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())?;
    let profile_dir = profile_dir(&config.browser, &config.panel_local_id);
    Ok(crate::process::ChromeLaunch {
        command,
        profile_dir,
        width: config.width,
        height: config.height,
        headless: config.browser.headless,
        extra_args: egress_argument(&config.browser)
            .into_iter()
            .chain(config.browser.extra_args.iter().cloned())
            .collect(),
        automation_disclosure: config.browser.automation_disclosure,
        background_windows: config.browser.background_windows,
    })
}

pub(crate) fn profile_dir(config: &BrowserConfig, panel_local_id: &str) -> std::path::PathBuf {
    config.profile_dir(panel_local_id)
}

#[cfg(test)]
mod egress_tests {
    use super::egress_argument;

    #[test]
    fn a_session_without_an_egress_adds_no_switch() {
        let config = crate::BrowserConfig::default();
        assert_eq!(egress_argument(&config), None);
    }

    #[test]
    fn a_session_with_an_egress_starts_behind_its_proxy() {
        let config = crate::BrowserConfig {
            egress: Some(crate::Egress::parse("socks5://100.64.0.10:1080").unwrap()),
            ..crate::BrowserConfig::default()
        };
        assert_eq!(
            egress_argument(&config).as_deref(),
            Some("--proxy-server=socks5://100.64.0.10:1080")
        );
    }
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
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::BrowserConfig;
    use crate::frames::FrameSlot;

    use super::super::{BrowserSessionConfig, start_session};
    use super::page::first_available_page_target_id;
    use super::{build_launch, profile_dir};

    /// Write `body` as the program `name` of `folder`, with the permissions of a
    /// program, so the session under test starts a program that this test wrote.
    ///
    /// A `/bin/sh` child writes the file. A write from this process keeps a descriptor
    /// of the program open for a few microseconds, and a fork of another test thread in
    /// that window copies it: the child then holds the program open for writing until
    /// its own `exec`, and the kernel refuses the start with `ETXTBSY` (`Text file
    /// busy`). The descriptor of a child belongs to that child.
    ///
    /// This crate has no dependency on `horizon-core`, so the helper of the tests of
    /// the workspace is not reachable from here.
    fn stand_in_program(folder: &Path, name: &str, body: &str) -> PathBuf {
        use std::io::Write as _;
        use std::process::{Command, Stdio};

        let path = folder.join(name);
        let mut child = Command::new("/bin/sh")
            .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(&path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("start the shell that writes {name}: {error}"));
        let mut stdin = child
            .stdin
            .take()
            .unwrap_or_else(|| panic!("the shell that writes {name} holds its input"));
        stdin
            .write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
            .unwrap_or_else(|error| panic!("write {name}: {error}"));
        drop(stdin);
        let status = child
            .wait()
            .unwrap_or_else(|error| panic!("wait for the shell that writes {name}: {error}"));
        assert!(status.success(), "the shell wrote {name}: {status}");
        path
    }

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
        let profiles = tempfile::tempdir().expect("a folder of this computer");
        let config = BrowserConfig {
            profile_root: Some(profiles.path().to_path_buf()),
            ..BrowserConfig::default()
        };

        assert_eq!(
            profile_dir(&config, "../outside/profile"),
            profiles
                .path()
                .join("%2e2e2f6f7574736964652f70726f66696c65")
                .join("chromium")
        );
        assert_ne!(profile_dir(&config, "a/b"), profile_dir(&config, "a_b"));
        assert_ne!(profile_dir(&config, "Panel-A"), profile_dir(&config, "panel-a"));
        let firefox = BrowserConfig {
            backend: crate::BackendKind::FirefoxBidi,
            ..config.clone()
        };
        assert_ne!(profile_dir(&config, "panel"), profile_dir(&firefox, "panel"));
    }

    /// A shared section can name the Chromium of another computer. A program that
    /// this computer does not have is read as unset, so the panel starts the
    /// browser that this computer would start with no value at all.
    #[test]
    fn a_chromium_of_another_computer_starts_the_browser_of_this_one() {
        let temp = tempfile::tempdir().expect("a folder of this computer");
        let profiles = temp.path().join("profiles");
        std::fs::create_dir_all(&profiles).expect("a profile folder");
        let session = |command: Option<String>| BrowserSessionConfig {
            attach: None,
            resume_target: None,
            browser: BrowserConfig {
                command,
                profile_root: Some(profiles.clone()),
                ..BrowserConfig::default()
            },
            panel_local_id: "fallback".to_string(),
            initial_url: None,
            width: 320,
            height: 200,
            frame_slot: std::sync::Arc::new(FrameSlot::new()),
            coordination: None,
            capture_directory: None,
            video: std::sync::Arc::new(crate::VideoCaptureHandle::default()),
            remote: None,
        };
        let of_another = build_launch(&session(Some(
            "/nonexistent/a-home-of-another-computer/chromium".to_string(),
        )))
        .map(|launch| launch.command);
        let of_this_one = build_launch(&session(None)).map(|launch| launch.command);
        assert_eq!(
            of_another, of_this_one,
            "a Chromium of another computer is read as unset"
        );
    }

    #[test]
    fn shutdown_cancels_devtools_endpoint_wait() {
        // This test needs a launch that does not finish. Its browser is a program
        // of the test that starts and never answers, not a browser of this
        // computer.
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("create temp dir: {error}"));
        let started = temp.path().join("started");
        let browser = stand_in_program(
            temp.path(),
            "delayed-browser",
            &format!("printf started > '{}'\nexec sleep 30", started.display()),
        );

        let session = start_session(BrowserSessionConfig {
            attach: None,
            resume_target: None,
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
mod attach_tests;

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
        let metadata = super::disclosure::read_disclosure_metadata_from_target(
            &mut link,
            &AtomicBool::new(false),
            "bootstrap-target",
        )
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
        let result = super::disclosure::wait_for_disclosure_document(
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
        let Err(error) = super::disclosure::wait_for_disclosure_document(
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
