//! A bounded native source sampler; encoding and finalization stay off the UI thread.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use horizon_browser::{
    BrowserVideoCaptureOptions, BrowserVideoQuality, FrameSlot, PixelVideoRecorder, VideoCaptureHandle,
};
use horizon_core::browser::manifest::device::{Outcome, VideoAction, VideoRecording};
use horizon_vnc::Frame;

use super::DeviceUiState;

/// A VNC desktop is re-encoded as a whole frame, so the target is the rate a
/// release build sustains at a 1600 × 900 desktop with the default compression
/// (about 6 fps on the reference host). Asking for more only skips samples.
const VNC_FPS: u32 = 5;
const MAX_DURATION: Duration = Duration::from_secs(300);
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILES: usize = 4;

#[derive(Default)]
pub(super) struct Recording {
    worker: Option<Worker>,
    directories: std::collections::VecDeque<Arc<tempfile::TempDir>>,
    error: Option<String>,
    /// The start of a recording that waits for the coordination worker.
    start: Option<PendingVideoStart>,
}

/// The pixels and the source of a recording that waits for its start on the
/// coordination worker. The start takes the manifest lock of the capture, which
/// waits up to 2 s for another writer, and it makes the folder of the video.
pub(crate) struct PendingVideoStart {
    frames: Arc<FrameSlot>,
    source: Box<dyn Fn() -> Option<Arc<Frame>> + Send>,
}

/// A recording that the coordination worker started.
pub(crate) struct VideoStarted {
    directory: Arc<tempfile::TempDir>,
    recorder: PixelVideoRecorder,
    frames: Arc<FrameSlot>,
    source: Box<dyn Fn() -> Option<Arc<Frame>> + Send>,
}

/// Starts the recorder of `pending`. This is the work of the coordination
/// worker: the manifest lock of the capture waits for another writer and makes
/// the folder of the video.
pub(crate) fn start_video(pending: PendingVideoStart) -> Result<VideoStarted, String> {
    horizon_core::ui_thread::blocking("the manifest lock of a video is taken");
    let PendingVideoStart { frames, source } = pending;
    let directory = tempfile::Builder::new()
        .prefix("horizon-vnc-video-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let options = BrowserVideoCaptureOptions {
        fps: VNC_FPS,
        max_file_bytes: MAX_BYTES,
        ..Default::default()
    };
    let recorder = PixelVideoRecorder::start(directory.path(), "vnc", Arc::clone(&frames), &options)
        .map_err(|error| error.message)?;
    Ok(VideoStarted {
        directory: Arc::new(directory),
        recorder,
        frames,
        source,
    })
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.stop();
        // Unlink while the host still runs. Detached encoder threads are not
        // guaranteed a destructor when the whole application exits. A folder is
        // a recursive delete, so it goes to the coordination worker and this
        // drop only records it.
        for directory in self.directories.drain(..) {
            let Ok(directory) = Arc::try_unwrap(directory) else {
                // The sampler still holds it, and removes it when it ends.
                continue;
            };
            crate::app::queue_recording_directory_delete(directory.keep());
        }
    }
}

struct Worker {
    stop: Arc<AtomicBool>,
    handle: Arc<VideoCaptureHandle>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // The sampler owns the active directory until encoder finalization ends.
        // Dropping a panel must not wait for an AV1 encode on the UI thread.
        self.thread.take();
    }
}

impl Recording {
    pub(super) fn stop(&self) {
        if let Some(worker) = &self.worker {
            worker.stop.store(true, Ordering::Release);
        }
    }

    fn status(&self) -> Result<VideoRecording, String> {
        let worker = self.worker.as_ref().ok_or("No VNC recording has been started")?;
        let capture = worker.handle.snapshot().ok_or("No recording progress is available")?;
        let running = worker.thread.as_ref().is_some_and(|thread| !thread.is_finished());
        Ok(VideoRecording {
            finalizing: running && (worker.stop.load(Ordering::Acquire) || !capture.active),
            capture,
        })
    }

    /// Asks for the start of a recording of `first`. The frame keeps the pixels
    /// and the source; the coordination worker starts the encoder.
    fn start(&mut self, first: &Frame, source: impl Fn() -> Option<Arc<Frame>> + Send + 'static) -> Result<(), String> {
        if self.start.is_some()
            || self
                .worker
                .as_ref()
                .is_some_and(|worker| worker.thread.as_ref().is_some_and(|thread| !thread.is_finished()))
        {
            return Err("A VNC recording is already active or finalizing".into());
        }
        let frames = Arc::new(FrameSlot::new());
        if !publish(&frames, first) {
            return Err("The desktop dimensions exceed the recording limit".into());
        }
        self.error = None;
        self.start = Some(PendingVideoStart {
            frames,
            source: Box::new(source),
        });
        Ok(())
    }

