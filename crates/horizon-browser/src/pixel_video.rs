//! Host-owned native pixel recording with the shared bounded encoder.
use crate::{
    BrowserControlFailure, BrowserVideoCapture, BrowserVideoCaptureOptions, BrowserVideoOperation, FrameSlot,
    VideoCaptureHandle, video,
};
use std::path::Path;

/// Bounded `WebM` capture for a host-owned RGB frame source.
/// The caller supplies a private directory and controls source authorization.
pub struct PixelVideoRecorder {
    state: video::VideoCaptureState,
    handle: std::sync::Arc<VideoCaptureHandle>,
    frames: std::sync::Arc<FrameSlot>,
}

impl PixelVideoRecorder {
    /// Start an encoder with the same limits and format as browser capture.
    ///
    /// # Errors
    /// Returns a capture failure if options, storage, or encoder support are unavailable.
    pub fn start(
        directory: &Path,
        capture_id: &str,
        frames: std::sync::Arc<FrameSlot>,
        options: &BrowserVideoCaptureOptions,
    ) -> Result<Self, BrowserControlFailure> {
        let handle = std::sync::Arc::new(VideoCaptureHandle::default());
        let mut state = video::VideoCaptureState::new(std::sync::Arc::clone(&handle));
        state.apply(
            video::VideoCaptureHost::new(Some(directory), None, capture_id, options),
            capture_id,
            std::sync::Arc::clone(&frames),
            BrowserVideoOperation::Start,
            None,
        )?;
        Ok(Self { state, handle, frames })
    }

    /// Progress can be read without waiting for the encoder.
    #[must_use]
    pub fn handle(&self) -> std::sync::Arc<VideoCaptureHandle> {
        std::sync::Arc::clone(&self.handle)
    }

    /// Finish the file before returning its final counters.
    ///
    /// # Errors
    /// Returns a capture failure when the encoder or file finalization fails.
    pub fn stop(&mut self) -> Result<BrowserVideoCapture, BrowserControlFailure> {
        self.state.apply(
            video::VideoCaptureHost::new(None, None, "", &BrowserVideoCaptureOptions::default()),
            "",
            std::sync::Arc::clone(&self.frames),
            BrowserVideoOperation::Stop,
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_rgb_input_rejects_bad_dimensions_without_replacing_pixels() {
        let frames = FrameSlot::new();
        assert!(frames.store_rgb(2, 2, vec![10; 12]).is_some());
        let seq = frames.latest().unwrap().seq;
        for (width, height, bytes) in [(0, 2, 0), (2, 2, 11), (u32::MAX, u32::MAX, 0)] {
            assert!(frames.store_rgb(width, height, vec![0; bytes]).is_none());
            assert_eq!(frames.latest().unwrap().seq, seq);
        }
    }

    #[cfg(not(feature = "video-capture"))]
    #[test]
    fn native_recorder_refuses_without_encoder_and_creates_no_file() {
        let root = tempfile::tempdir().unwrap();
        let frames = std::sync::Arc::new(FrameSlot::new());
        assert!(frames.store_rgb(2, 2, vec![10; 12]).is_some());
        let result = PixelVideoRecorder::start(root.path(), "native", frames, &BrowserVideoCaptureOptions::default());
        assert!(matches!(result, Err(error) if error.code == "capture_unavailable"));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
