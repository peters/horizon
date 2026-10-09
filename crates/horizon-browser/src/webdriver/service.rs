use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::process::{ChromeProcessControl, ServiceProcess, resolve_binary};
use crate::{BackendKind, BrowserConfig};

use super::http::HttpClient;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const STARTUP_POLL: Duration = Duration::from_millis(25);
const STARTUP_ATTEMPTS: usize = 3;
static SAFARI_SESSION_LEASED: AtomicBool = AtomicBool::new(false);

pub(super) struct WebDriverService {
    pub(super) http: HttpClient,
    pub(super) process: ServiceProcess,
    _safari_lease: Option<SafariLease>,
}

struct SafariLease;

impl SafariLease {
    #[cfg(any(target_os = "macos", test))]
    fn acquire() -> Result<Self, String> {
        SAFARI_SESSION_LEASED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| "Safari automation is busy in another Horizon panel".to_string())
    }
}

impl Drop for SafariLease {
    fn drop(&mut self) {
        SAFARI_SESSION_LEASED.store(false, Ordering::Release);
    }
}

impl WebDriverService {
    pub(super) fn delete_session(&self, session_id: &str) {
        let path = format!("/session/{session_id}");
        let _ = self.http.delete(&path);
    }

    pub(super) fn start(
        config: &BrowserConfig,
        control: &ChromeProcessControl,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, String> {
        let (command, base_args, label, safari_lease) = match config.backend {
            BackendKind::FirefoxBidi => (
                resolve_service(config.geckodriver_command.as_deref(), &["geckodriver"])
                    .map_err(|error| format!("Firefox requires geckodriver: {error}"))?,
                Vec::new(),
                "geckodriver",
                None,
            ),
            BackendKind::SafariWebDriver => {
                #[cfg(not(target_os = "macos"))]
                return Err("Safari WebDriver is available only on macOS".to_string());
                #[cfg(target_os = "macos")]
                {
                    let lease = SafariLease::acquire()?;
                    (
                        resolve_service(
                            config.safaridriver_command.as_deref(),
                            &["/usr/bin/safaridriver", "safaridriver"],
                        )
                        .map_err(|error| format!("Safari requires safaridriver: {error}"))?,
                        Vec::new(),
                        "safaridriver",
                        Some(lease),
                    )
                }
            }
            BackendKind::ChromiumCdp => return Err("Chromium does not use WebDriver service startup".to_string()),
        };

        let mut permit_system_access = true;
        for attempt in 1..=STARTUP_ATTEMPTS {
            let webdriver_listener = reserve_loopback_listener()?;
            let address = webdriver_listener
                .local_addr()
                .map_err(|error| format!("failed to read reserved WebDriver port: {error}"))?;
            let bidi_listener = if config.backend == BackendKind::FirefoxBidi {
                Some(reserve_loopback_listener()?)
            } else {
                None
            };
            let bidi_port = bidi_listener
                .as_ref()
                .map(TcpListener::local_addr)
                .transpose()
                .map_err(|error| format!("failed to read reserved Firefox BiDi port: {error}"))?
                .map(|address| address.port());
            let args = if config.backend == BackendKind::FirefoxBidi {
                firefox_service_arguments(
                    config,
                    address.port(),
                    bidi_port.unwrap_or(address.port()),
                    permit_system_access,
                )
            } else {
                service_args(config.backend, address.port(), bidi_port, base_args.clone())
            };
            drop(bidi_listener);
            drop(webdriver_listener);
            let mut process = ServiceProcess::spawn(&command, &args, control.clone(), label)
                .map_err(|error| format!("failed to start {label}: {error}"))?;
            let http = HttpClient::new(address).map_err(|error| error.to_string())?;
            let deadline = Instant::now() + STARTUP_TIMEOUT;
            loop {
                if cancelled() {
                    let _ = process.kill();
                    return Err(format!("{label} startup cancelled"));
                }
                if http.get("/status").is_ok_and(|status| status_is_ready(&status)) {
                    return Ok(Self {
                        http,
                        process,
                        _safari_lease: safari_lease,
                    });
                }
                if let Some(status) = process.child_status() {
                    let stderr = process.stderr_tail();
                    let error = format!("{label} exited before becoming ready ({status}); stderr: {stderr}");
                    if retry_without_system_access(permit_system_access, process.stderr_is_complete(), &stderr) {
                        permit_system_access = false;
                        tracing::warn!("{error}; retrying without --allow-system-access");
                        if attempt == STARTUP_ATTEMPTS {
                            return Err(error);
                        }
                        break;
                    }
                    if attempt == STARTUP_ATTEMPTS {
                        return Err(error);
                    }
                    tracing::warn!(attempt, "{error}; retrying with fresh reserved ports");
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = process.kill();
                    let stderr = process.stderr_tail();
                    return Err(format!("timed out waiting for {label}; stderr: {stderr}"));
                }
                std::thread::sleep(STARTUP_POLL);
            }
        }
        Err(format!("{label} exhausted startup attempts"))
    }
}

fn status_is_ready(status: &serde_json::Value) -> bool {
    status.pointer("/value/ready").and_then(serde_json::Value::as_bool) == Some(true)
}

pub(super) fn firefox_service_arguments(
    config: &BrowserConfig,
    port: u16,
    bidi_port: u16,
    permit_system_access: bool,
) -> Vec<String> {
    let mut extra = Vec::new();
    // geckodriver 0.37 rejects this privilege inside `moz:firefoxOptions`.
    // Older drivers reject the process flag, so startup retries without it.
    if permit_system_access && config.automation_disclosure == crate::AutomationDisclosurePolicy::MinimizeCommonSignals
    {
        extra.push("--allow-system-access".to_string());
    }
    service_args(BackendKind::FirefoxBidi, port, Some(bidi_port), extra)
}

/// Drop `--allow-system-access` when geckodriver rejected it, or when the
/// stderr reader did not finish. An unfinished tail must not count as acceptance.
#[must_use]
pub(super) fn retry_without_system_access(permit_system_access: bool, stderr_complete: bool, stderr: &str) -> bool {
    permit_system_access && (!stderr_complete || geckodriver_rejected_system_access(stderr))
}

pub(super) fn geckodriver_rejected_system_access(stderr: &str) -> bool {
    // clap 4 says "unexpected argument". clap 2 and 3 say
    // "Found argument '--allow-system-access' which wasn't expected".
    let text = stderr.to_ascii_lowercase().replace(['\u{2019}', '\u{2018}'], "'");
    text.contains("allow-system-access")
        && (text.contains("unexpected")
            || text.contains("unrecognized")
            || text.contains("unknown")
            || text.contains("wasn't expected")
            || text.contains("was not expected")
            || text.contains("not valid in this context"))
}

fn service_args(backend: BackendKind, port: u16, bidi_port: Option<u16>, mut extra: Vec<String>) -> Vec<String> {
    match backend {
        BackendKind::FirefoxBidi => {
            extra.extend([
                "--host".to_string(),
                Ipv4Addr::LOCALHOST.to_string(),
                "--port".to_string(),
                port.to_string(),
                "--websocket-port".to_string(),
                bidi_port.unwrap_or(port).to_string(),
            ]);
            extra
        }
        BackendKind::SafariWebDriver => {
            extra.extend(["--port".to_string(), port.to_string()]);
            extra
        }
        BackendKind::ChromiumCdp => extra,
    }
}

fn reserve_loopback_listener() -> Result<TcpListener, String> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("failed to reserve browser automation port: {error}"))
}