    /// Starts the recording now, off the UI thread, for a test that judges the
    /// sampler and not the queue of the coordination worker.
    #[cfg(test)]
    fn start_now(
        &mut self,
        first: &Frame,
        source: impl Fn() -> Option<Arc<Frame>> + Send + 'static,
    ) -> Result<(), String> {
        self.start(first, source)?;
        let pending = self.take_start().expect("a queued start");
        let started = start_video(pending)?;
        self.finish_start(started);
        Ok(())
    }

    /// The start that waits for the coordination worker, so the app queues it
    /// once.
    pub(super) fn take_start(&mut self) -> Option<PendingVideoStart> {
        self.start.take()
    }

    /// Starts sampling with the recorder that the worker started. The sampler
    /// thread is a process of this one; no frame waits for it.
    pub(super) fn finish_start(&mut self, started: VideoStarted) {
        let VideoStarted {
            directory,
            mut recorder,
            frames,
            source,
        } = started;
        let retained_directory = Arc::clone(&directory);
        let handle = recorder.handle();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&stop);
        let progress = Arc::clone(&handle);
        let interval = Duration::from_secs_f64(1.0 / f64::from(VNC_FPS));
        let thread = std::thread::Builder::new()
            .name("vnc-recording".into())
            .spawn(move || {
                let _retained_directory = retained_directory;
                let started = Instant::now();
                let mut previous = None;
                while !signal.load(Ordering::Acquire) && started.elapsed() < MAX_DURATION {
                    if progress.snapshot().is_none_or(|capture| !capture.active) {
                        break;
                    }
                    let Some(image) = source() else {
                        break;
                    };
                    if !publish_changed(&frames, &mut previous, image) {
                        break;
                    }
                    std::thread::sleep(interval);
                }
                if let Err(error) = recorder.stop() {
                    tracing::warn!(code = %error.code, "VNC recording finalization failed");
                }
            })
            .map_err(|error| error.to_string())
            .ok();
        self.worker = Some(Worker { stop, handle, thread });
        self.directories.push_back(directory);
        while self.directories.len() > MAX_FILES {
            self.directories.pop_front();
        }
        self.error = None;
    }
}

fn publish_changed(frames: &FrameSlot, previous: &mut Option<Arc<Frame>>, image: Arc<Frame>) -> bool {
    if previous.as_ref().is_some_and(|last| Arc::ptr_eq(last, &image)) {
        return true;
    }
    if !publish(frames, &image) {
        return false;
    }
    *previous = Some(image);
    true
}

fn publish(frames: &FrameSlot, image: &Frame) -> bool {
    let [width, height] = image.size();
    let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
        return false;
    };
    if u64::from(width) * u64::from(height) > 33_554_432 {
        return false;
    }
    let rgb = image
        .rgba()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, _]| [r, g, b])
        .collect();
    frames.store_rgb(width, height, rgb).is_some()
}

/// A short warning when the encoder could not write most of what the desktop
/// offered. `None` means the counters do not show a gap worth a line.
fn capture_quality_label(status: &VideoRecording) -> Option<(&'static str, egui::Color32)> {
    match status.capture.quality {
        BrowserVideoQuality::Poor => Some(("Recording fell far behind", crate::theme::PALETTE_RED())),
        BrowserVideoQuality::Degraded => Some(("Recording fell behind", crate::theme::PALETTE_YELLOW())),
        BrowserVideoQuality::Unknown | BrowserVideoQuality::Good => None,
    }
}

fn recording_detail(status: &VideoRecording) -> String {
    format!(
        "{:.1} of {} fps written, {} frames held, {} samples missed. A short state can be missing. Save a copy before closing this panel.",
        status.capture.effective_fps, status.capture.fps, status.capture.frames_repeated, status.capture.frames_dropped
    )
}

impl DeviceUiState {
    /// The start of the recording that waits for the coordination worker, so
    /// the app queues it once.
    pub(crate) fn take_video_start(&mut self) -> Option<PendingVideoStart> {
        self.recording.take_start()
    }

