use crate::{
    EncoderSelection, Error, PairedReceiver, Pairing, PairingStore, Result, VideoFormat, cancellation::Cancellation,
    encoder,
};
use std::{
    net::SocketAddr,
    process::Child,
    sync::atomic::{AtomicBool, Ordering},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

mod progress;
pub(crate) use progress::Progress;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CastStatus {
    Connecting,
    PinRequired,
    Starting,
    Streaming { frames: u64 },
    Stopping,
    Stopped,
    Failed(String),
}

/// Bounded background pairing/encoder/transport worker for one receiver.
pub struct CastSession {
    status: Arc<Mutex<Progress>>,
    cancellation: Arc<Cancellation>,
    pin: SyncSender<Zeroizing<String>>,
    frames: SyncSender<encoder::Frame>,
    capture_paused: Arc<AtomicBool>,
    process: Arc<Mutex<Option<Child>>>,
    worker: Option<JoinHandle<()>>,
    format: VideoFormat,
    pairing_saved: Arc<AtomicBool>,
    encoding: Arc<Mutex<Option<EncoderSelection>>>,
}
impl CastSession {
    /// Start pairing off the UI thread. Status becomes `PinRequired` when the TV displays a code.
    /// # Errors
    /// Returns an error if the worker cannot start.
    pub fn start(address: SocketAddr, format: VideoFormat) -> Result<Self> {
        Self::start_worker(address, format, None)
    }
    /// Reuse a saved pairing, or request one PIN and save the authenticated identity.
    /// # Errors
    /// Returns an error if the worker cannot start. Store and verification failures appear in status.
    pub fn start_remembered(address: SocketAddr, format: VideoFormat, store: PairingStore) -> Result<Self> {
        Self::start_worker(address, format, Some(store))
    }
    fn start_worker(address: SocketAddr, format: VideoFormat, store: Option<PairingStore>) -> Result<Self> {
        let status = Arc::new(Mutex::new(Progress::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let cancellation = Arc::new(Cancellation::new(stop.clone()));
        let authentication = cancellation.clone();
        let process = Arc::new(Mutex::new(None));
        let (pin_tx, pin_rx) = mpsc::sync_channel(1);
        let (frames_tx, frames_rx) = mpsc::sync_channel(1);
        let capture_paused = Arc::new(AtomicBool::new(false));
        let frames = encoder::FrameInput::new(frames_rx, capture_paused.clone());
        let state = status.clone();
        let cancel = stop.clone();
        let child = process.clone();
        let pairing_saved = Arc::new(AtomicBool::new(false));
        let saved = pairing_saved.clone();
        let encoding = Arc::new(Mutex::new(None));
        let selection = encoding.clone();
        let worker = thread::Builder::new().name("horizon-cast".into()).spawn(move || {
            let result = (|| {
                // Keep the cross-process receiver lease through authentication, video and teardown.
                let _reservation = store.as_ref().map(PairingStore::reserve).transpose()?;
                authenticate(address, &pin_rx, &state, &authentication, store.as_ref(), &saved).and_then(|receiver| {
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    let selected = encoder::select(format, &cancel, &child);
                    let backend = selected.backend;
                    *lock(&selection) = Some(selected);
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    let mirror = receiver.mirror(format)?;
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    encoder::stream(mirror, format, frames, &state, &cancel, &child, backend)
                })
            })();
            if let Some(mut process) = lock(&child).take() {
                let _ = process.kill();
                let _ = process.wait();
            }
            lock(&state).state = match result {
                Err(error) if !cancel.load(Ordering::Relaxed) => CastStatus::Failed(error.to_string()),
                _ => CastStatus::Stopped,
            };
        })?;
        Ok(Self {
            status,
            cancellation,
            pin: pin_tx,
            frames: frames_tx,
            capture_paused,
            process,
            worker: Some(worker),
            format,
            pairing_saved,
            encoding,
        })
    }
    #[must_use]
    pub fn status(&self) -> CastStatus {
        lock(&self.status).state.clone()
    }
    /// Cumulative successfully transmitted frames, retained after stopping or failure.
    #[must_use]
    pub fn frames_sent(&self) -> u64 {
        lock(&self.status).frames_sent()
    }
    /// Selected encoder and any pre-stream hardware fallback.
    #[must_use]
    pub fn encoding(&self) -> Option<EncoderSelection> {
        lock(&self.encoding).clone()
    }
    /// Whether the selected backend scales validated source crops on the GPU.
    #[must_use]
    pub fn uses_source_frames(&self) -> bool {
        lock(&self.encoding)
            .as_ref()
            .is_some_and(|selection| selection.backend.source_frames())
    }
    /// Whether a raw crop can fit the output canvas with nonzero even CUDA dimensions.
    /// Hosts may letterbox unsupported crops before submitting the fixed canvas.
    #[must_use]
    pub const fn supports_source_dimensions(width: usize, height: usize, canvas: (usize, usize)) -> bool {
        encoder::Frame::supports_source_dimensions(width, height)
            && canvas.0 > 0
            && canvas.1 > 0
            && width.saturating_mul(canvas.1) / height >= 2
            && height.saturating_mul(canvas.0) / width >= 2
    }
    /// Whether this worker saved a new pairing since the host last checked.
    #[must_use]
    pub fn take_pairing_saved(&self) -> bool {
        self.pairing_saved.swap(false, Ordering::Relaxed)
    }
    /// Submit the user's current onscreen code. The PIN is never persisted.
    /// # Errors
    /// Rejects codes outside the pending pairing state or an unavailable worker.
    pub fn pair(&self, pin: Zeroizing<String>) -> Result<()> {
        if pin.len() != 4 || !pin.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Error::Protocol("enter the four-digit code shown on the TV"));
        }
        let mut status = lock(&self.status);
        if status.state != CastStatus::PinRequired {
            return Err(Error::Protocol("not waiting for a PIN"));
        }
        self.pin
            .try_send(pin)
            .map_err(|_| Error::Protocol("pairing worker unavailable"))?;
        status.state = CastStatus::Starting;
        Ok(())
    }
    /// Freeze the last validated frame while host controls cover the capture surface.
    pub fn set_capture_paused(&self, paused: bool) {
        self.capture_paused.store(paused, Ordering::Relaxed);
    }
    /// Submit one fixed-size RGBA frame. A full queue drops the new frame rather than adding latency.
    /// # Errors
    /// Rejects a frame whose dimensions do not match the selected format.
    pub fn submit(&self, rgba: Vec<u8>) -> Result<()> {
        let (width, height) = self.format.dimensions();
        self.submit_source(width, height, rgba)
    }
    /// Submit a validated crop to the selected source-scaling backend, or a fixed canvas to CPU scaling.
    /// # Errors
    /// Rejects malformed/unbounded crops and source-sized frames when the selected backend cannot scale them.
    pub fn submit_source(&self, width: u16, height: u16, rgba: Vec<u8>) -> Result<()> {
        let (output_width, output_height) = self.format.dimensions();
        let source_frames = self.uses_source_frames();
        if !source_frames && (width, height) != (output_width, output_height) {
            return Err(Error::Protocol("invalid RGBA frame size"));
        }
        if source_frames
            && !Self::supports_source_dimensions(
                usize::from(width),
                usize::from(height),
                (usize::from(output_width), usize::from(output_height)),
            )
        {
            return Err(Error::Protocol("source dimensions cannot fit the selected canvas"));
        }
        let frame = encoder::Frame::new(width, height, rgba)?;
        if self.capture_paused.load(Ordering::Relaxed) {
            return Ok(());
        }
        match self.frames.try_send(frame) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(_)) => Err(Error::Protocol("casting worker ended")),
        }
    }
    /// Request cancellation without blocking the UI. The receiver remains reserved until the worker ends.
    pub fn stop(&self) {
        self.cancellation.stop();
        {
            let mut status = lock(&self.status);
            if !matches!(status.state, CastStatus::Stopped | CastStatus::Failed(_)) {
                status.state = CastStatus::Stopping;
            }
        }
        if let Some(child) = lock(&self.process).as_mut() {
            let _ = child.kill();
        }
    }
    /// Release a completed worker handle without losing its final status.
    pub fn reap(&mut self) {
        if self.finished()
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
    }
    #[must_use]
    pub fn finished(&self) -> bool {
        self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }
}
impl Drop for CastSession {
    fn drop(&mut self) {
        self.stop();
        if self.finished()
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
    }
}
pub(crate) fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn authenticate(
    address: SocketAddr,
    pins: &Receiver<Zeroizing<String>>,
    status: &Arc<Mutex<Progress>>,
    cancellation: &Arc<Cancellation>,
    store: Option<&PairingStore>,
    saved: &AtomicBool,
) -> Result<PairedReceiver> {
    if let Some(credentials) = store.map(PairingStore::load).transpose()?.flatten() {
        return PairedReceiver::connect_cancellable(address, credentials, Some(cancellation.clone()));
    }
    let pairing = Pairing::begin_cancellable(address, Some(cancellation.clone()))?;
    lock(status).state = CastStatus::PinRequired;
    let deadline = Instant::now() + Duration::from_secs(180);
    let pin = loop {
        cancellation.check()?;
        if Instant::now() >= deadline {
            return Err(Error::Protocol("pairing code expired; start again"));
        }
        match pins.recv_timeout(Duration::from_millis(100)) {
            Ok(pin) => break pin,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(Error::Protocol("pairing cancelled")),
        }
    };
    let receiver = pairing.finish(pin)?;
    cancellation.check()?;
    if let Some(store) = store {
        store.save(&receiver.credentials)?;
        saved.store(true, Ordering::Relaxed);
    }
    Ok(receiver)
}

#[cfg(test)]
mod tests;
