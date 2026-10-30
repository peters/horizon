//! Public video-capture values and bounded option validation.

use serde::{Deserialize, Serialize};

pub const DEFAULT_VIDEO_QUALITY: u32 = 90;
pub const DEFAULT_VIDEO_COMPRESSION_LEVEL: u32 = 4;
pub const DEFAULT_VIDEO_FPS: u32 = 10;
pub const DEFAULT_VIDEO_MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub const MIN_VIDEO_QUALITY: u32 = 1;
pub const MAX_VIDEO_QUALITY: u32 = 100;
pub const MAX_VIDEO_COMPRESSION_LEVEL: u32 = 10;
pub const MIN_VIDEO_FPS: u32 = 1;
pub const MAX_VIDEO_FPS: u32 = 30;
pub const MIN_VIDEO_MAX_WIDTH: u32 = 320;
pub const MAX_VIDEO_MAX_WIDTH: u32 = 1920;
pub const MIN_VIDEO_FILE_BYTES: u64 = 4 * 1024;
pub const MAX_VIDEO_FILE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserVideoOperation {
    Start,
    Pause,
    Resume,
    Status,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserVideoState {
    Recording,
    Paused,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct BrowserVideoCaptureOptions {
    pub quality: u32,
    pub compression_level: u32,
    pub fps: u32,
    /// Maximum encoded longest side (320-1920). `None` uses source-frame
    /// dimensions with codec-block alignment, bounded by a 3840-pixel longest
    /// side and 8,294,400 pixels; larger frames are downscaled proportionally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,
    pub max_file_bytes: u64,
}

impl Default for BrowserVideoCaptureOptions {
    fn default() -> Self {
        Self {
            quality: DEFAULT_VIDEO_QUALITY,
            compression_level: DEFAULT_VIDEO_COMPRESSION_LEVEL,
            fps: DEFAULT_VIDEO_FPS,
            max_width: None,
            max_file_bytes: DEFAULT_VIDEO_MAX_FILE_BYTES,
        }
    }
}

impl BrowserVideoCaptureOptions {
    fn field_errors(
        quality: u32,
        compression_level: u32,
        fps: u32,
        max_width: Option<u32>,
        max_file_bytes: u64,
    ) -> Result<(), &'static str> {
        if !(MIN_VIDEO_QUALITY..=MAX_VIDEO_QUALITY).contains(&quality) {
            return Err("video quality must be between 1 and 100");
        }
        if compression_level > MAX_VIDEO_COMPRESSION_LEVEL {
            return Err("video compression level must be between 0 and 10");
        }
        if !(MIN_VIDEO_FPS..=MAX_VIDEO_FPS).contains(&fps) {
            return Err("video fps must be between 1 and 30");
        }
        if let Some(max_width) = max_width
            && !(MIN_VIDEO_MAX_WIDTH..=MAX_VIDEO_MAX_WIDTH).contains(&max_width)
        {
            return Err("video max width must be between 320 and 1920");
        }
        if !(MIN_VIDEO_FILE_BYTES..=MAX_VIDEO_FILE_BYTES).contains(&max_file_bytes) {
            return Err("video capture file limit is outside the supported range");
        }
        Ok(())
    }

    /// # Errors
    /// Returns a stable explanation when a numeric option is outside the engine contract.
    pub fn validate(&self) -> Result<(), &'static str> {
        Self::field_errors(
            self.quality,
            self.compression_level,
            self.fps,
            self.max_width,
            self.max_file_bytes,
        )
    }
}

/// Start-only overrides. Omitted fields keep the host defaults, where
/// `max_width: None` means source-frame sizing subject to automatic safety limits.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct BrowserVideoCaptureOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression_level: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_bytes: Option<u64>,
}

impl BrowserVideoCaptureOverrides {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.quality.is_none()
            && self.compression_level.is_none()
            && self.fps.is_none()
            && self.max_width.is_none()
            && self.max_file_bytes.is_none()
    }

    /// # Errors
    /// Returns a stable explanation when a provided override is outside the engine contract.
    pub fn validate(&self) -> Result<(), &'static str> {
        BrowserVideoCaptureOptions::field_errors(
            self.quality.unwrap_or(DEFAULT_VIDEO_QUALITY),
            self.compression_level.unwrap_or(DEFAULT_VIDEO_COMPRESSION_LEVEL),
            self.fps.unwrap_or(DEFAULT_VIDEO_FPS),
            self.max_width,
            self.max_file_bytes.unwrap_or(DEFAULT_VIDEO_MAX_FILE_BYTES),
        )
    }

    #[must_use]
    pub fn apply_to(&self, mut base: BrowserVideoCaptureOptions) -> BrowserVideoCaptureOptions {
        if let Some(quality) = self.quality {
            base.quality = quality;
        }
        if let Some(compression_level) = self.compression_level {
            base.compression_level = compression_level;
        }
        if let Some(fps) = self.fps {
            base.fps = fps;
        }
        if let Some(max_width) = self.max_width {
            base.max_width = Some(max_width);
        }
        if let Some(max_file_bytes) = self.max_file_bytes {
            base.max_file_bytes = max_file_bytes;
        }
        base
    }
}

