//! An SSH presentation connection. Dropping it never stops the worker browser.
mod transfer;
mod uploads;
use super::{BrowserCommand, BrowserDrainOutput, BrowserEventWaker, BrowserPanelState, BrowserStatus, FrameSlot};
use crate::cloud_runtime::ssh::Connection;
use horizon_browser_protocol::cloud_view::{CloudViewRequest, CloudViewResponse, CloudViewState, MAX_CLOUD_VIEW_BYTES};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, Stdio},
    sync::{
        Arc, Mutex, Weak,
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
    /// The pump thread holds the matching `Arc` until it drops the receiver.
    channel_alive: Weak<()>,
    latest: Latest,
    waker: Waker,
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Child>>,
    handoff_sequence: std::sync::atomic::AtomicU64,
    uploads: uploads::Relay,
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
        let alive = Arc::new(());
        let view = Self {
            connection,
            id,
            initial_url: url,
            target,
            device: None,
            process_lost: false,
            backend,
            tx,
            channel_alive: Arc::downgrade(&alive),
            latest,
            waker,
            stop,
            child,
            handoff_sequence: std::sync::atomic::AtomicU64::new(0),
            uploads: uploads::Relay::default(),
        };
        let (latest, waker, stop, child, outgoing) = (
            view.latest.clone(),
            view.waker.clone(),
            view.stop.clone(),
            view.child.clone(),
            view.uploads.outgoing.clone(),
        );
        let open = CloudViewRequest::Open {
            id: view.id.clone(),
            url: view.initial_url.clone(),
            backend: view.target.is_none().then_some(view.backend),
            target: view.target.clone(),
        };
        thread::spawn(move || {
            let result = pump(input, &responses, &rx, &frames, &latest, &waker, &stop, &outgoing, open);
            // Drop the receiver before publishing a follow-up error so a send
            // cannot succeed after the pump has stopped reading.
            drop(rx);
            drop(alive);
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
        self.can_send() && self.tx.try_send(command).is_ok()
    }
    /// The receiver is still alive. A full queue can still reject one command.
    pub(super) fn can_send(&self) -> bool {
        self.channel_alive.upgrade().is_some()
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
    outgoing: &uploads::SharedOutgoing,
    mut request: CloudViewRequest,
) -> io::Result<()> {
    let id = match &request {
        CloudViewRequest::Open { id, .. } => id.clone(),
        _ => return Ok(()),
    };
    let mut after = 0;
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
        let lost = state.lost;
        if let Some(png) = state.png.take()
            && frames.store_base64_png(&png).is_some()
        {
            after = state.sequence;
        }
        *latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(state);
        wake(waker);
        if lost {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(150));
        request = CloudViewRequest::Poll {
            id: id.clone(),
            after,
            commands: commands.try_iter().take(128).collect(),
            uploads: Some(uploads::next_poll(outgoing)),
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
        self.relay_uploads(state.as_ref());
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
            if state.lost {
                state.backend = self.config.backend;
            }
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
        if state.lost || state.error.is_some() {
            self.reset_orientation_view();
        } else {
            self.apply_orientation_view(state.orientation);
        }
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
    /// Mirrors the worker chooser, sends answered or dropped files and shows worker notices.
    fn relay_uploads(&mut self, state: Option<&CloudViewState>) {
        let chooser = self.frame_slot.file_chooser().clone();
        let Some(CloudView {
            uploads,
            connection,
            id,
            waker,
            ..
        }) = self.cloud.as_mut()
        else {
            return;
        };
        let mut notices = Vec::new();
        notices.extend(uploads.finish(&chooser));
        match state {
            // A lost browser or connection says nothing about upload support. A
            // warning of a live browser still carries its upload state.
            Some(state) if state.uploads.is_none() && (state.lost || state.error.is_some()) => {
                uploads.close(&chooser);
            }
            Some(state) => notices.extend(uploads.apply(state.uploads.clone(), &chooser)),
            None => {}
        }
        uploads.take_answer(&chooser, &|paths, limits, purpose| {
            start_transfer(connection, id, waker, paths, limits, purpose)
        });
        if let Some(notice) = notices.pop() {
            self.navigation_error = Some(notice);
        }
    }
    pub(super) fn drop_cloud_files_now(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        x: f64,
        y: f64,
    ) -> Result<(), String> {
        let CloudView {
            uploads,
            connection,
            id,
            waker,
            target,
            ..
        } = self.cloud.as_mut().ok_or("This browser is not a cloud browser")?;
        if target.is_some() {
            return Err("A remote device of a provider cannot take files from this computer.".into());
        }
        uploads.drop_files(paths, x, y, &|paths, limits, purpose| {
            start_transfer(connection, id, waker, paths, limits, purpose)
        })
    }
    pub(super) fn cloud_drops(&self) -> bool {
        self.cloud.as_ref().is_some_and(|cloud| cloud.uploads.drops())
    }
    pub(super) fn cloud_upload(&self) -> Option<(u64, u64)> {
        self.cloud.as_ref().and_then(|cloud| cloud.uploads.progress())
    }
    pub(super) fn relaunch_cloud(&mut self) {
        if !self.can_retry() {
            return;
        }
        let Some(mut old) = self.cloud.take() else { return };
        // The new connection mirrors the worker request again.
        old.uploads.close(self.frame_slot.file_chooser());
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

/// Sends the files on a new SSH connection, on its own thread.
fn start_transfer(
    connection: &Connection,
    browser: &str,
    waker: &Waker,
    paths: Vec<std::path::PathBuf>,
    limits: horizon_browser_protocol::cloud_view::CloudUploadLimits,
    purpose: uploads::Purpose,
) -> uploads::Transfer {
    let (finished, done) = mpsc::channel();
    let progress = Arc::new(transfer::Progress::default());
    let stop = Arc::new(AtomicBool::new(false));
    let child = uploads::ChildSlot::default();
    let (connection, browser, waker) = (connection.clone(), browser.to_owned(), waker.clone());
    let (sending, stopping, process) = (progress.clone(), stop.clone(), child.clone());
    thread::spawn(move || {
        let result = send_files(&connection, &browser, &paths, limits, &sending, &stopping, &process);
        // The UI thread can lock the slot to stop a transfer, so never wait while holding it.
        let child = process.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = finished.send(result);
        wake(&waker);
    });
    uploads::Transfer {
        purpose,
        progress,
        stop,
        child,
        done,
    }
}
fn send_files(
    connection: &Connection,
    browser: &str,
    paths: &[std::path::PathBuf],
    limits: horizon_browser_protocol::cloud_view::CloudUploadLimits,
    progress: &transfer::Progress,
    stop: &AtomicBool,
    process: &uploads::ChildSlot,
) -> Result<horizon_browser_protocol::cloud_view::CloudStagedFiles, String> {
    let plan = transfer::plan(paths, limits)?;
    // Only a host key that the presentation connection already pinned.
    let mut child = connection
        .pinned_command("horizon-cloud-worker connect")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Cannot connect to the cloud worker: {error}"))?;
    let pipes = (child.stdin.take(), child.stdout.take());
    // From here a dropped transfer kills the process, so a blocked read ends.
    *process.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(child);
    let (Some(input), Some(output)) = pipes else {
        return Err("Cannot connect to the cloud worker".into());
    };
    let mut link = transfer::StreamLink {
        input,
        output: BufReader::new(output),
    };
    let batch = uuid::Uuid::new_v4().simple().to_string();
    transfer::send(&plan, browser, &batch, &mut link, progress, stop)
}

#[cfg(test)]
mod tests;
