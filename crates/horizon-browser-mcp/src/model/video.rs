use horizon_browser::{
    BrowserControlAction, BrowserVideoCapture, BrowserVideoCaptureOverrides, BrowserVideoOperation, BrowserVideoState,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum VideoOperation {
    Start,
    Pause,
    Resume,
    Status,
    Stop,
}

impl From<VideoOperation> for BrowserVideoOperation {
    fn from(value: VideoOperation) -> Self {
        match value {
            VideoOperation::Start => Self::Start,
            VideoOperation::Pause => Self::Pause,
            VideoOperation::Resume => Self::Resume,
            VideoOperation::Status => Self::Status,
            VideoOperation::Stop => Self::Stop,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct VideoInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// Start a new `WebM` recording, pause, resume, inspect, or stop and finalize it.
    operation: VideoOperation,
    /// Visual quality 1-100. Start only; omit to keep the host `browser.video` setting.
    quality: Option<u32>,
    /// Compression effort 0-10 (0 is fastest/largest). Start only; omit to keep the host `browser.video` setting.
    compression_level: Option<u32>,
    /// Target frames per second 1-30. Start only; omit to keep the host `browser.video` setting.
    fps: Option<u32>,
    /// Longest encoded side in pixels 320-1920. Start only; omit to keep the host `browser.video` setting.
    max_width: Option<u32>,
    /// Maximum `WebM` file size in bytes (minimum 4096, maximum 1073741824). Start only; omit to keep the host `browser.video` setting.
    max_file_bytes: Option<u64>,
    /// Per-action timeout in milliseconds (1-60000).
    pub(crate) timeout_millis: Option<u64>,
}

impl VideoInput {
    pub(crate) fn build_action(&self) -> Result<BrowserControlAction, String> {
        let has_options = self.quality.is_some()
            || self.compression_level.is_some()
            || self.fps.is_some()
            || self.max_width.is_some()
            || self.max_file_bytes.is_some();
        if !matches!(self.operation, VideoOperation::Start) && has_options {
            return Err("video pause, resume, status, and stop do not accept capture options".to_string());
        }
        let options = if matches!(self.operation, VideoOperation::Start) && has_options {
            Some(BrowserVideoCaptureOverrides {
                quality: self.quality,
                compression_level: self.compression_level,
                fps: self.fps,
                max_width: self.max_width,
                max_file_bytes: self.max_file_bytes,
            })
        } else {
            None
        };
        Ok(BrowserControlAction::Video {
            operation: self.operation.into(),
            options,
        })
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct VideoOutput {
    panel_id: String,
    action_id: String,
    capture_id: String,
    /// Private local `WebM` export. Agents may use ordinary read-only tools on this exact returned path.
    path: String,
    state: String,
    active: bool,
    width: u32,
    height: u32,
    fps: u32,
    frames_encoded: u64,
    frames_dropped: u64,
    frames_repeated: u64,
    bytes_written: u64,
    file_limit_reached: bool,
    encoder_failed: bool,
    started_at_millis: i64,
    elapsed_millis: u64,
    next_step: String,
}

impl VideoOutput {
    pub(crate) fn new(panel_id: String, action_id: String, capture: BrowserVideoCapture) -> Self {
        Self {
            panel_id,
            action_id,
            capture_id: capture.capture_id,
            path: capture.path.clone(),
            state: match capture.state {
                BrowserVideoState::Recording => "recording",
                BrowserVideoState::Paused => "paused",
                BrowserVideoState::Stopped => "stopped",
            }
            .to_string(),
            active: capture.active,
            width: capture.width,
            height: capture.height,
            fps: capture.fps,
            frames_encoded: capture.frames_encoded,
            frames_dropped: capture.frames_dropped,
            frames_repeated: capture.frames_repeated,
            bytes_written: capture.bytes_written,
            file_limit_reached: capture.file_limit_reached,
            encoder_failed: capture.encoder_failed,
            started_at_millis: capture.started_at_millis,
            elapsed_millis: capture.elapsed_millis,
            next_step: if capture.encoder_failed {
                "The encoder failed; this WebM may be incomplete and should not be treated as a successful export."
                    .to_string()
            } else {
                match capture.state {
                    BrowserVideoState::Recording => {
                        "Call browser_video pause or stop; inspect status for elapsed time and file size.".to_string()
                    }
                    BrowserVideoState::Paused => {
                        "Call browser_video resume to continue or stop to finalize this WebM.".to_string()
                    }
                    BrowserVideoState::Stopped => {
                        "The WebM is finalized; inspect this exact path with a player or read-only tool.".to_string()
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_without_options_does_not_send_protocol_defaults() {
        let input = VideoInput {
            panel_id: "panel".to_string(),
            operation: VideoOperation::Start,
            quality: None,
            compression_level: None,
            fps: None,
            max_width: None,
            max_file_bytes: None,
            timeout_millis: None,
        };
        let BrowserControlAction::Video { options, .. } = input.build_action().expect("start") else {
            panic!("expected video action");
        };
        assert!(options.is_none());
    }

    #[test]
    fn start_options_are_rejected_on_pause() {
        let input = VideoInput {
            panel_id: "panel".to_string(),
            operation: VideoOperation::Pause,
            quality: Some(80),
            compression_level: None,
            fps: None,
            max_width: None,
            max_file_bytes: None,
            timeout_millis: None,
        };
        assert!(input.build_action().is_err());
    }

    #[test]
    fn failed_stop_does_not_present_the_webm_as_finalized() {
        let capture = BrowserVideoCapture {
            capture_id: "capture".to_string(),
            path: "/tmp/failed.webm".to_string(),
            state: BrowserVideoState::Stopped,
            active: false,
            width: 16,
            height: 16,
            fps: 5,
            frames_encoded: 0,
            frames_dropped: 0,
            frames_repeated: 0,
            bytes_written: 12,
            file_limit_reached: false,
            encoder_failed: true,
            started_at_millis: 0,
            elapsed_millis: 0,
        };
        let output = VideoOutput::new("panel".to_string(), "action".to_string(), capture);
        assert!(output.encoder_failed);
        assert!(output.next_step.contains("incomplete"));
    }
}
