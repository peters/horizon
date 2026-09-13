//! Backend-neutral, bounded page-pixel `WebM` export.

mod convert;
mod encoder;
mod muxer;

use std::path::Path;
use std::sync::Arc;

pub use encoder::VideoCaptureHandle;
use encoder::{EncoderCommand, EncoderThread};

use crate::frames::FrameSlot;
use crate::{
    BrowserControlFailure, BrowserCoordination, BrowserVideoCapture, BrowserVideoCaptureOptions,
    BrowserVideoCaptureOverrides, BrowserVideoOperation, BrowserVideoState,
};

const MAX_CAPTURE_ID_BYTES: usize = 96;

#[derive(Clone, Copy)]
pub(crate) struct VideoCaptureHost<'a> {
    directory: Option<&'a Path>,
    coordination: Option<&'a dyn BrowserCoordination>,
    panel_local_id: &'a str,
    defaults: &'a BrowserVideoCaptureOptions,
}

impl<'a> VideoCaptureHost<'a> {
    pub(crate) fn new(
        directory: Option<&'a Path>,
        coordination: Option<&'a dyn BrowserCoordination>,
        panel_local_id: &'a str,
        defaults: &'a BrowserVideoCaptureOptions,
    ) -> Self {
        Self {
            directory,
            coordination,
            panel_local_id,
            defaults,
        }
    }
}

#[derive(Debug)]
pub(crate) struct VideoCaptureState {
    handle: Arc<VideoCaptureHandle>,
    active: Option<ActiveVideo>,
    last: Option<BrowserVideoCapture>,
}

#[derive(Debug)]
struct ActiveVideo {
    thread: EncoderThread,
    paused: bool,
}

impl VideoCaptureState {
    pub(crate) fn new(handle: Arc<VideoCaptureHandle>) -> Self {
        Self {
            handle,
            active: None,
            last: None,
        }
    }

    pub(crate) fn start(
        &mut self,
        host: VideoCaptureHost<'_>,
        capture_id: &str,
        frame_slot: Arc<FrameSlot>,
        options: Option<&BrowserVideoCaptureOverrides>,
    ) -> Result<BrowserVideoCapture, BrowserControlFailure> {
        if self.active.is_some() {
            return Err(BrowserControlFailure::new(
                "capture_active",
                "a browser video capture is already active",
            ));
        }
        if capture_id.trim().is_empty()
            || capture_id.len() > MAX_CAPTURE_ID_BYTES
            || capture_id.chars().any(char::is_control)
        {
            return Err(BrowserControlFailure::new(
                "invalid_input",
                "video capture id must be a short printable value",
            ));
        }
        if let Some(overlay) = options {
            overlay
                .validate()
                .map_err(|message| BrowserControlFailure::new("invalid_input", message))?;
        }
        let options = options.map_or_else(
            || host.defaults.clone(),
            |overlay| overlay.apply_to(host.defaults.clone()),
        );
        options
            .validate()
            .map_err(|message| BrowserControlFailure::new("invalid_input", message))?;
        let directory = host.directory.ok_or_else(|| {
            BrowserControlFailure::new(
                "capture_unavailable",
                "the browser host did not configure a capture directory",
            )
        })?;
        if let Some(coordination) = host.coordination {
            coordination
                .prepare_video_capture(host.panel_local_id, directory, options.max_file_bytes)
                .map_err(|error| {
                    BrowserControlFailure::new(
                        "capture_retention",
                        format!("browser video capture retention failed: {error}"),
                    )
                })?;
        }
        let thread = EncoderThread::start(directory, capture_id, frame_slot, options, Arc::clone(&self.handle))
            .map_err(|error| BrowserControlFailure::new("capture_io", error.to_string()))?;
        let capture = thread.snapshot(BrowserVideoState::Recording);
        self.active = Some(ActiveVideo { thread, paused: false });
        Ok(capture)
    }

    fn reconcile(&mut self) {
        if self.active.as_ref().is_some_and(|active| active.thread.is_finished()) {
            let _ = self.stop();
        }
    }

