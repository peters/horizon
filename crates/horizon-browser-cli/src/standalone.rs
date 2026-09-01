use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Write as _};
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
const HOST_INITIALIZE: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"horizon-browser-job","version":"1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
"#;

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

trait HostChild {
    fn try_wait_success(&mut self) -> io::Result<Option<bool>>;
    fn kill(&mut self) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<()>;
}

impl HostChild for Child {
    fn try_wait_success(&mut self) -> io::Result<Option<bool>> {
        self.try_wait().map(|status| status.map(|status| status.success()))
    }

    fn kill(&mut self) -> io::Result<()> {
        Child::kill(self)
    }

    fn wait(&mut self) -> io::Result<()> {
        Child::wait(self).map(|_| ())
    }
}

impl OwnedHostProcess {
    /// Start an owned standalone MCP subprocess with private host state.
    ///
    /// # Errors
    /// Returns when the subprocess cannot start, initialize, or publish its
    /// first browser manifest before the startup deadline.
    pub fn start(
        options: StandaloneOptions,
        home: &Path,
        stderr: File,
    ) -> Result<(Self, BackendKind), StandaloneError> {
        let horizon_root = home.join(".horizon");
        let manifests = horizon_root.join("runtime").join("browsers");
        let existing_manifests = manifest::list_panels_in(&manifests).into_iter().collect::<HashSet<_>>();
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
            .env_remove("HORIZON")
            .env_remove("HORIZON_BROWSER_ACTOR")
            .env_remove(manifest::HOST_INSTANCE_ENV)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|error| StandaloneError::Startup(format!("could not start browser host: {error}")))?;
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(StandaloneError::Startup(
                "browser host stdin was unavailable".to_string(),
            ));
        };
        let child_id = child.id();
        let mut host = Self {
            child,
            stdin: Some(stdin),
        }
        .initialize()?;
        let deadline = std::time::Instant::now() + STARTUP_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if let Some(backend) = host_manifest_backend(&horizon_root, &existing_manifests, child_id) {
                return Ok((host, backend));
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

    fn initialize(mut self) -> Result<Self, StandaloneError> {
        let initialized = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "browser host stdin was unavailable"))
            .and_then(|stdin| stdin.write_all(HOST_INITIALIZE).and_then(|()| stdin.flush()));
        if let Err(error) = initialized {
            self.force_stop();
            return Err(StandaloneError::Startup(format!(
                "could not initialize browser host: {error}"
            )));
        }
        Ok(self)
    }

    fn stop(&mut self) -> bool {
        stop_host(&mut self.child, &mut self.stdin, HOST_EXIT_TIMEOUT)
    }

    fn force_stop(&mut self) {
        self.stdin.take();
        force_host_cleanup(&mut self.child);
    }
}

fn stop_host(child: &mut impl HostChild, stdin: &mut Option<ChildStdin>, timeout: Duration) -> bool {
    stdin.take();
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        match child.try_wait_success() {
            Ok(Some(success)) => return success,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
    force_host_cleanup(child);
    false
}

fn force_host_cleanup(child: &mut impl HostChild) {
    let _ = child.kill();
    let _ = child.wait();
}

fn host_manifest_backend(root: &Path, existing: &HashSet<String>, child_id: u32) -> Option<BackendKind> {
    let expected_prefix = format!("standalone-{child_id}-");
    manifest::list_panels_in(&root.join("runtime").join("browsers"))
        .into_iter()
        .find(|panel_id| panel_id.starts_with(&expected_prefix) && !existing.contains(panel_id))
        .and_then(|panel_id| manifest::read_at(&manifest::manifest_path_for_root(root, &panel_id)))
        .map(|manifest| manifest.backend)
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
    let mcp_result = horizon_browser_mcp::serve_stdio_standalone().await;
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
    let mut browser = BrowserConfig {
        backend,
        headless: !visible,
        ..BrowserConfig::default()
    };
    if browser.profile_root.is_none() {
        browser.profile_root = Some(browser.effective_profile_root(&home.root().join("browser-profiles")));
    }
    let profile_root = browser.panel_profile_dir_with_default_root(&panel_id, &home.root().join("browser-profiles"));
    let coordination = Arc::new(ManifestCoordination::default());
    let session = start_session(BrowserSessionConfig {
        browser,
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
    use std::io::Read as _;

    use super::*;

    struct TryWaitFailure {
        tries: usize,
        kills: usize,
        waits: usize,
    }

    impl HostChild for TryWaitFailure {
        fn try_wait_success(&mut self) -> io::Result<Option<bool>> {
            self.tries += 1;
            Err(io::Error::other("forced try_wait failure"))
        }

        fn kill(&mut self) -> io::Result<()> {
            self.kills += 1;
            Ok(())
        }

        fn wait(&mut self) -> io::Result<()> {
            self.waits += 1;
            Ok(())
        }
    }

    #[test]
    fn initialization_write_failure_reaps_the_child() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exec 0<&-; printf ready; exec sleep 30"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("start closed-stdin fixture: {error}"));
        let child_id = child.id();
        let mut ready = [0; 5];
        child
            .stdout
            .take()
            .unwrap_or_else(|| panic!("fixture stdout unavailable"))
            .read_exact(&mut ready)
            .unwrap_or_else(|error| panic!("wait for fixture readiness: {error}"));
        assert_eq!(&ready, b"ready");
        let stdin = child.stdin.take();

        let result = OwnedHostProcess { child, stdin }.initialize();
        assert!(matches!(result, Err(StandaloneError::Startup(_))));

        let alive = Command::new("kill")
            .args(["-0", &child_id.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        assert!(!alive, "failed initialization left child {child_id} alive or unreaped");
    }

    #[test]
    fn try_wait_error_forces_kill_and_reap() {
        let mut child = TryWaitFailure {
            tries: 0,
            kills: 0,
            waits: 0,
        };
        let mut stdin = None;

        assert!(!stop_host(&mut child, &mut stdin, Duration::from_secs(1)));
        assert_eq!(child.tries, 1);
        assert_eq!(child.kills, 1);
        assert_eq!(child.waits, 1);
    }

    #[test]
    fn host_readiness_returns_the_exact_child_backend() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("create temp home: {error}"));
        let stale = "standalone-42-before";
        manifest::write_at(
            &manifest::manifest_path_for_root(home.path(), stale),
            &horizon_core::browser::manifest::BrowserManifest {
                panel_local_id: stale.to_string(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|error| panic!("write stale manifest: {error}"));
        let existing = HashSet::from([stale.to_string()]);

        assert_eq!(host_manifest_backend(home.path(), &existing, 42), None);

        let unrelated = "standalone-7-after";
        manifest::write_at(
            &manifest::manifest_path_for_root(home.path(), unrelated),
            &horizon_core::browser::manifest::BrowserManifest {
                panel_local_id: unrelated.to_string(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|error| panic!("write unrelated manifest: {error}"));
        assert_eq!(host_manifest_backend(home.path(), &existing, 42), None);

        let current = "standalone-42-after";
        manifest::write_at(
            &manifest::manifest_path_for_root(home.path(), current),
            &horizon_core::browser::manifest::BrowserManifest {
                panel_local_id: current.to_string(),
                backend: BackendKind::FirefoxBidi,
                ..Default::default()
            },
        )
        .unwrap_or_else(|error| panic!("write current manifest: {error}"));
        assert_eq!(
            host_manifest_backend(home.path(), &existing, 42),
            Some(BackendKind::FirefoxBidi)
        );
    }

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
