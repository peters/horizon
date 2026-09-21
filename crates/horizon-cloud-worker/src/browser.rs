use base64::Engine;
use horizon_browser::{
    BackendKind, BrowserConfig, BrowserEvent, BrowserSession, BrowserSessionConfig, FrameSlot, VideoCaptureHandle,
    start_session,
};
use horizon_browser_control::{
    BrowserRuntimePaths,
    manifest::{self, ManifestCoordination},
};
use horizon_browser_protocol::cloud_view::{CloudViewRequest, CloudViewResponse, CloudViewState};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
pub struct HostedBrowser {
    pub session: BrowserSession,
    pub state: CloudViewState,
}
pub struct Host {
    pub capabilities: horizon_cloud::Capabilities,
    pub browsers: BTreeMap<String, HostedBrowser>,
    pub pending: Vec<manifest::BrowserCreateRequest>,
    pub pending_cleanup: std::collections::BTreeSet<String>,
    root: PathBuf,
    closed: std::collections::BTreeSet<String>,
    stopping: BTreeMap<String, horizon_browser::BrowserShutdownSignal>,
    pub catalog: super::catalog::Host,
    pub remote_allocations: super::remote::Allocations,
}
impl Host {
    pub fn new() -> io::Result<Self> {
        let paths = BrowserRuntimePaths::resolve();
        std::fs::create_dir_all(paths.root())?;
        std::fs::write(paths.root().join("cloud-host-instance"), manifest::host_instance())?;
        let root = BrowserRuntimePaths::resolve().root().join("cloud-browser-history");
        std::fs::create_dir_all(&root)?;
        let mut closed = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            if entry.file_type()?.is_file() && std::fs::read(entry.path())? == b"closed" {
                closed.insert(entry.file_name().to_string_lossy().into_owned());
            }
        }
        let capabilities = match std::fs::read("/workspace/capabilities.json") {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| io::Error::other("Invalid worker capabilities"))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => horizon_cloud::Capabilities::default(),
            Err(error) => return Err(error),
        };
        let remote_allocations = super::remote::Allocations::new(root.join("remote-holds"))?;
        Ok(Self {
            capabilities,
            browsers: BTreeMap::new(),
            pending: Vec::new(),
            pending_cleanup: std::collections::BTreeSet::new(),
            root,
            closed,
            stopping: BTreeMap::new(),
            remote_allocations,
            catalog: crate::catalog::Host::default(),
        })
    }
    pub fn open(
        &mut self,
        id: &str,
        url: Option<String>,
        backend: Option<BackendKind>,
        target: Option<&str>,
        actor: &str,
    ) -> io::Result<()> {
        if !valid_identity(id) {
            return Err(io::Error::other("Invalid browser identity"));
        }
        if let Some(existing) = self.browsers.get(id) {
            validate_existing(&existing.state, backend, target)?;
            return Ok(());
        }
        let marker = self.root.join(id);
        if marker.exists() {
            return Err(io::Error::other(LOST_PROCESS));
        }
        if target.is_some() && Path::new("/run/horizon-credentials/browserstack-revoked").exists() {
            return Err(io::Error::other(
                "Remote credentials were revoked; reconnect only after restoring the local grant",
            ));
        }
        let remote = target
            .map(|name| super::remote::request(&self.capabilities, name, &self.catalog.cache))
            .transpose()?;
        let backend = match &remote {
            Some(remote) => remote.browser,
            None => selected_backend(&self.capabilities, backend)?,
        };
        let coordination =
            ManifestCoordination::with_remote_allocation(remote.as_ref().map(|request| request.recovery.clone()));
        if self.browsers.len() >= 16 {
            return Err(io::Error::other("Cloud browser capacity reached"));
        }
        if let Some(remote) = &remote {
            self.remote_allocations.insert(id, actor, remote)?;
        }
        let config = BrowserConfig {
            backend,
            headless: true,
            extra_args: if backend == BackendKind::ChromiumCdp {
                vec!["--no-sandbox".into(), "--disable-dev-shm-usage".into()]
            } else {
                Vec::new()
            },
            ..BrowserConfig::default()
        };
        let session = start_fenced(&marker, || {
            start_session(BrowserSessionConfig {
                browser: config,
                panel_local_id: id.into(),
                initial_url: url,
                width: 1280,
                height: 800,
                frame_slot: Arc::new(FrameSlot::new()),
                coordination: Some(Arc::new(coordination)),
                capture_directory: Some(
                    self.root
                        .join("profiles")
                        .join(horizon_browser_control::paths::safe_local_id(id))
                        .join("captures"),
                ),
                video: Arc::new(VideoCaptureHandle::default()),
                remote,
            })
            .map_err(io::Error::other)
        })
        .inspect_err(|_| self.remote_allocations.cancel_start(id))?;
        self.browsers.insert(
            id.into(),
            HostedBrowser {
                session,
                state: CloudViewState {
                    id: id.into(),
                    backend,
                    visible: true,
                    remote_target: target.map(str::to_owned),
                    ..CloudViewState::default()
                },
            },
        );
        Ok(())
    }
    pub fn drain(&mut self) {
        for browser in self.browsers.values_mut() {
            for event in browser.session.event_rx.try_iter() {
                match event {
                    BrowserEvent::RemoteSession(horizon_browser::RemoteSessionEvent::DeviceIdentity {
                        identity,
                        ..
                    }) => browser.state.remote_device = Some(identity.summary()),
                    BrowserEvent::Ready => browser.state.ready = true,
                    BrowserEvent::Title(title) => browser.state.title = title,
                    BrowserEvent::UrlChanged(url) => browser.state.url = url,
                    BrowserEvent::HandoffRequested(reason) => {
                        browser.state.handoff = Some(reason);
                        browser.state.handoff_error = None;
                        browser.state.handoff_sequence = browser.state.handoff_sequence.wrapping_add(1);
                    }
                    BrowserEvent::HandoffCleared => {
                        browser.state.handoff = None;
                        browser.state.handoff_error = None;
                        browser.state.handoff_sequence = browser.state.handoff_sequence.wrapping_add(1);
                    }
                    BrowserEvent::HandoffResolutionFailed(message) => {
                        browser.state.handoff_error = Some(message);
                        browser.state.handoff_sequence = browser.state.handoff_sequence.wrapping_add(1);
                    }
                    BrowserEvent::OwnerChanged(owner) => browser.state.owner = owner,
                    BrowserEvent::Stopped { .. } => {
                        browser.state.lost = true;
                        browser.state.ready = false;
                    }
                    BrowserEvent::Warning(message) => browser.state.error = Some(message),
                    BrowserEvent::Frame { .. } => browser.session.frame_slot.release_notification(),
                    _ => {}
                }
            }
        }
    }
    pub fn request(&mut self, request: CloudViewRequest) -> CloudViewResponse {
        match request {
            CloudViewRequest::Open {
                id,
                url: _,
                backend,
                target,
            } if valid_identity(&id) && !self.browsers.contains_key(&id) && self.root.join(&id).exists() => {
                lost_response(&id, backend, target)
            }
            CloudViewRequest::Open {
                id,
                url,
                backend,
                target,
            } => match self.open(&id, url, backend, target.as_deref(), "") {
                Ok(()) => self.snapshot(&id, 0),
                Err(e) => CloudViewResponse {
                    error: Some(e.to_string()),
                    ..CloudViewResponse::default()
                },
            },
            CloudViewRequest::Poll { id, after, commands } => {
                if commands.len() > 128 {
                    return CloudViewResponse {
                        error: Some("Too many browser inputs".into()),
                        ..CloudViewResponse::default()
                    };
                }
                if let Some(browser) = self.browsers.get(&id) {
                    for command in commands {
                        // Closing a client presentation must not terminate the worker browser.
                        if !matches!(command, horizon_browser::BrowserCommand::Stop) {
                            let _ = browser.session.send(command);
                        }
                    }
                }
                self.snapshot(&id, after)
            }
            CloudViewRequest::Close { id } => match self.close(&id) {
                Ok(()) => CloudViewResponse {
                    closed: vec![id],
                    ..CloudViewResponse::default()
                },
                Err(e) => CloudViewResponse {
                    error: Some(e.to_string()),
                    ..CloudViewResponse::default()
                },
            },
            CloudViewRequest::RevokeRemote => {
                if let Err(error) = self
                    .catalog
                    .revoke(Path::new("/run/horizon-credentials/browserstack-revoked"))
                {
                    return CloudViewResponse {
                        error: Some(error.to_string()),
                        ..CloudViewResponse::default()
                    };
                }
                self.remote_allocations.reconcile_retained_for_host(&self.capabilities);
                if let Err(error) = self.remote_allocations.ensure_recoverable() {
                    return CloudViewResponse {
                        error: Some(error.to_string()),
                        ..CloudViewResponse::default()
                    };
                }
                let ids: Vec<_> = self.remote_allocations.ids();
                for id in &ids {
                    if let Err(error) = self.close(id) {
                        return CloudViewResponse {
                            error: Some(error.to_string()),
                            ..CloudViewResponse::default()
                        };
                    }
                }
                CloudViewResponse {
                    closed: ids,
                    ..CloudViewResponse::default()
                }
            }
            CloudViewRequest::List => CloudViewResponse {
                closed: self.closed.iter().cloned().collect(),
                browsers: self.browsers.values().map(|b| b.state.clone()).collect(),
                error: None,
                ..CloudViewResponse::default()
            },
        }
    }
    pub fn close(&mut self, id: &str) -> io::Result<()> {
        if !valid_identity(id) {
            return Err(io::Error::other("Invalid browser identity"));
        }
        self.pending_cleanup.insert(id.into());
        self.begin_shutdown(id);
        if let Some(signal) = self.stopping.get(id)
            && !signal.wait(std::time::Duration::from_secs(10))
            && !signal.force_cleanup(std::time::Duration::from_secs(3))
        {
            return Err(io::Error::other("Browser shutdown is unresolved"));
        }
        self.finish_close(id)?;
        self.pending_cleanup.remove(id);
        Ok(())
    }
    fn begin_shutdown(&mut self, id: &str) {
        if let Some(browser) = self.browsers.remove(id) {
            self.stopping.insert(id.into(), browser.session.shutdown_signal());
        }
    }
    fn finish_close(&mut self, id: &str) -> io::Result<()> {
        if self
            .stopping
            .get(id)
            .is_some_and(horizon_browser::BrowserShutdownSignal::holds_remote_allocation)
        {
            return Err(io::Error::other(
                "Remote device release is unconfirmed; reconcile the retained allocation before closing",
            ));
        }
        self.remote_allocations.confirm_closed(id)?;
        self.stopping.remove(id);
        self.closed.insert(id.into());
        std::fs::write(self.root.join(id), "closed")?;
        Ok(())
    }
    pub fn retry_cleanup(&mut self) {
        for id in std::mem::take(&mut self.pending_cleanup) {
            self.begin_shutdown(&id);
            if self.stopping.get(&id).is_some_and(|signal| !signal.is_complete()) || self.finish_close(&id).is_err() {
                self.pending_cleanup.insert(id);
            }
        }
    }
    fn snapshot(&self, id: &str, after: u64) -> CloudViewResponse {
        let Some(browser) = self.browsers.get(id) else {
            return lost_response(id, None, None);
        };
        let mut state = browser.state.clone();
        if let Some(frame) = browser.session.frame_slot.latest() {
            if frame.width > 4096 || frame.height > 4096 {
                return CloudViewResponse {
                    error: Some("Cloud browser presentation supports at most 4096 pixels per axis".into()),
                    ..CloudViewResponse::default()
                };
            }
            state.sequence = frame.seq;
            if frame.seq != after {
                state.png = encode_frame(&frame).ok();
            }
        }
        CloudViewResponse {
            browsers: vec![state],
            error: None,
            ..CloudViewResponse::default()
        }
    }
}

