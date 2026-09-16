//! Typed video refusal for builds without an encoder.

use std::path::Path;
use std::sync::Arc;

use crate::{
    BrowserControlFailure, BrowserCoordination, BrowserVideoCapture, BrowserVideoCaptureOptions,
    BrowserVideoCaptureOverrides, BrowserVideoOperation, FrameSlot,
};

/// Video progress remains empty when the `video-capture` feature is disabled.
#[derive(Debug, Default)]
pub struct VideoCaptureHandle {
    _private: (),
}

impl VideoCaptureHandle {
    #[must_use]
    pub fn snapshot(&self) -> Option<BrowserVideoCapture> {
        None
    }
}

pub(crate) struct VideoCaptureHost;

impl VideoCaptureHost {
    pub(crate) fn new(
        _directory: Option<&Path>,
        _coordination: Option<&dyn BrowserCoordination>,
        _panel_local_id: &str,
        _defaults: &BrowserVideoCaptureOptions,
    ) -> Self {
        Self
    }
}

#[derive(Debug)]
pub(crate) struct VideoCaptureState {
    failure: BrowserControlFailure,
}

impl VideoCaptureState {
    pub(crate) fn new(_handle: Arc<VideoCaptureHandle>) -> Self {
        Self {
            failure: BrowserControlFailure::new(
                "capture_unavailable",
                "this browser engine was built without the video-capture feature",
            ),
        }
    }

    pub(crate) fn apply(
        &mut self,
        _host: VideoCaptureHost,
        _capture_id: &str,
        _frame_slot: Arc<FrameSlot>,
        _operation: BrowserVideoOperation,
        _options: Option<&BrowserVideoCaptureOverrides>,
    ) -> Result<BrowserVideoCapture, BrowserControlFailure> {
        Err(self.failure.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_operations_refuse_without_creating_output() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let handle = Arc::new(VideoCaptureHandle::default());
        let mut state = VideoCaptureState::new(Arc::clone(&handle));
        let frames = Arc::new(FrameSlot::new());
        frames.store_test_rgb(2, 2, vec![128; 12]);
        let defaults = BrowserVideoCaptureOptions::default();
        for operation in [
            BrowserVideoOperation::Start,
            BrowserVideoOperation::Pause,
            BrowserVideoOperation::Resume,
            BrowserVideoOperation::Status,
            BrowserVideoOperation::Stop,
        ] {
            let result = state.apply(
                VideoCaptureHost::new(Some(root.path()), None, "panel", &defaults),
                "capture",
                Arc::clone(&frames),
                operation,
                None,
            );
            let failure = result.unwrap_err();
            assert_eq!(failure.code, "capture_unavailable");
            assert!(handle.snapshot().is_none());
            assert_eq!(frames.latest().unwrap().rgb, vec![128; 12]);
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
