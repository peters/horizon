use horizon_browser::{
    BrowserControlAction, BrowserVideoCapture, BrowserVideoCaptureOverrides, BrowserVideoOperation,
    BrowserVideoQuality, BrowserVideoState,
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
    /// Longest encoded side in pixels (320-1920). Start only; omit to keep
    /// the host setting, which defaults to source-frame sizing with codec alignment,
    /// bounded by a 3840-pixel longest side and 8,294,400 pixels.
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
    /// Frames written per active recording second. Unchanged source frames are
    /// held by their timestamp instead of being written again.
    effective_fps: f64,
    /// Typed sample coverage. `good` writes at least four of every five frames
    /// the source offered up to the requested rate; `degraded` and `poor` mean
    /// the host fell behind and brief states can be absent.
    quality: String,
    frames_encoded: u64,
    frames_dropped: u64,
    frames_repeated: u64,
    frames_offered: u64,
    bytes_written: u64,
    file_limit_reached: bool,
    encoder_failed: bool,
    started_at_millis: i64,
    elapsed_millis: u64,
    next_step: String,
}

impl VideoOutput {
    pub(crate) fn new(panel_id: String, action_id: String, mut capture: BrowserVideoCapture) -> Self {
        capture.update_performance();
        let next_step = next_step(&capture);
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
            effective_fps: capture.effective_fps,
            quality: match capture.quality {
                BrowserVideoQuality::Unknown => "unknown",
                BrowserVideoQuality::Good => "good",
                BrowserVideoQuality::Degraded => "degraded",
                BrowserVideoQuality::Poor => "poor",
            }
            .to_string(),
            frames_encoded: capture.frames_encoded,
            frames_dropped: capture.frames_dropped,
            frames_repeated: capture.frames_repeated,
            frames_offered: capture.frames_offered,
            bytes_written: capture.bytes_written,
            file_limit_reached: capture.file_limit_reached,
            encoder_failed: capture.encoder_failed,
            started_at_millis: capture.started_at_millis,
            elapsed_millis: capture.elapsed_millis,
            next_step,
        }
    }
}

/// Tell the caller what a typed capture quality means for this export.
fn next_step(capture: &BrowserVideoCapture) -> String {
    if capture.encoder_failed {
        return "The encoder failed; this WebM may be incomplete and should not be treated as a successful export."
            .to_string();
    }
    match capture.quality {
        BrowserVideoQuality::Poor => format!(
            "The host kept fewer than two of every five source frames: it wrote {:.2} fps against the {} fps you requested, so brief page states can be absent. Lower `fps`, `max_width` or `quality` for the next recording, and inspect this export before relying on it.",
            capture.effective_fps, capture.fps
        ),
        BrowserVideoQuality::Degraded => format!(
            "The host fell behind and wrote {:.2} fps against the {} fps you requested, so a short page state can be absent. Inspect this export before relying on it.",
            capture.effective_fps, capture.fps
        ),
        BrowserVideoQuality::Unknown | BrowserVideoQuality::Good => match capture.state {
            BrowserVideoState::Recording => {
                "Call browser_video pause or stop; inspect status for elapsed time and file size.".to_string()
            }
            BrowserVideoState::Paused => {
                "Call browser_video resume to continue or stop to finalize this WebM.".to_string()
            }
            BrowserVideoState::Stopped => {
                "The WebM is finalized; inspect this exact path with a player or read-only tool.".to_string()
            }
        },
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

    fn capture(fps: u32, elapsed_millis: u64, encoded: u64, dropped: u64, offered: u64) -> BrowserVideoCapture {
        BrowserVideoCapture {
            capture_id: "capture".to_string(),
            path: "/tmp/failed.webm".to_string(),
            state: BrowserVideoState::Stopped,
            active: false,
            width: 16,
            height: 16,
            fps,
            effective_fps: 0.0,
            quality: BrowserVideoQuality::Unknown,
            frames_encoded: encoded,
            frames_dropped: dropped,
            frames_repeated: 0,
            frames_offered: offered,
            bytes_written: 12,
            file_limit_reached: false,
            encoder_failed: false,
            started_at_millis: 0,
            elapsed_millis,
        }
    }

    #[test]
    fn failed_stop_does_not_present_the_webm_as_finalized() {
        let mut capture = capture(5, 0, 0, 0, 0);
        capture.encoder_failed = true;
        let output = VideoOutput::new("panel".to_string(), "action".to_string(), capture);
        assert!(output.encoder_failed);
        assert!(output.next_step.contains("incomplete"));
    }

    #[test]
    fn older_host_counters_report_the_written_rate_without_a_grade() {
        // An older host sends no `frames_offered`, so it cannot grade coverage.
        // Its written rate and dropped samples stay available.
        let capture = capture(5, 10_000, 20, 30, 0);
        let output = VideoOutput::new("panel".to_string(), "action".to_string(), capture);
        assert!((output.effective_fps - 2.0).abs() < f64::EPSILON);
        assert_eq!(output.quality, "unknown");
        assert_eq!(output.frames_dropped, 30);
        assert!(output.next_step.contains("finalized"));
    }

    #[test]
    fn a_source_that_changes_rarely_is_not_graded_poor() {
        // 600 s at 5 fps offers 3000 samples, but the source only changed 120
        // times and every change was written.
        let capture = capture(5, 600_000, 120, 0, 120);
        let output = VideoOutput::new("panel".to_string(), "action".to_string(), capture);
        assert_eq!(output.quality, "good");
        assert!(output.next_step.contains("finalized"));
    }

    #[test]
    fn a_degraded_capture_warns_that_a_state_can_be_absent() {
        let capture = capture(10, 10_000, 45, 55, 100);
        let output = VideoOutput::new("panel".to_string(), "action".to_string(), capture);
        assert_eq!(output.quality, "degraded");
        assert!(output.next_step.contains("a short page state can be absent"));
    }
}
