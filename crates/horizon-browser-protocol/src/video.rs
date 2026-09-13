//! Public video-capture values and bounded option validation.

use serde::{Deserialize, Serialize};

pub const DEFAULT_VIDEO_QUALITY: u32 = 70;
pub const DEFAULT_VIDEO_COMPRESSION_LEVEL: u32 = 4;
pub const DEFAULT_VIDEO_FPS: u32 = 10;
pub const DEFAULT_VIDEO_MAX_WIDTH: u32 = 1280;
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
    pub max_width: u32,
    pub max_file_bytes: u64,
}

impl Default for BrowserVideoCaptureOptions {
    fn default() -> Self {
        Self {
            quality: DEFAULT_VIDEO_QUALITY,
            compression_level: DEFAULT_VIDEO_COMPRESSION_LEVEL,
            fps: DEFAULT_VIDEO_FPS,
            max_width: DEFAULT_VIDEO_MAX_WIDTH,
            max_file_bytes: DEFAULT_VIDEO_MAX_FILE_BYTES,
        }
    }
}

impl BrowserVideoCaptureOptions {
    fn field_errors(
        quality: u32,
        compression_level: u32,
        fps: u32,
        max_width: u32,
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
        if !(MIN_VIDEO_MAX_WIDTH..=MAX_VIDEO_MAX_WIDTH).contains(&max_width) {
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

/// Start-only overrides. Omitted fields keep the host's `browser.video` defaults.
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
            self.max_width.unwrap_or(DEFAULT_VIDEO_MAX_WIDTH),
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
            base.max_width = max_width;
        }
        if let Some(max_file_bytes) = self.max_file_bytes {
            base.max_file_bytes = max_file_bytes;
        }
        base
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct BrowserVideoCapture {
    pub capture_id: String,
    pub path: String,
    pub state: BrowserVideoState,
    pub active: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames_encoded: u64,
    pub frames_dropped: u64,
    pub frames_repeated: u64,
    pub bytes_written: u64,
    pub file_limit_reached: bool,
    pub encoder_failed: bool,
    pub started_at_millis: i64,
    pub elapsed_millis: u64,
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
    fn default_options_are_valid() {
        assert!(BrowserVideoCaptureOptions::default().validate().is_ok());
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
                max_width: 16,
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
}