    /// Starts the sampler with the recorder that the coordination worker
    /// started, or says why the recording did not start.
    pub(crate) fn finish_video_start(&mut self, result: Result<VideoStarted, String>) {
        match result {
            Ok(started) => self.recording.finish_start(started),
            Err(error) => self.recording.error = Some(error),
        }
    }

    pub(crate) fn video(&mut self, action: VideoAction) -> Outcome {
        let result = match action {
            VideoAction::Start => self.start_recording(),
            VideoAction::Stop => {
                self.recording.stop();
                Ok(())
            }
            VideoAction::Status => Ok(()),
        }
        .and_then(|()| self.recording.status());
        match result {
            Ok(recording) => Outcome::Video { recording },
            Err(message) => Outcome::failed("video_unavailable", &message),
        }
    }

    fn start_recording(&mut self) -> Result<(), String> {
        let first = self.current_desktop()?;
        let session = self.session.as_ref().ok_or("Device viewer is not connected")?;
        self.recording.start(&first, session.recording_source())
    }

    pub(super) fn recording_controls(&mut self, ui: &mut egui::Ui, interactive: bool) {
        let status = self.recording.status().ok();
        if let Some(status) = &status
            && (status.capture.active || status.finalizing)
        {
            if crate::icon_button::icon_button(
                ui,
                interactive && !status.finalizing,
                "Stop recording",
                crate::icon_button::paint_stop,
            )
            .on_hover_text("Stop recording")
            .clicked()
            {
                self.recording.stop();
            }
            ui.label(if status.finalizing {
                "Finishing recording…"
            } else {
                "Recording VNC desktop"
            });
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        } else {
            let can_record = interactive && self.has_current_desktop_frame();
            if crate::icon_button::icon_button(ui, can_record, "Record video", |painter, rect, _color| {
                crate::icon_button::paint_record(painter, rect, can_record);
            })
            .on_hover_text("Record the full desktop, including when hidden. Maximum five minutes or 256 MiB. No audio.")
            .clicked()
            {
                self.recording.error = self.start_recording().err();
            }
            if let Some(status) = &status {
                if status.capture.encoder_failed {
                    ui.colored_label(egui::Color32::RED, "Recording failed");
                } else if let Some((text, color)) = capture_quality_label(status) {
                    ui.colored_label(color, text).on_hover_text(recording_detail(status));
                } else {
                    ui.label(format!("{} frames recorded", status.capture.frames_encoded));
                }
                if crate::icon_button::icon_button(ui, true, "Copy video path", crate::icon_button::paint_clipboard)
                    .on_hover_text(
                        "Copy video path. Private temporary WebM. Save a copy before closing this panel or making four more recordings.",
                    )
                    .clicked()
                {
                    crate::clipboard::copy_text(status.capture.path.clone());
                }
            }
        }
        if let Some(error) = &self.recording.error {
            ui.colored_label(egui::Color32::RED, error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_browser::BrowserVideoState;

    fn frame() -> Frame {
        Frame::from_rgba([64, 64], &[255, 0, 0, 255].repeat(64 * 64)).expect("64 × 64 pixels")
    }

    fn recording(quality: BrowserVideoQuality) -> VideoRecording {
        VideoRecording {
            finalizing: false,
            capture: horizon_browser::BrowserVideoCapture {
                capture_id: "vnc".to_string(),
                path: "/tmp/horizon-vnc-video-1/vnc.webm".to_string(),
                state: BrowserVideoState::Stopped,
                active: false,
                width: 1600,
                height: 900,
                fps: VNC_FPS,
                effective_fps: 2.0,
                quality,
                frames_encoded: 10,
                frames_dropped: 40,
                frames_repeated: 0,
                frames_offered: 50,
                bytes_written: 1024,
                file_limit_reached: false,
                encoder_failed: false,
                started_at_millis: 0,
                elapsed_millis: 10_000,
            },
        }
    }

    #[test]
    fn a_capture_that_kept_under_two_of_five_states_is_a_red_warning() {
        let warning = capture_quality_label(&recording(BrowserVideoQuality::Poor)).expect("warning");
        assert_eq!(warning.0, "Recording fell far behind");
        assert_eq!(warning.1, crate::theme::PALETTE_RED());
        assert!(recording_detail(&recording(BrowserVideoQuality::Poor)).contains("2.0 of 5 fps written"));
    }

    #[test]
    fn a_degraded_capture_is_a_yellow_warning_and_a_good_one_has_none() {
        let warning = capture_quality_label(&recording(BrowserVideoQuality::Degraded)).expect("warning");
        assert_eq!(warning.1, crate::theme::PALETTE_YELLOW());
        assert!(capture_quality_label(&recording(BrowserVideoQuality::Good)).is_none());
        assert!(capture_quality_label(&recording(BrowserVideoQuality::Unknown)).is_none());
    }

    #[test]
    fn unchanged_source_reuses_published_pixels() {
        let frames = FrameSlot::new();
        let image = Arc::new(frame());
        let mut previous = None;
        assert!(publish_changed(&frames, &mut previous, Arc::clone(&image)));
        let first = frames.latest().expect("published");
        for _ in 0..100 {
            assert!(publish_changed(&frames, &mut previous, Arc::clone(&image)));
        }
        assert_eq!(frames.latest().expect("unchanged").seq, first.seq);
        assert!(publish_changed(&frames, &mut previous, Arc::new(frame())));
        assert!(frames.latest().expect("new source").seq > first.seq);
    }

    #[test]
    fn records_and_finalizes_without_ui_frames() {
        let mut recording = Recording::default();
        assert!(recording.status().is_err());
        recording
            .start_now(&frame(), || Some(Arc::new(frame())))
            .expect("start");
        assert!(recording.start_now(&frame(), || Some(Arc::new(frame()))).is_err());
        std::thread::sleep(Duration::from_millis(350));
        recording.stop();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let state = recording.status().expect("status");
            if !state.capture.active && !state.finalizing {
                assert!(!state.capture.encoder_failed);
                assert_eq!(
                    state.capture.fps, VNC_FPS,
                    "the desktop recorder targets the documented rate"
                );
                assert!(state.capture.frames_encoded > 0);
                let path = state.capture.path;
                let bytes = std::fs::read(&path).expect("webm");
                assert_eq!(&bytes[..4], &[0x1a, 0x45, 0xdf, 0xa3]);
                // The encoder reports the resolved folder of the video, and macOS
                // spells one folder below `/var` two ways: the folder that the
                // recording made and the folder that its video names are the same
                // folder, so compare the folders and not their spelling. The
                // folder is read before the drop, because the drop queues it for
                // the coordination worker, which may remove it at once.
                let folder = std::path::Path::new(&path).parent().expect("the folder of the video");
                let folder = std::fs::canonicalize(folder).expect("the folder of the video exists");
                // The drop queues the recursive delete of the folder for the
                // coordination worker, which the exit flushes. The queue is a lock
                // of the process that a frame of another test drains, so the drop
                // and the read of the entry happen inside the gate of that drain.
                let queued = crate::app::folders_that_the_drop_queues(&folder, || drop(recording));
                assert_eq!(
                    queued.len(),
                    1,
                    "the drop queues the folder of the video: {}",
                    folder.display()
                );
                for queued in queued {
                    let _ = std::fs::remove_dir_all(queued);
                }
                assert!(!std::path::Path::new(&path).exists());
                break;
            }
            assert!(Instant::now() < deadline, "encoder did not stop");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn dropping_an_active_recording_unlinks_output_without_waiting_for_the_source() {
        let mut recording = Recording::default();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        recording
            .start_now(&frame(), move || {
                let _ = entered_tx.send(());
                let _ = release_rx.recv_timeout(Duration::from_secs(10));
                None
            })
            .expect("start");
        entered_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("source entered");
        let directory = recording.directories[0].path().to_path_buf();
        assert!(directory.exists());
        // The drop does not wait for the source: the sampler still holds the
        // folder, so this drop queues nothing. Only the entry of this drop is
        // judged, and the gate of the drain keeps a frame of another test from
        // taking a queued folder in the meantime.
        let queued = crate::app::folders_that_the_drop_queues(&directory, || drop(recording));
        assert_eq!(
            queued.len(),
            0,
            "a folder that the sampler still holds is not deleted beside it: {}",
            directory.display()
        );
        let _ = release_tx.send(());
        let deadline = Instant::now() + Duration::from_secs(10);
        while directory.exists() {
            assert!(Instant::now() < deadline, "the folder of the ended recording stays");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn source_loss_stops_recording() {
        let mut recording = Recording::default();
        recording.start_now(&frame(), || None).expect("start");
        let deadline = Instant::now() + Duration::from_secs(20);
        while recording
            .worker
            .as_ref()
            .unwrap()
            .thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!recording.status().unwrap().capture.active);
    }
}
