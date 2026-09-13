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
    /// # Errors
    /// Returns a stable explanation when a numeric option is outside the engine contract.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(MIN_VIDEO_QUALITY..=MAX_VIDEO_QUALITY).contains(&self.quality) {
            return Err("video quality must be between 1 and 100");
        }
        if self.compression_level > MAX_VIDEO_COMPRESSION_LEVEL {
            return Err("video compression level must be between 0 and 10");
        }
        if !(MIN_VIDEO_FPS..=MAX_VIDEO_FPS).contains(&self.fps) {
            return Err("video fps must be between 1 and 30");
        }
        if !(MIN_VIDEO_MAX_WIDTH..=MAX_VIDEO_MAX_WIDTH).contains(&self.max_width) {
            return Err("video max width must be between 320 and 1920");
        }
        if !(1..=MAX_VIDEO_FILE_BYTES).contains(&self.max_file_bytes) {
            return Err("video capture file limit is outside the supported range");
        }
        Ok(())
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
    }
}