/// Typed sample coverage of a recording, graded from the encoder's counters.
///
/// The host grades the frames it wrote against the distinct source frames it
/// could have written: the source's own rate, capped by the requested `fps`. A
/// source that changes rarely therefore grades as `Good` even though it has a
/// low `effective_fps`, because no source frame was replaced before it was
/// written. This grades sample coverage, not image quality and not whether the
/// content in the file shows the event a caller wanted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserVideoQuality {
    /// The host has not written a frame yet, so it cannot grade the capture.
    #[default]
    Unknown,
    /// The encoder wrote at least four of every five frames it could write.
    Good,
    /// The encoder wrote two to four of every five frames it could write, so a
    /// short source state can be absent.
    Degraded,
    /// The encoder wrote fewer than two of every five frames it could write.
    Poor,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct BrowserVideoCapture {
    pub capture_id: String,
    pub path: String,
    pub state: BrowserVideoState,
    pub active: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Frames written per active recording second. Unchanged source frames are
    /// held by their timestamp instead of being written again, so a source that
    /// changes rarely reports a low rate and still grades [`BrowserVideoQuality::Good`].
    #[serde(default)]
    pub effective_fps: f64,
    /// Typed sample coverage, graded by [`Self::update_performance`].
    #[serde(default)]
    pub quality: BrowserVideoQuality,
    /// Frames the encoder accepted for the file. A running capture counts them
    /// before the encoder returns the packet, so a held source still counts.
    pub frames_encoded: u64,
    /// Scheduled samples the host could not attempt because it was still
    /// encoding a previous frame.
    pub frames_dropped: u64,
    /// Scheduled samples whose source pixels had not changed since the last
    /// written frame, so no frame was written for them.
    pub frames_repeated: u64,
    /// Distinct source frames the encoder observed, including the frames it
    /// replaced before it could write them.
    #[serde(default)]
    pub frames_offered: u64,
    pub bytes_written: u64,
    pub file_limit_reached: bool,
    pub encoder_failed: bool,
    pub started_at_millis: i64,
    pub elapsed_millis: u64,
}

impl BrowserVideoCapture {
    /// Fill the fields the host derives from the raw counters.
    pub fn update_performance(&mut self) {
        let milli_fps = self
            .frames_encoded
            .saturating_mul(1_000_000)
            .checked_div(self.elapsed_millis)
            .unwrap_or(0);
        self.effective_fps = f64::from(u32::try_from(milli_fps).unwrap_or(u32::MAX)) / 1_000.0;
        self.quality = self.grade();
    }