    pub(crate) fn pause(&mut self) -> Result<BrowserVideoCapture, BrowserControlFailure> {
        self.reconcile();
        let active = self.active.as_mut().ok_or_else(|| {
            BrowserControlFailure::new("capture_not_started", "no browser video capture has been started")
        })?;
        active.paused = true;
        active.thread.send(EncoderCommand::Pause);
        Ok(active.thread.snapshot(BrowserVideoState::Paused))
    }

    pub(crate) fn resume(&mut self) -> Result<BrowserVideoCapture, BrowserControlFailure> {
        self.reconcile();
        let active = self.active.as_mut().ok_or_else(|| {
            BrowserControlFailure::new("capture_not_started", "no browser video capture has been started")
        })?;
        active.paused = false;
        active.thread.send(EncoderCommand::Resume);
        Ok(active.thread.snapshot(BrowserVideoState::Recording))
    }

    pub(crate) fn status(&mut self) -> Result<BrowserVideoCapture, BrowserControlFailure> {
        self.reconcile();
        if let Some(active) = self.active.as_ref() {
            let state = if active.paused {
                BrowserVideoState::Paused
            } else {
                BrowserVideoState::Recording
            };
            return Ok(active.thread.snapshot(state));
        }
        self.handle.snapshot().or_else(|| self.last.clone()).ok_or_else(|| {
            BrowserControlFailure::new("capture_not_started", "no browser video capture has been started")
        })
    }

    pub(crate) fn stop(&mut self) -> Result<BrowserVideoCapture, BrowserControlFailure> {
        let Some(active) = self.active.take() else {
            return self.handle.snapshot().or_else(|| self.last.clone()).ok_or_else(|| {
                BrowserControlFailure::new("capture_not_started", "no browser video capture has been started")
            });
        };
        match active.thread.finish() {
            Ok(capture) => {
                self.last = Some(capture.clone());
                Ok(capture)
            }
            Err(error) => {
                let mut capture = self.handle.snapshot().unwrap_or_else(|| BrowserVideoCapture {
                    capture_id: String::new(),
                    path: String::new(),
                    state: BrowserVideoState::Stopped,
                    active: false,
                    width: 0,
                    height: 0,
                    fps: 0,
                    frames_encoded: 0,
                    frames_dropped: 0,
                    frames_repeated: 0,
                    bytes_written: 0,
                    file_limit_reached: false,
                    encoder_failed: true,
                    started_at_millis: 0,
                    elapsed_millis: 0,
                });
                capture.state = BrowserVideoState::Stopped;
                capture.active = false;
                capture.encoder_failed = true;
                self.last = Some(capture.clone());
                Err(BrowserControlFailure::new("capture_io", error.to_string()))
            }
        }
    }

    pub(crate) fn apply(
        &mut self,
        host: VideoCaptureHost<'_>,
        capture_id: &str,
        frame_slot: Arc<FrameSlot>,
        operation: BrowserVideoOperation,
        options: Option<&BrowserVideoCaptureOverrides>,
    ) -> Result<BrowserVideoCapture, BrowserControlFailure> {
        if !matches!(operation, BrowserVideoOperation::Start) && options.is_some() {
            return Err(BrowserControlFailure::new(
                "invalid_input",
                "video pause, resume, status, and stop do not accept capture options",
            ));
        }
        self.reconcile();
        match operation {
            BrowserVideoOperation::Start => self.start(host, capture_id, frame_slot, options),
            BrowserVideoOperation::Pause => self.pause(),
            BrowserVideoOperation::Resume => self.resume(),
            BrowserVideoOperation::Status => self.status(),
            BrowserVideoOperation::Stop => self.stop(),
        }
    }
}

impl Default for VideoCaptureState {
    fn default() -> Self {
        Self::new(Arc::new(VideoCaptureHandle::default()))
    }
}