const LOST_PROCESS: &str = "Browser process was lost; create a new browser explicitly";

fn valid_identity(id: &str) -> bool {
    !id.is_empty() && id.len() <= 100 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn lost_response(id: &str, backend: Option<BackendKind>, target: Option<String>) -> CloudViewResponse {
    CloudViewResponse {
        browsers: vec![CloudViewState {
            id: id.into(),
            backend: backend.unwrap_or_default(),
            remote_target: target,
            lost: true,
            error: Some(LOST_PROCESS.into()),
            ..CloudViewState::default()
        }],
        ..CloudViewResponse::default()
    }
}

fn validate_existing(state: &CloudViewState, backend: Option<BackendKind>, target: Option<&str>) -> io::Result<()> {
    if state.remote_target.as_deref() != target {
        return Err(io::Error::other("Existing browser uses a different remote target"));
    }
    if target.is_none() && backend.is_some_and(|backend| backend != state.backend) {
        return Err(io::Error::other("Existing browser uses a different engine"));
    }
    Ok(())
}

// start_session only returns Err before its driver thread has started. Once it
// returns a session, even a later asynchronous failure must retain the fence.
fn start_fenced<T>(marker: &Path, start: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker)?
        .sync_all()?;
    sync_marker_directory(marker)?;
    match start() {
        Ok(session) => Ok(session),
        Err(error) => {
            std::fs::remove_file(marker)?;
            sync_marker_directory(marker)?;
            Err(error)
        }
    }
}