    /// Grade written frames against the distinct source frames that were
    /// available, bounded by the requested sample rate.
    fn grade(&self) -> BrowserVideoQuality {
        if self.elapsed_millis == 0 || self.frames_offered == 0 {
            return BrowserVideoQuality::Unknown;
        }
        let scheduled = u64::from(self.fps.max(1))
            .saturating_mul(self.elapsed_millis)
            .saturating_div(1_000);
        // The requested rate is a ceiling, never a requirement: a source that
        // changes fewer times than the target has nothing more to write.
        let expected = scheduled.min(self.frames_offered).max(1);
        if self.frames_encoded.saturating_mul(10) >= expected.saturating_mul(8) {
            BrowserVideoQuality::Good
        } else if self.frames_encoded.saturating_mul(10) >= expected.saturating_mul(4) {
            BrowserVideoQuality::Degraded
        } else {
            BrowserVideoQuality::Poor
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_merge_onto_host_defaults() {
        let overlay = BrowserVideoCaptureOverrides {
            fps: Some(5),
            ..BrowserVideoCaptureOverrides::default()
        };
        let merged = overlay.apply_to(BrowserVideoCaptureOptions::default());
        assert_eq!(merged.fps, 5);
        assert_eq!(merged.quality, DEFAULT_VIDEO_QUALITY);
        assert!(overlay.validate().is_ok());
    }

    #[test]
    fn default_options_are_valid_and_record_at_viewport_width() {
        let options = BrowserVideoCaptureOptions::default();
        assert!(options.max_width.is_none());
        assert_eq!(options.quality, DEFAULT_VIDEO_QUALITY);
        assert!(options.validate().is_ok());
    }

    #[test]
    fn explicit_max_width_keeps_the_engine_range_contract() {
        assert!(
            BrowserVideoCaptureOptions {
                max_width: Some(319),
                ..BrowserVideoCaptureOptions::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            BrowserVideoCaptureOptions {
                max_width: Some(1921),
                ..BrowserVideoCaptureOptions::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            BrowserVideoCaptureOptions {
                max_width: Some(320),
                ..BrowserVideoCaptureOptions::default()
            }
            .validate()
            .is_ok()
        );
        assert!(
            BrowserVideoCaptureOptions {
                max_width: Some(1920),
                ..BrowserVideoCaptureOptions::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn overrides_validate_only_provided_fields_and_keep_an_omitted_width() {
        assert!(
            BrowserVideoCaptureOverrides {
                max_width: Some(16),
                ..BrowserVideoCaptureOverrides::default()
            }
            .validate()
            .is_err()
        );
        let overlay = BrowserVideoCaptureOverrides {
            quality: Some(DEFAULT_VIDEO_QUALITY),
            ..BrowserVideoCaptureOverrides::default()
        };
        assert!(overlay.validate().is_ok());
        let merged = overlay.apply_to(BrowserVideoCaptureOptions {
            max_width: Some(1280),
            ..BrowserVideoCaptureOptions::default()
        });
        assert_eq!(merged.max_width, Some(1280));
        assert_eq!(merged.quality, DEFAULT_VIDEO_QUALITY);
    }

    #[test]
    fn options_reject_out_of_range_values() {
        assert!(
            BrowserVideoCaptureOptions {
                quality: 0,
                ..BrowserVideoCaptureOptions::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            BrowserVideoCaptureOptions {
                compression_level: 11,
                ..BrowserVideoCaptureOptions::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            BrowserVideoCaptureOptions {
                fps: 31,
                ..BrowserVideoCaptureOptions::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            BrowserVideoCaptureOptions {
                max_width: Some(16),
                ..BrowserVideoCaptureOptions::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            BrowserVideoCaptureOptions {
                max_file_bytes: 0,
                ..BrowserVideoCaptureOptions::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            BrowserVideoCaptureOptions {
                max_file_bytes: 1,
                ..BrowserVideoCaptureOptions::default()
            }
            .validate()
            .is_err()
        );
    }

    fn capture(fps: u32, elapsed_millis: u64, encoded: u64, offered: u64) -> BrowserVideoCapture {
        BrowserVideoCapture {
            capture_id: "capture".to_string(),
            path: "/tmp/capture.webm".to_string(),
            state: BrowserVideoState::Recording,
            active: true,
            width: 640,
            height: 360,
            fps,
            effective_fps: 0.0,
            quality: BrowserVideoQuality::Unknown,
            frames_encoded: encoded,
            frames_dropped: 0,
            frames_repeated: 0,
            frames_offered: offered,
            bytes_written: 0,
            file_limit_reached: false,
            encoder_failed: false,
            started_at_millis: 0,
            elapsed_millis,
        }
    }

    fn grade(mut capture: BrowserVideoCapture) -> BrowserVideoQuality {
        capture.update_performance();
        capture.quality
    }

    #[test]
    fn a_capture_without_a_frame_is_unknown() {
        assert_eq!(grade(capture(5, 0, 0, 0)), BrowserVideoQuality::Unknown);
        assert_eq!(grade(capture(5, 5_000, 0, 0)), BrowserVideoQuality::Unknown);
    }

    #[test]
    fn a_source_that_changes_rarely_grades_good_at_its_own_rate() {
        // 60 s at 5 fps allows 300 samples, but the source changed 12 times and
        // every change was written.
        assert_eq!(grade(capture(5, 60_000, 12, 12)), BrowserVideoQuality::Good);
    }

    #[test]
    fn written_frames_are_graded_against_the_frames_that_were_available() {
        assert_eq!(grade(capture(10, 10_000, 90, 100)), BrowserVideoQuality::Good);
        assert_eq!(grade(capture(10, 10_000, 80, 100)), BrowserVideoQuality::Good);
        assert_eq!(grade(capture(10, 10_000, 79, 100)), BrowserVideoQuality::Degraded);
        assert_eq!(grade(capture(10, 10_000, 40, 100)), BrowserVideoQuality::Degraded);
        assert_eq!(grade(capture(10, 10_000, 39, 100)), BrowserVideoQuality::Poor);
        assert_eq!(grade(capture(10, 10_000, 5, 100)), BrowserVideoQuality::Poor);
    }

    #[test]
    fn the_requested_rate_caps_expected_frames_and_grades_a_fast_source_good() {
        // A source that offers 3000 frames in 10 s at a requested 5 fps is
        // deliberately sampled down; that is not a capture defect.
        assert_eq!(grade(capture(5, 10_000, 50, 3_000)), BrowserVideoQuality::Good);
    }

    #[test]
    fn update_performance_reports_rate_and_quality_together() {
        let mut capture = capture(10, 10_000, 50, 100);
        capture.update_performance();
        assert!((capture.effective_fps - 5.0).abs() < f64::EPSILON);
        assert_eq!(capture.quality, BrowserVideoQuality::Degraded);
    }
}