fn resolve_service(explicit: Option<&str>, candidates: &[&str]) -> Result<PathBuf, String> {
    if let Some(command) = explicit {
        return resolve_binary(command).map_err(|error| error.to_string());
    }
    for candidate in candidates {
        if let Ok(path) = resolve_binary(candidate) {
            return Ok(path);
        }
    }
    Err(format!("none of {} were found", candidates.join(", ")))
}

pub(super) fn prepare_profile(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|error| format!("failed to create browser profile: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("failed to protect browser profile: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SafariLease, geckodriver_rejected_system_access, service_args, status_is_ready};
    use crate::BackendKind;

    #[test]
    fn driver_service_arguments_bind_exact_loopback_port() {
        assert_eq!(
            service_args(BackendKind::FirefoxBidi, 4444, Some(4666), Vec::new()),
            ["--host", "127.0.0.1", "--port", "4444", "--websocket-port", "4666"]
        );
        assert_eq!(
            service_args(BackendKind::SafariWebDriver, 5555, None, Vec::new()),
            ["--port", "5555"]
        );
    }

    #[test]
    fn older_geckodriver_can_start_without_system_access() {
        assert!(geckodriver_rejected_system_access(
            "geckodriver: error: unexpected argument '--allow-system-access' found"
        ));
        assert!(geckodriver_rejected_system_access(
            "error: Found argument '--allow-system-access' which wasn't expected, or isn't valid in this context"
        ));
        assert!(geckodriver_rejected_system_access(
            "error: Found argument '--allow-system-access' which wasn’t expected, or isn’t valid in this context"
        ));
        assert!(!geckodriver_rejected_system_access("address already in use"));
        assert!(!geckodriver_rejected_system_access(
            "unexpected error while starting firefox"
        ));
        assert!(super::retry_without_system_access(true, false, ""));
        assert!(super::retry_without_system_access(
            true,
            true,
            "geckodriver: error: unexpected argument '--allow-system-access' found"
        ));
        assert!(!super::retry_without_system_access(
            true,
            true,
            "address already in use"
        ));
        assert!(!super::retry_without_system_access(false, false, ""));
        let config = crate::BrowserConfig {
            backend: BackendKind::FirefoxBidi,
            ..crate::BrowserConfig::default()
        };
        assert!(
            super::firefox_service_arguments(&config, 9, 10, true)
                .iter()
                .any(|argument| argument == "--allow-system-access")
        );
        assert!(
            !super::firefox_service_arguments(&config, 9, 10, false)
                .iter()
                .any(|argument| argument == "--allow-system-access")
        );
    }

    #[test]
    fn safari_lease_allows_only_one_session() {
        let first = SafariLease::acquire().expect("first lease");
        assert!(SafariLease::acquire().is_err());
        drop(first);
        assert!(SafariLease::acquire().is_ok());
    }

    #[test]
    fn driver_status_requires_explicit_readiness() {
        assert!(status_is_ready(&serde_json::json!({ "value": { "ready": true } })));
        assert!(!status_is_ready(&serde_json::json!({ "value": { "ready": false } })));
        assert!(!status_is_ready(&serde_json::json!({ "value": {} })));
    }
}
