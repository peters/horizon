//! A bounded native source sampler; encoding and finalization stay off the UI thread.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use egui::ColorImage;
use horizon_browser::{BrowserVideoCaptureOptions, FrameSlot, PixelVideoRecorder, VideoCaptureHandle};
use horizon_core::browser::manifest::device::{Outcome, VideoAction, VideoRecording};

use super::DeviceUiState;

const MAX_DURATION: Duration = Duration::from_secs(300);
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILES: usize = 4;

#[derive(Default)]
pub(super) struct Recording {
    worker: Option<Worker>,
    directories: std::collections::VecDeque<Arc<tempfile::TempDir>>,
    error: Option<String>,
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.stop();
        // Unlink while the host still runs. Detached encoder threads are not
        // guaranteed a destructor when the whole application exits.
        for directory in &self.directories {
            if let Err(error) = std::fs::remove_dir_all(directory.path())
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(%error, "could not remove private VNC recording directory");
            }
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

    fn start(
        &mut self,
        first: &ColorImage,
        source: impl Fn() -> Option<Arc<ColorImage>> + Send + 'static,
    ) -> Result<(), String> {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.thread.as_ref().is_some_and(|thread| !thread.is_finished()))
        {
            return Err("A VNC recording is already active or finalizing".into());
        }
        let directory = tempfile::Builder::new()
            .prefix("horizon-vnc-video-")
            .tempdir()
            .map_err(|error| error.to_string())?;
        let directory = Arc::new(directory);
        let retained_directory = Arc::clone(&directory);
        let frames = Arc::new(FrameSlot::new());
        if !publish(&frames, first) {
            return Err("The desktop dimensions exceed the recording limit".into());
        }
        let options = BrowserVideoCaptureOptions {
            max_file_bytes: MAX_BYTES,
            ..Default::default()
        };
        let mut recorder = PixelVideoRecorder::start(directory.path(), "vnc", Arc::clone(&frames), &options)
            .map_err(|error| error.message)?;
        let handle = recorder.handle();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&stop);
        let progress = Arc::clone(&handle);
        let interval = Duration::from_secs_f64(1.0 / f64::from(options.fps));
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
            .map_err(|error| error.to_string())?;
        self.worker = Some(Worker {
            stop,
            handle,
            thread: Some(thread),
        });
        self.directories.push_back(directory);
        while self.directories.len() > MAX_FILES {
            self.directories.pop_front();
        }
        self.error = None;
        Ok(())
    }
}

fn publish_changed(frames: &FrameSlot, previous: &mut Option<Arc<ColorImage>>, image: Arc<ColorImage>) -> bool {
    if previous.as_ref().is_some_and(|last| Arc::ptr_eq(last, &image)) {
        return true;
    }
    if !publish(frames, &image) {
        return false;
    }
    *previous = Some(image);
    true
}

fn publish(frames: &FrameSlot, image: &ColorImage) -> bool {
    let (Ok(width), Ok(height)) = (u32::try_from(image.size[0]), u32::try_from(image.size[1])) else {
        return false;
    };
    if u64::from(width) * u64::from(height) > 33_554_432 {
        return false;
    }
    let rgb = image
        .pixels
        .iter()
        .flat_map(|pixel| {
            let [r, g, b, _] = pixel.to_array();
            [r, g, b]
        })
        .collect();
    frames.store_rgb(width, height, rgb).is_some()
}

impl DeviceUiState {
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
        let first = self.screenshot_image()?;
        let session = self.session.as_ref().ok_or("Device viewer is not connected")?;
        self.recording.start(&first, session.recording_source())
    }

    pub(super) fn recording_controls(&mut self, ui: &mut egui::Ui, interactive: bool) {
        let status = self.recording.status().ok();
        if let Some(status) = &status
            && (status.capture.active || status.finalizing)
        {
            if ui
                .add_enabled(interactive && !status.finalizing, egui::Button::new("Stop recording"))
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
            if ui
                .add_enabled(
                    interactive && matches!(self.status, super::session::Status::Connected),
                    egui::Button::new("Record video"),
                )
                .on_hover_text(
                    "Record the full desktop, including when hidden. Maximum five minutes or 256 MiB. No audio.",
                )
                .clicked()
            {
                self.recording.error = self.start_recording().err();
            }
            if let Some(status) = &status {
                if status.capture.encoder_failed {
                    ui.colored_label(egui::Color32::RED, "Recording failed");
                } else {
                    ui.label(format!("{} frames recorded", status.capture.frames_encoded));
                }
                if ui
                    .button("Copy video path")
                    .on_hover_text(
                        "Private temporary WebM. Save a copy before closing this panel or making four more recordings.",
                    )
                    .clicked()
                {
                    ui.ctx().copy_text(status.capture.path.clone());
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

    fn frame() -> ColorImage {
        ColorImage::filled([64, 64], egui::Color32::RED)
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
        recording.start(&frame(), || Some(Arc::new(frame()))).expect("start");
        assert!(recording.start(&frame(), || Some(Arc::new(frame()))).is_err());
        std::thread::sleep(Duration::from_millis(350));
        recording.stop();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let state = recording.status().expect("status");
            if !state.capture.active && !state.finalizing {
                assert!(!state.capture.encoder_failed);
                assert!(state.capture.frames_encoded > 0);
                let path = state.capture.path;
                let bytes = std::fs::read(&path).expect("webm");
                assert_eq!(&bytes[..4], &[0x1a, 0x45, 0xdf, 0xa3]);
                drop(recording);
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
            .start(&frame(), move || {
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
        drop(recording);
        let removed = !directory.exists();
        let _ = release_tx.send(());
        assert!(removed, "host shutdown must unlink private output before process exit");
    }

    #[test]
    fn source_loss_stops_recording() {
        let mut recording = Recording::default();
        recording.start(&frame(), || None).expect("start");
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
