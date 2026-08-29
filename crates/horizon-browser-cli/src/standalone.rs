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

/// Start one browser session and serve the Horizon browser MCP contract until
/// stdin closes.
///
/// # Errors
///
/// Returns when no requested backend can start, MCP transport fails, or the
/// owned browser cannot be stopped within its bounded cleanup deadline.
pub async fn serve(options: StandaloneOptions) -> Result<(), StandaloneError> {
    let (session, profile_root) = start(options)?;
    let mcp_result = horizon_browser_mcp::serve_stdio().await;
    let shutdown = session.shutdown_signal().with_profile_cleanup(profile_root);
    let stopped = shutdown.wait(SHUTDOWN_TIMEOUT) || shutdown.force_cleanup(FORCED_SHUTDOWN_TIMEOUT);
    mcp_result?;
    if stopped {
        Ok(())
    } else {
        Err(StandaloneError::Shutdown)
    }
}

fn start(options: StandaloneOptions) -> Result<(BrowserSession, std::path::PathBuf), StandaloneError> {
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
        match start_backend(backend, options.visible) {
            Ok(session) => return Ok(session),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| StandaloneError::Startup("no compatible browser backend found".to_string())))
}

fn start_backend(backend: BackendKind, visible: bool) -> Result<(BrowserSession, std::path::PathBuf), StandaloneError> {
    let home = HorizonHome::resolve();
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
