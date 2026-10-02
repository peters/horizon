use crate::{EncoderSelection, Error, PairedReceiver, Pairing, PairingStore, Result, VideoFormat, encoder};
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
    status: Arc<Mutex<CastStatus>>,
    stop: Arc<AtomicBool>,
    pin: SyncSender<Zeroizing<String>>,
    frames: SyncSender<Vec<u8>>,
    capture_paused: Arc<AtomicBool>,
    process: Arc<Mutex<Option<Child>>>,
    worker: Option<JoinHandle<()>>,
    expected_bytes: usize,
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
        let status = Arc::new(Mutex::new(CastStatus::Connecting));
        let stop = Arc::new(AtomicBool::new(false));
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
                authenticate(address, &pin_rx, &state, &cancel, store.as_ref(), &saved).and_then(|receiver| {
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
            *lock(&state) = match result {
                Err(error) if !cancel.load(Ordering::Relaxed) => CastStatus::Failed(error.to_string()),
                _ => CastStatus::Stopped,
            };
        })?;
        let (width, height) = format.dimensions();
        Ok(Self {
            status,
            stop,
            pin: pin_tx,
            frames: frames_tx,
            capture_paused,
            process,
            worker: Some(worker),
            expected_bytes: usize::from(width) * usize::from(height) * 4,
            pairing_saved,
            encoding,
        })
    }
    #[must_use]
    pub fn status(&self) -> CastStatus {
        lock(&self.status).clone()
    }
    /// Selected encoder and any pre-stream hardware fallback.
    #[must_use]
    pub fn encoding(&self) -> Option<EncoderSelection> {
        lock(&self.encoding).clone()
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
        if *status != CastStatus::PinRequired {
            return Err(Error::Protocol("not waiting for a PIN"));
        }
        self.pin
            .try_send(pin)
            .map_err(|_| Error::Protocol("pairing worker unavailable"))?;
        *status = CastStatus::Starting;
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
        if rgba.len() != self.expected_bytes {
            return Err(Error::Protocol("invalid RGBA frame size"));
        }
        if self.capture_paused.load(Ordering::Relaxed) {
            return Ok(());
        }
        match self.frames.try_send(rgba) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(_)) => Err(Error::Protocol("casting worker ended")),
        }
    }
    /// Request cancellation without blocking the UI. The receiver remains reserved until the worker ends.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        {
            let mut status = lock(&self.status);
            if !matches!(*status, CastStatus::Stopped | CastStatus::Failed(_)) {
                *status = CastStatus::Stopping;
            }
        }
        if let Some(child) = lock(&self.process).as_mut() {
            let _ = child.kill();
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
    status: &Arc<Mutex<CastStatus>>,
    stop: &Arc<AtomicBool>,
    store: Option<&PairingStore>,
    saved: &AtomicBool,
) -> Result<PairedReceiver> {
    if let Some(credentials) = store.map(PairingStore::load).transpose()?.flatten() {
        return PairedReceiver::connect(address, credentials);
    }
    let pairing = Pairing::begin(address)?;
    *lock(status) = CastStatus::PinRequired;
    let deadline = Instant::now() + Duration::from_secs(180);
    let pin = loop {
        if stop.load(Ordering::Relaxed) {
            return Err(Error::Protocol("pairing cancelled"));
        }
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
    if let Some(store) = store {
        store.save(&receiver.credentials)?;
        saved.store(true, Ordering::Relaxed);
    }
    Ok(receiver)
}