impl Drop for VideoCaptureState {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn solid_rgb(red: u8, green: u8, blue: u8) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(64 * 64 * 3);
        for _ in 0..(64 * 64) {
            rgb.extend_from_slice(&[red, green, blue]);
        }
        rgb
    }

    #[test]
    fn start_pause_resume_stop_writes_a_webm_file() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let slot = Arc::new(FrameSlot::new());
        slot.store_test_rgb(64, 64, solid_rgb(255, 0, 0));
        let mut state = VideoCaptureState::default();
        let defaults = BrowserVideoCaptureOptions {
            fps: 5,
            compression_level: 0,
            max_file_bytes: 4 * 1024 * 1024,
            ..BrowserVideoCaptureOptions::default()
        };
        let host = VideoCaptureHost::new(Some(root.path()), None, "panel", &defaults);
        let started = state
            .start(host, "capture-1", Arc::clone(&slot), None)
            .unwrap_or_else(|error| panic!("start video: {error:?}"));
        assert!(started.active);
        assert_eq!(started.state, BrowserVideoState::Recording);
        std::thread::sleep(Duration::from_millis(400));
        slot.store_test_rgb(64, 64, solid_rgb(0, 255, 0));
        let paused = state.pause().unwrap_or_else(|error| panic!("pause video: {error:?}"));
        assert_eq!(paused.state, BrowserVideoState::Paused);
        std::thread::sleep(Duration::from_millis(120));
        let resumed = state.resume().unwrap_or_else(|error| panic!("resume video: {error:?}"));
        assert_eq!(resumed.state, BrowserVideoState::Recording);
        slot.store_test_rgb(64, 64, solid_rgb(0, 0, 255));
        std::thread::sleep(Duration::from_millis(400));
        let stopped = state.stop().unwrap_or_else(|error| panic!("stop video: {error:?}"));
        assert!(!stopped.active);
        assert_eq!(stopped.state, BrowserVideoState::Stopped);
        let bytes = std::fs::read(&stopped.path).unwrap_or_else(|error| panic!("read webm: {error}"));
        assert_eq!(&bytes[..4], &[0x1A, 0x45, 0xDF, 0xA3]);
        assert!(bytes.windows(4).any(|window| window == b"webm"));
        assert!(stopped.frames_encoded >= 1);
    }

    #[test]
    fn second_start_while_active_fails() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let slot = Arc::new(FrameSlot::new());
        slot.store_test_rgb(64, 64, solid_rgb(32, 32, 32));
        let mut state = VideoCaptureState::default();
        let defaults = BrowserVideoCaptureOptions {
            fps: 5,
            compression_level: 0,
            ..BrowserVideoCaptureOptions::default()
        };
        let host = VideoCaptureHost::new(Some(root.path()), None, "panel", &defaults);
        state
            .start(host, "one", Arc::clone(&slot), None)
            .unwrap_or_else(|error| panic!("start video: {error:?}"));
        let host = VideoCaptureHost::new(Some(root.path()), None, "panel", &defaults);
        let error = state
            .start(host, "two", slot, None)
            .expect_err("second start must fail");
        assert_eq!(error.code, "capture_active");
        let _ = state.stop();
    }

    #[test]
    fn pause_rejects_start_only_options() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let slot = Arc::new(FrameSlot::new());
        slot.store_test_rgb(64, 64, solid_rgb(32, 32, 32));
        let mut state = VideoCaptureState::default();
        let defaults = BrowserVideoCaptureOptions {
            fps: 5,
            compression_level: 0,
            ..BrowserVideoCaptureOptions::default()
        };
        let host = VideoCaptureHost::new(Some(root.path()), None, "panel", &defaults);
        state
            .start(host, "one", Arc::clone(&slot), None)
            .unwrap_or_else(|error| panic!("start video: {error:?}"));
        let host = VideoCaptureHost::new(Some(root.path()), None, "panel", &defaults);
        let overlay = BrowserVideoCaptureOverrides {
            fps: Some(5),
            ..BrowserVideoCaptureOverrides::default()
        };
        let error = state
            .apply(host, "one", slot, BrowserVideoOperation::Pause, Some(&overlay))
            .expect_err("pause must reject start-only options");
        assert_eq!(error.code, "invalid_input");
        let _ = state.stop();
    }
}
