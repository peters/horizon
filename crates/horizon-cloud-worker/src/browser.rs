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
use std::{collections::BTreeMap, io, path::PathBuf, sync::Arc};
pub struct HostedBrowser {
    pub session: BrowserSession,
    pub state: CloudViewState,
}
pub struct Host {
    pub capabilities: horizon_cloud::Capabilities,
    pub browsers: BTreeMap<String, HostedBrowser>,
    pub pending: Vec<manifest::BrowserCreateRequest>,
    root: PathBuf,
    closed: std::collections::BTreeSet<String>,
    stopping: BTreeMap<String, horizon_browser::BrowserShutdownSignal>,
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
        Ok(Self {
            capabilities,
            browsers: BTreeMap::new(),
            pending: Vec::new(),
            root,
            closed,
            stopping: BTreeMap::new(),
        })
    }
    pub fn open(&mut self, id: &str, url: Option<String>, backend: Option<BackendKind>) -> io::Result<()> {
        if id.is_empty() || id.len() > 100 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Err(io::Error::other("Invalid browser identity"));
        }
        if let Some(existing) = self.browsers.get(id) {
            if backend.is_some_and(|backend| backend != existing.state.backend) {
                return Err(io::Error::other("Existing browser uses a different engine"));
            }
            return Ok(());
        }
        let backend = selected_backend(&self.capabilities, backend)?;
        let marker = self.root.join(id);
        if marker.exists() {
            return Err(io::Error::other(
                "Browser process was lost; create a new browser explicitly",
            ));
        }
        if self.browsers.len() >= 16 {
            return Err(io::Error::other("Cloud browser capacity reached"));
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
        let session = start_session(BrowserSessionConfig {
            browser: config,
            panel_local_id: id.into(),
            initial_url: url,
            width: 1280,
            height: 800,
            frame_slot: Arc::new(FrameSlot::new()),
            coordination: Some(Arc::new(ManifestCoordination::default())),
            capture_directory: Some(self.root.join("captures")),
            video: Arc::new(VideoCaptureHandle::default()),
            remote: None,
        })
        .map_err(io::Error::other)?;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(marker)?
            .sync_all()?;
        self.browsers.insert(
            id.into(),
            HostedBrowser {
                session,
                state: CloudViewState {
                    id: id.into(),
                    backend,
                    visible: true,
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
            CloudViewRequest::Open { id, url, backend } => match self.open(&id, url, backend) {
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
            CloudViewRequest::List => CloudViewResponse {
                closed: self.closed.iter().cloned().collect(),
                browsers: self.browsers.values().map(|b| b.state.clone()).collect(),
                error: None,
                ..CloudViewResponse::default()
            },
        }
    }
    pub fn close(&mut self, id: &str) -> io::Result<()> {
        if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') || id.is_empty() || id.len() > 100 {
            return Err(io::Error::other("Invalid browser identity"));
        }
        if let Some(browser) = self.browsers.remove(id) {
            self.stopping.insert(id.into(), browser.session.shutdown_signal());
        }
        if let Some(signal) = self.stopping.get(id)
            && !signal.wait(std::time::Duration::from_secs(10))
            && !signal.force_cleanup(std::time::Duration::from_secs(3))
        {
            return Err(io::Error::other("Browser shutdown is unresolved"));
        }
        self.stopping.remove(id);
        self.closed.insert(id.into());
        std::fs::write(self.root.join(id), "closed")?;
        Ok(())
    }
    fn snapshot(&self, id: &str, after: u64) -> CloudViewResponse {
        let Some(browser) = self.browsers.get(id) else {
            return CloudViewResponse {
                error: Some("Browser process no longer exists".into()),
                ..CloudViewResponse::default()
            };
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
