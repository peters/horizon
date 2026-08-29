use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use horizon_browser::{
    BackendKind, BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, BrowserAuditStatus, BrowserConfig,
    BrowserCoordination, BrowserEvent, BrowserSession, BrowserSessionConfig, FrameSlot, new_action_id, start_session,
};
use horizon_core::{
    HorizonHome,
    browser::manifest::{self, ManifestCoordination},
};
use thiserror::Error;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const FORCED_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const HOST_EXIT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StandaloneOptions {
    pub backend: Option<BackendKind>,
    pub visible: bool,
}

#[derive(Debug, Error)]
pub enum StandaloneError {
    #[error("could not start standalone browser: {0}")]
    Start(#[from] horizon_browser::BrowserError),
    #[error("standalone browser startup failed: {0}")]
    Startup(String),
    #[error(transparent)]
    Mcp(#[from] horizon_browser_mcp::StdioServerError),
    #[error("standalone browser did not stop within the cleanup deadline")]
    Shutdown,
}

pub(crate) struct OwnedSession {
    session: Option<BrowserSession>,
    profile_root: PathBuf,
}

impl OwnedSession {
    pub(crate) fn start(options: StandaloneOptions, home: &HorizonHome) -> Result<Self, StandaloneError> {
        let (session, profile_root) = start(options, home)?;
        Ok(Self {
            session: Some(session),
            profile_root,
        })
    }

    pub(crate) fn shutdown(mut self) -> bool {
        self.stop()
    }

    fn stop(&mut self) -> bool {
        self.session.take().is_none_or(|session| {
            let profile_root = std::mem::take(&mut self.profile_root);
            let shutdown = session.shutdown_signal().with_profile_cleanup(profile_root);
            shutdown.wait(SHUTDOWN_TIMEOUT) || shutdown.force_cleanup(FORCED_SHUTDOWN_TIMEOUT)
        })
    }
}

impl Drop for OwnedSession {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[doc(hidden)]
pub struct OwnedHostProcess {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl OwnedHostProcess {
    /// Start an owned standalone MCP subprocess with private host state.
    ///
    /// # Errors
    /// Returns when the subprocess cannot start, initialize, or publish its
    /// first browser manifest before the startup deadline.
    pub fn start(options: StandaloneOptions, home: &Path, stderr: File) -> Result<Self, StandaloneError> {
        let executable = std::env::current_exe()
            .map_err(|error| StandaloneError::Startup(format!("could not resolve horizon-browser: {error}")))?;
        let mut command = Command::new(executable);
        command.args(["mcp", "--standalone"]);
        if let Some(backend) = options.backend {
            command.args(["--backend", backend_name(backend)]);
        }
        if options.visible {
            command.arg("--visible");
        }
        let mut child = command
            .env("HOME", home)
            .env("RUST_LOG", "off")
            .env_remove("HORIZON_BROWSER_ACTOR")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|error| StandaloneError::Startup(format!("could not start browser host: {error}")))?;
        let Some(mut stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(StandaloneError::Startup(
                "browser host stdin was unavailable".to_string(),
            ));
        };
        let initialize = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"horizon-browser-job","version":"1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
"#;
        if let Err(error) = stdin.write_all(initialize).and_then(|()| stdin.flush()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(StandaloneError::Startup(format!(
                "could not initialize browser host: {error}"
            )));
        }
        let mut host = Self {
            child,
            stdin: Some(stdin),
        };
        let manifests = home.join(".horizon").join("runtime").join("browsers");
        let deadline = std::time::Instant::now() + STARTUP_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if !manifest::list_panels_in(&manifests).is_empty() {
                return Ok(host);
            }
            if let Some(status) = host
                .child
                .try_wait()
                .map_err(|error| StandaloneError::Startup(format!("could not inspect browser host: {error}")))?
            {
                return Err(StandaloneError::Startup(format!(
                    "browser host exited during startup ({status})"
                )));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = host.stop();
        Err(StandaloneError::Startup(
            "browser host did not publish a session within 30 seconds".to_string(),
        ))
    }

    /// Close MCP stdin and wait for bounded browser/profile cleanup.
    #[must_use]
    pub fn shutdown(mut self) -> bool {
        self.stop()
    }

    fn stop(&mut self) -> bool {
        self.stdin.take();
        let deadline = std::time::Instant::now() + HOST_EXIT_TIMEOUT;
        while std::time::Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => return false,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        false
    }
}

impl Drop for OwnedHostProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Start one browser session and serve the Horizon browser MCP contract until
/// stdin closes.
///
/// # Errors
///
/// Returns when no requested backend can start, MCP transport fails, or the
/// owned browser cannot be stopped within its bounded cleanup deadline.
pub async fn serve(options: StandaloneOptions) -> Result<(), StandaloneError> {
    let session = OwnedSession::start(options, &HorizonHome::resolve())?;
    let mcp_result = horizon_browser_mcp::serve_stdio().await;
    let stopped = session.shutdown();
    mcp_result?;
    if stopped {
        Ok(())
    } else {
        Err(StandaloneError::Shutdown)
    }
}

fn start(
    options: StandaloneOptions,
    home: &HorizonHome,
) -> Result<(BrowserSession, std::path::PathBuf), StandaloneError> {
    if options.backend == Some(BackendKind::SafariWebDriver) && !options.visible {
        return Err(StandaloneError::Startup(
            "Safari has no headless mode; pass --visible or choose another backend".to_string(),
        ));
    }
    let candidates = options.backend.map_or_else(
        || {
            let mut backends = vec![BackendKind::ChromiumCdp, BackendKind::FirefoxBidi];
            if cfg!(target_os = "macos") && options.visible {
                backends.push(BackendKind::SafariWebDriver);
            }
            backends
        },
        |backend| vec![backend],
    );
    let mut last_error = None;
    for backend in candidates {
        match start_backend(home, backend, options.visible) {
            Ok(session) => return Ok(session),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| StandaloneError::Startup("no compatible browser backend found".to_string())))
}

fn start_backend(
    home: &HorizonHome,
    backend: BackendKind,
    visible: bool,
) -> Result<(BrowserSession, std::path::PathBuf), StandaloneError> {
    let panel_id = standalone_panel_id();
    let profile_root = home.browser_profile_dir(&panel_id);
    let coordination = Arc::new(ManifestCoordination::default());
    let session = start_session(BrowserSessionConfig {
        browser: BrowserConfig {
            backend,
            headless: !visible,
            profile_root: Some(home.root().join("browser-profiles")),
            ..BrowserConfig::default()
        },
        panel_local_id: panel_id.clone(),
        initial_url: None,
        width: horizon_browser::DEFAULT_VIEWPORT.0,
        height: horizon_browser::DEFAULT_VIEWPORT.1,
        frame_slot: Arc::new(FrameSlot::new()),
        coordination: Some(coordination.clone()),
        capture_directory: Some(profile_root.join("captures")),
    })?;
    if let Err(error) = wait_until_ready(&session) {
        let shutdown = session.shutdown_signal().with_profile_cleanup(profile_root);
        let _ = shutdown.wait(SHUTDOWN_TIMEOUT) || shutdown.force_cleanup(FORCED_SHUTDOWN_TIMEOUT);
        return Err(error);
    }
    let entry = BrowserAuditEntry::new(
        new_action_id(),
        BrowserAuditActor::System,
        BrowserAuditStatus::Completed,
        BrowserAuditAction::session_created(backend, None, visible),
    );
    let publish = manifest::update(&panel_id, |manifest| {
        manifest.hidden = !visible;
        manifest.updated_at = manifest::now_millis();
    })
    .and_then(|_| coordination.record_action(&panel_id, &entry));
    if let Err(error) = publish {
        let shutdown = session.shutdown_signal().with_profile_cleanup(profile_root);
        let _ = shutdown.wait(SHUTDOWN_TIMEOUT) || shutdown.force_cleanup(FORCED_SHUTDOWN_TIMEOUT);
        return Err(StandaloneError::Startup(format!(
            "could not publish standalone browser state: {error}"
        )));
    }
    Ok((session, profile_root))
}

fn wait_until_ready(session: &BrowserSession) -> Result<(), StandaloneError> {
    let deadline = std::time::Instant::now() + STARTUP_TIMEOUT;
    let mut warning = None;
    while std::time::Instant::now() < deadline {
        match session.event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(BrowserEvent::Ready) => return Ok(()),
            Ok(BrowserEvent::Frame { .. }) => session.frame_slot.release_notification(),
            Ok(BrowserEvent::Warning(message)) => warning = Some(message),
            Ok(BrowserEvent::Stopped { code }) => {
                return Err(StandaloneError::Startup(
                    warning.unwrap_or_else(|| format!("browser stopped ({code:?})")),
                ));
            }
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(StandaloneError::Startup(
                    warning.unwrap_or_else(|| "browser driver disconnected".to_string()),
                ));
            }
        }
    }
    Err(StandaloneError::Startup(warning.unwrap_or_else(|| {
        "browser did not become ready within 30 seconds".to_string()
    })))
}

fn standalone_panel_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("standalone-{}-{nanos:x}", std::process::id())
}

const fn backend_name(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::ChromiumCdp => "chromium",
        BackendKind::FirefoxBidi => "firefox",
        BackendKind::SafariWebDriver => "safari",
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn owned_host_closes_stdin_before_waiting_for_exit() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "cat >/dev/null"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("start stdin waiter: {error}"));
        let stdin = child.stdin.take();
        let host = OwnedHostProcess { child, stdin };

        assert!(host.shutdown());
    }
}
