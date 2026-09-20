//! An SSH presentation connection. Dropping it never stops the worker browser.
use super::{BrowserCommand, BrowserDrainOutput, BrowserEventWaker, BrowserPanelState, BrowserStatus, FrameSlot};
use crate::cloud_runtime::ssh::Connection;
use horizon_browser_protocol::cloud_view::{CloudViewRequest, CloudViewResponse, CloudViewState, MAX_CLOUD_VIEW_BYTES};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

type Latest = Arc<Mutex<Option<CloudViewState>>>;
type Waker = Arc<Mutex<Option<BrowserEventWaker>>>;
pub(super) struct CloudView {
    connection: Connection,
    id: String,
    initial_url: Option<String>,
    backend: super::BackendKind,
    pub(super) target: Option<String>,
    pub(super) device: Option<String>,
    pub(super) process_lost: bool,
    tx: mpsc::SyncSender<BrowserCommand>,
    latest: Latest,
    waker: Waker,
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Child>>,
    handoff_sequence: std::sync::atomic::AtomicU64,
}
impl CloudView {
    fn start(
        connection: Connection,
        id: String,
        url: Option<String>,
        target: Option<String>,
        backend: super::BackendKind,
        frames: Arc<FrameSlot>,
    ) -> io::Result<Self> {
        let mut child = connection
            .command("horizon-cloud-worker connect")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("SSH input unavailable"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("SSH output unavailable"))?;
        let child = Arc::new(Mutex::new(child));
        let (tx, rx) = mpsc::sync_channel(128);
        let (replies, responses) = mpsc::sync_channel(2);
        thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut line = String::new();
                match reader
                    .by_ref()
                    .take(MAX_CLOUD_VIEW_BYTES as u64 + 1)
                    .read_line(&mut line)
                {
                    Ok(n) if n > 0 && n <= MAX_CLOUD_VIEW_BYTES => {
                        let Ok(reply) = serde_json::from_str::<CloudViewResponse>(&line) else {
                            break;
                        };
                        if replies.send(reply).is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
        });
        let latest = Latest::default();
        let waker = Waker::default();
        let stop = Arc::new(AtomicBool::new(false));
        let view = Self {
            connection,
            id,
            initial_url: url,
            target,
            device: None,
            process_lost: false,
            backend,
            tx,
            latest,
            waker,
            stop,
            child,
            handoff_sequence: std::sync::atomic::AtomicU64::new(0),
        };
        let (latest, waker, stop, child) = (
            view.latest.clone(),
            view.waker.clone(),
            view.stop.clone(),
            view.child.clone(),
        );
        let open = CloudViewRequest::Open {
            id: view.id.clone(),
            url: view.initial_url.clone(),
            backend: view.target.is_none().then_some(view.backend),
            target: view.target.clone(),
        };
        thread::spawn(move || {
            let result = pump(input, &responses, &rx, &frames, &latest, &waker, &stop, open);
            if let Err(e) = result {
                *latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(CloudViewState {
                    error: Some(e.to_string()),
                    backend,
                    ..CloudViewState::default()
                });
                wake(&waker);
            }
            let mut child = child.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = child.kill();
            let _ = child.wait();
        });
        Ok(view)
    }
    pub(super) fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        let _ = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .kill();
    }
    pub(super) fn send(&self, command: BrowserCommand) -> bool {
        self.tx.try_send(command).is_ok()
    }
    pub(super) fn needs_waker(&self) -> bool {
        self.waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none()
    }
    pub(super) fn set_waker(&self, waker: BrowserEventWaker) {
        *self.waker.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(waker);
    }
}
impl Drop for CloudView {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .kill();
    }
}
fn wake(waker: &Waker) {
    if let Some(waker) = waker.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
        waker();
    }
}
#[expect(
    clippy::too_many_arguments,
    reason = "One owned transport loop with independent input, output and cancellation channels"
)]
fn pump(
    mut input: impl Write,
    responses: &mpsc::Receiver<CloudViewResponse>,
    commands: &mpsc::Receiver<BrowserCommand>,
    frames: &FrameSlot,
    latest: &Latest,
    waker: &Waker,
    stop: &AtomicBool,
    mut request: CloudViewRequest,
) -> io::Result<()> {
    let id = match &request {
        CloudViewRequest::Open { id, .. } => id.clone(),
        _ => return Ok(()),
    };
    while !stop.load(Ordering::Acquire) {
        serde_json::to_writer(&mut input, &request)?;
        input.write_all(b"\n")?;
        input.flush()?;
        let deadline = Instant::now() + Duration::from_secs(35);
        let response = loop {
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            match responses.recv_timeout(Duration::from_millis(100)) {
                Ok(response) => break response,
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                _ => {
                    return Err(io::Error::other(
                        "Cloud browser connection interrupted; reconnect to attach again",
                    ));
                }
            }
        };
        if let Some(error) = response.error {
            return Err(io::Error::other(error));
        }
        let mut state = response
            .browsers
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| io::Error::other("Worker browser identity mismatch"))?;
        let after = state.sequence;
        if let Some(png) = state.png.take() {
            let _ = frames.store_base64_png(&png);
        }
        *latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(state);
        wake(waker);
        thread::sleep(Duration::from_millis(150));
        request = CloudViewRequest::Poll {
            id: id.clone(),
            after,
            commands: commands.try_iter().take(128).collect(),
        };
    }
    Ok(())
}
impl BrowserPanelState {
    /// # Errors
    /// The authenticated presentation connection could not be launched.
    pub fn start_cloud(
        id: String,
        connection: Connection,
        target: Option<String>,
        url: Option<String>,
        config: &super::BrowserConfig,
    ) -> crate::Result<Self> {
        let mut state = Self::inert();
        state.panel_local_id.clone_from(&id);
        state.config = config.clone();
        state.status = BrowserStatus::Starting;
        state.requested_url.clone_from(&url);
        state.cloud = Some(CloudView::start(
            connection,
            id,
            url,
            target,
            config.backend,
            state.frame_slot.clone(),
        )?);
        Ok(state)
    }
    pub(super) fn drain_cloud(&mut self) -> BrowserDrainOutput {
        let state = self.cloud.as_ref().and_then(|cloud| {
            cloud
                .latest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        });
        let Some(state) = state else {
            return BrowserDrainOutput::default();
        };
        let changed = self.cloud.as_ref().is_some_and(|cloud| {
            cloud.handoff_sequence.swap(state.handoff_sequence, Ordering::Relaxed) != state.handoff_sequence
        });
        self.apply_cloud_state(state, changed)
    }
    fn apply_cloud_state(&mut self, mut state: CloudViewState, coordination_changed: bool) -> BrowserDrainOutput {
        let url_changed = !state.url.is_empty() && self.url.as_deref() != Some(&state.url);
        if url_changed {
            self.url = Some(state.url);
            self.pending_user_navigation = None;
        }
        if let Some(cloud) = &mut self.cloud {
            cloud.process_lost |= state.lost;
            if state.error.is_some() {
                state.remote_target = state.remote_target.or_else(|| cloud.target.clone());
                state.remote_device = state.remote_device.or_else(|| cloud.device.clone());
            }
            cloud.target.clone_from(&state.remote_target);
            cloud.device.clone_from(&state.remote_device);
        }
        self.remote_status = state.remote_target.as_ref().map(|target| {
            format!(
                "{target} · {}",
                state
                    .remote_device
                    .as_deref()
                    .unwrap_or("Awaiting provider confirmation")
            )
        });
        self.config.backend = state.backend;
        self.title = state.title;
        self.owner = state.owner;
        if coordination_changed || state.handoff.is_none() || state.handoff_error.is_some() {
            self.handoff_resolution_pending = false;
            self.handoff_error = state.handoff_error;
        }
        self.handoff_reason = state.handoff;
        self.loading = !state.ready;
        self.status = if let Some(message) = state.error {
            BrowserStatus::Error { message }
        } else if state.lost {
            BrowserStatus::Error {
                message: "Remote browser process was lost".into(),
            }
        } else if state.ready {
            BrowserStatus::Ready
        } else {
            BrowserStatus::Starting
        };
        BrowserDrainOutput {
            had_output: true,
            url_changed,
            config_changed: false,
        }
    }
    pub(super) fn relaunch_cloud(&mut self) {
        if !self.can_retry() {
            return;
        }
        let Some(old) = self.cloud.take() else { return };
        let next = CloudView::start(
            old.connection.clone(),
            old.id.clone(),
            old.initial_url.clone(),
            old.target.clone(),
            old.backend,
            self.frame_slot.clone(),
        );
        match next {
            Ok(view) => {
                self.cloud = Some(view);
                self.status = BrowserStatus::Starting;
            }
            Err(e) => {
                self.cloud = Some(old);
                self.status = BrowserStatus::Error { message: e.to_string() };
            }
        }
    }
}

#[cfg(test)]
mod tests;