fn sync_marker_directory(marker: &Path) -> io::Result<()> {
    let parent = marker
        .parent()
        .ok_or_else(|| io::Error::other("Browser history has no directory"))?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

fn selected_backend(
    capabilities: &horizon_cloud::Capabilities,
    requested: Option<BackendKind>,
) -> io::Result<BackendKind> {
    use horizon_cloud::BrowserEngine;
    let backend = requested
        .or_else(|| {
            capabilities.browsers.first().map(|browser| match browser {
                BrowserEngine::Chromium => BackendKind::ChromiumCdp,
                BrowserEngine::Firefox => BackendKind::FirefoxBidi,
            })
        })
        .ok_or_else(|| io::Error::other("Browsers are disabled by this cloud profile"))?;
    let engine = match backend {
        BackendKind::ChromiumCdp => BrowserEngine::Chromium,
        BackendKind::FirefoxBidi => BrowserEngine::Firefox,
        BackendKind::SafariWebDriver => {
            return Err(io::Error::other("This browser engine is unavailable on the worker"));
        }
    };
    if !capabilities.browsers.contains(&engine) {
        return Err(io::Error::other("Requested browser is disabled by this cloud profile"));
    }
    Ok(backend)
}

fn encode_frame(frame: &horizon_browser::frames::FrameData) -> io::Result<String> {
    let mut data = Vec::new();
    let mut encoder = png::Encoder::new(&mut data, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&frame.rgb)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(data))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fenced_browser_returns_typed_loss_before_credentials_or_capacity_validation() {
        let root = tempfile::tempdir().unwrap();
        let mut host = Host {
            capabilities: serde_json::from_str("{}").unwrap(),
            catalog: crate::catalog::Host::default(),
            browsers: BTreeMap::new(),
            pending: Vec::new(),
            pending_cleanup: std::collections::BTreeSet::new(),
            root: root.path().into(),
            closed: std::collections::BTreeSet::default(),
            stopping: BTreeMap::new(),
            remote_allocations: super::super::remote::Allocations::new(root.path().join("remote-holds")).unwrap(),
        };
        std::fs::write(root.path().join("lost"), "").unwrap();
        let response = host.request(CloudViewRequest::Open {
            id: "lost".into(),
            url: None,
            backend: Some(BackendKind::FirefoxBidi),
            target: Some("unconfigured-target".into()),
        });
        assert!(response.error.is_none());
        let state = &response.browsers[0];
        assert!(state.lost);
        assert_eq!(state.id, "lost");
        assert_eq!(state.backend, BackendKind::FirefoxBidi);
        assert_eq!(state.remote_target.as_deref(), Some("unconfigured-target"));
        assert!(state.error.as_deref().unwrap().contains("create a new browser"));
        assert!(host.browsers.is_empty());
        assert!(host.remote_allocations.ids().is_empty());
        let polled = host.request(CloudViewRequest::Poll {
            id: "lost".into(),
            after: 0,
            commands: Vec::new(),
        });
        assert!(polled.error.is_none());
        assert!(polled.browsers[0].lost);
        assert!(
            host.request(CloudViewRequest::Open {
                id: "../lost".into(),
                url: None,
                backend: None,
                target: None,
            })
            .error
            .is_some()
        );
    }
    #[test]
    fn completed_driver_does_not_prove_remote_release_or_discard_recovery() {
        let root = tempfile::tempdir().unwrap();
        let mut host = Host {
            capabilities: horizon_cloud::Capabilities::default(),
            catalog: crate::catalog::Host::default(),
            browsers: BTreeMap::new(),
            pending: Vec::new(),
            pending_cleanup: std::collections::BTreeSet::new(),
            root: root.path().into(),
            closed: std::collections::BTreeSet::default(),
            stopping: BTreeMap::new(),
            remote_allocations: super::super::remote::Allocations::new(root.path().join("remote-holds")).unwrap(),
        };
        let (completion, signal) = std::sync::mpsc::channel();
        host.stopping
            .insert("phone".into(), horizon_browser::BrowserShutdownSignal::for_test(signal));
        host.pending_cleanup.insert("phone".into());
        let started = std::time::Instant::now();
        host.retry_cleanup();
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(host.pending_cleanup.contains("phone"));
        assert!(host.request(CloudViewRequest::List).error.is_none());
        completion.send(()).unwrap();
        host.retry_cleanup();
        assert!(host.pending_cleanup.is_empty());
        assert!(host.closed.remove("phone"));
        std::fs::remove_file(root.path().join("phone")).unwrap();
        host.stopping.insert(
            "phone".into(),
            horizon_browser::BrowserShutdownSignal::completed_remote_for_test("account", None),
        );
        assert!(host.close("phone").is_err());
        assert!(host.pending_cleanup.contains("phone"));
        host.retry_cleanup();
        assert!(host.pending_cleanup.contains("phone"));
        assert!(host.stopping.contains_key("phone"));
        assert!(!host.closed.contains("phone"));
        assert!(!root.path().join("phone").exists());
        host.stopping.insert(
            "phone".into(),
            horizon_browser::BrowserShutdownSignal::completed_remote_for_test(
                "account",
                Some(horizon_browser::RemoteReleaseOutcome::Released),
            ),
        );
        host.retry_cleanup();
        assert!(host.pending_cleanup.is_empty());
        assert!(!host.stopping.contains_key("phone"));
        assert!(host.closed.contains("phone"));
        assert_eq!(std::fs::read(root.path().join("phone")).unwrap(), b"closed");
    }
    #[test]
    fn existing_browser_cannot_switch_target_or_cross_local_remote_boundary() {
        let remote = CloudViewState {
            remote_target: Some("phone-a".into()),
            backend: BackendKind::SafariWebDriver,
            ..Default::default()
        };
        assert!(validate_existing(&remote, None, Some("phone-a")).is_ok());
        assert!(validate_existing(&remote, None, Some("phone-b")).is_err());
        assert!(validate_existing(&remote, None, None).is_err());
        assert!(validate_existing(&CloudViewState::default(), None, Some("phone-a")).is_err());
    }
    #[test]
    fn launch_is_fenced_before_start_and_cannot_be_replayed_after_interruption() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("browser");
        let interrupted = std::panic::catch_unwind(|| {
            let _: io::Result<()> = start_fenced(&marker, || {
                assert!(marker.is_file());
                panic!("simulated interruption before launch result");
            });
        });
        assert!(interrupted.is_err());
        assert!(start_fenced::<()>(&marker, || panic!("must not relaunch")).is_err());
    }

    #[test]
    fn definite_prelaunch_failure_releases_the_identity_but_success_retains_it() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("browser");
        let failed: io::Result<()> = start_fenced(&marker, || Err(io::Error::other("driver thread not started")));
        assert!(failed.is_err());
        assert!(!marker.exists());
        assert_eq!(start_fenced(&marker, || Ok(42)).unwrap(), 42);
        assert!(marker.is_file());
        assert!(start_fenced(&marker, || Ok(43)).is_err());
    }
    #[test]
    fn disabled_engines_are_rejected_and_firefox_only_defaults_to_firefox() {
        let mut capabilities: horizon_cloud::Capabilities = serde_json::from_str("{}").unwrap();
        assert!(selected_backend(&capabilities, None).is_err());
        assert!(selected_backend(&capabilities, Some(BackendKind::ChromiumCdp)).is_err());
        capabilities.browsers.insert(horizon_cloud::BrowserEngine::Firefox);
        assert_eq!(selected_backend(&capabilities, None).unwrap(), BackendKind::FirefoxBidi);
        assert!(selected_backend(&capabilities, Some(BackendKind::ChromiumCdp)).is_err());
        capabilities.browsers.insert(horizon_cloud::BrowserEngine::Chromium);
        assert_eq!(
            selected_backend(&capabilities, Some(BackendKind::FirefoxBidi)).unwrap(),
            BackendKind::FirefoxBidi
        );
        assert!(selected_backend(&capabilities, Some(BackendKind::SafariWebDriver)).is_err());
    }
    #[test]
    fn high_entropy_4k_frame_survives_transport_and_decode() {
        let mut seed = 0x1234_5678_u32;
        let rgb: Vec<_> = (0..3840 * 2160 * 3)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed.to_le_bytes()[0]
            })
            .collect();
        let frame = horizon_browser::frames::FrameData {
            width: 3840,
            height: 2160,
            rgb,
            seq: 1,
        };
        let encoded = encode_frame(&frame).unwrap();
        let response = CloudViewResponse {
            browsers: vec![CloudViewState {
                id: "frame-test".into(),
                png: Some(encoded),
                ..CloudViewState::default()
            }],
            ..CloudViewResponse::default()
        };
        let mut line = serde_json::to_string(&response).unwrap();
        line.push('\n');
        assert!(line.len() > 16 * 1024 * 1024);
        assert!(line.len() < horizon_browser_protocol::cloud_view::MAX_CLOUD_VIEW_BYTES);
        let line = crate::read_line(&mut std::io::Cursor::new(line)).unwrap().unwrap();
        let response: CloudViewResponse = serde_json::from_str(&line).unwrap();
        let slot = FrameSlot::new();
        assert!(
            slot.store_base64_png(response.browsers[0].png.as_deref().unwrap())
                .is_some()
        );
        let decoded = slot.latest().unwrap();
        assert_eq!((decoded.width, decoded.height), (3840, 2160));
        assert_eq!(decoded.rgb, frame.rgb);
    }
}
