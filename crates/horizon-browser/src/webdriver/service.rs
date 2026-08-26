use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::process::{ChromeProcessControl, ServiceProcess, resolve_binary};
use crate::{BackendKind, BrowserConfig};

use super::http::HttpClient;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const STARTUP_POLL: Duration = Duration::from_millis(25);
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
    pub(super) fn start(
        config: &BrowserConfig,
        control: ChromeProcessControl,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, String> {
        let (command, args, label, safari_lease) = match config.backend {
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
        let mut args = service_args(config.backend, address.port(), bidi_port, args);
        drop(bidi_listener);
        drop(webdriver_listener);
        let mut process = ServiceProcess::spawn(&command, &args, control, label)
            .map_err(|error| format!("failed to start {label}: {error}"))?;
        args.clear();
        let http = HttpClient::new(address).map_err(|error| error.to_string())?;
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if cancelled() {
                let _ = process.kill();
                return Err(format!("{label} startup cancelled"));
            }
            if http.get("/status").is_ok_and(|status| status_is_ready(&status)) {
                break;
            }
            if let Some(status) = process.child_status() {
                return Err(format!(
                    "{label} exited before becoming ready ({status}); stderr: {}",
                    process.stderr_tail()
                ));
            }
            if Instant::now() >= deadline {
                let stderr = process.stderr_tail();
                let _ = process.kill();
                return Err(format!("timed out waiting for {label}; stderr: {stderr}"));
            }
            std::thread::sleep(STARTUP_POLL);
        }
        Ok(Self {
            http,
            process,
            _safari_lease: safari_lease,
        })
    }
}

fn status_is_ready(status: &serde_json::Value) -> bool {
    status.pointer("/value/ready").and_then(serde_json::Value::as_bool) == Some(true)
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
    use super::{SafariLease, service_args, status_is_ready};
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
