//! Decoded screencast frames shared between the driver thread and the UI.
//!
//! The driver thread decodes JPEG frames into a double-buffered slot and
//! signals the UI with a lightweight "frame arrived" event; the UI clones
//! an `Arc` for texture upload. No frame payload ever crosses the mpsc
//! channel.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use base64::Engine;
use zune_jpeg::JpegDecoder;

const MAX_RETIRED_FRAMES: usize = 2;

/// A single decoded frame (RGB8, top-down, tightly packed).
#[derive(Debug, Clone)]
pub struct FrameData {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    /// Monotonic per-panel frame counter.
    pub seq: u64,
}

/// Lock-free counters for one panel's frame and command pipeline.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameMetrics {
    pub frames_received: u64,
    pub frames_acked: u64,
    pub capture_requests: u64,
    pub capture_completions: u64,
    pub capture_failures: u64,
    pub capture_superseded: u64,
    pub encoded_bytes: u64,
    pub decoded_frames: u64,
    pub decode_failures: u64,
    pub unchanged_frames: u64,
    pub published_frames: u64,
    pub wakeups_claimed: u64,
    pub wakeups_coalesced: u64,
    pub commands_coalesced: u64,
    pub commands_rejected: u64,
    pub interaction_frame_samples: u64,
    pub interaction_frame_total_us: u64,
    pub interaction_frame_max_us: u64,
}

#[derive(Debug, Default)]
struct FrameMetricCounters {
    frames_received: AtomicU64,
    frames_acked: AtomicU64,
    capture_requests: AtomicU64,
    capture_completions: AtomicU64,
    capture_failures: AtomicU64,
    capture_superseded: AtomicU64,
    encoded_bytes: AtomicU64,
    decoded_frames: AtomicU64,
    decode_failures: AtomicU64,
    unchanged_frames: AtomicU64,
    published_frames: AtomicU64,
    wakeups_claimed: AtomicU64,
    wakeups_coalesced: AtomicU64,
    commands_coalesced: AtomicU64,
    commands_rejected: AtomicU64,
    interaction_frame_samples: AtomicU64,
    interaction_frame_total_us: AtomicU64,
    interaction_frame_max_us: AtomicU64,
}

impl FrameData {
    #[must_use]
    pub fn byte_size(&self) -> usize {
        self.rgb.len()
    }
}

/// Latest-frame guard payload. Exposed through [`FrameSlot::latest`]; treat
/// as read-only UI data.
#[derive(Clone, Default, Debug)]
pub struct FrameSlotInner {
    data: Option<Arc<FrameData>>,
    /// Reused decode target so steady-state frames do not allocate.
    decode_buffer: Vec<u8>,
    /// Recently replaced frames whose pixels are still borrowed by the UI.
    /// Keeping a bounded set lets a later decode reclaim their allocations
    /// once the UI releases its `Arc`.
    retired_frames: Vec<Arc<FrameData>>,
    next_seq: u64,
}

/// Lock-guarded handoff of the newest decoded frame.
#[derive(Clone, Default, Debug)]
pub struct FrameSlot {
    inner: Arc<std::sync::Mutex<FrameSlotInner>>,
    notification_pending: Arc<AtomicBool>,
    metrics: Arc<FrameMetricCounters>,
    active_backend: Arc<std::sync::Mutex<Option<crate::ActiveBackendCapabilities>>>,
}

impl FrameSlot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode a JPEG frame into the slot and bump `seq`.
    ///
    /// The publication lock is held only while taking a reusable buffer and
    /// swapping the completed frame. JPEG parsing and decoding happen without
    /// the lock, so the render thread never waits on codec work.
    ///
    /// Returns the new sequence number, or `None` on decode failure (the
    /// previous frame is kept).
    #[must_use]
    pub fn store_jpeg(&self, jpeg: &[u8]) -> Option<u64> {
        self.record_encoded_bytes(jpeg.len());
        let result = self.decode_jpeg(jpeg);
        if result.is_none() {
            self.record_decode_failure();
        }
        result
    }

    fn decode_jpeg(&self, jpeg: &[u8]) -> Option<u64> {
        let mut decoder = JpegDecoder::new(std::io::Cursor::new(jpeg));
        let output_size = decoder.output_buffer_size().or_else(|| {
            decoder.decode_headers().ok()?;
            decoder.output_buffer_size()
        })?;
        let mut buffer = {
            let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            take_decode_buffer(&mut inner)
        };
        buffer.resize(output_size, 0);
        if decoder.decode_into(&mut buffer).is_err() {
            self.return_decode_buffer(buffer);
            return None;
        }
        let Some(info) = decoder.info() else {
            self.return_decode_buffer(buffer);
            return None;
        };

        Some(self.publish_rgb(u32::from(info.width), u32::from(info.height), buffer))
    }

    /// Decode a base64-encoded JPEG and publish it as the newest frame.
    #[must_use]
    pub fn store_base64_jpeg(&self, data: &str) -> Option<u64> {
        if let Ok(jpeg) = base64::engine::general_purpose::STANDARD.decode(data) {
            self.store_jpeg(&jpeg)
        } else {
            self.record_decode_failure();
            None
        }
    }

    /// Decode a PNG screenshot and publish it as the newest RGB frame.
    #[must_use]
    pub fn store_png(&self, png_bytes: &[u8]) -> Option<u64> {
        self.record_encoded_bytes(png_bytes.len());
        let result = self.decode_png(png_bytes);
        if result.is_none() {
            self.record_decode_failure();
        }
        result
    }

    fn decode_png(&self, png_bytes: &[u8]) -> Option<u64> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().ok()?;
        let mut decoded_png = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut decoded_png).ok()?;
        let pixels = &decoded_png[..info.buffer_size()];
        let mut rgb = {
            let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            take_decode_buffer(&mut inner)
        };
        rgb.clear();
        let pixel_count = usize::try_from(info.width)
            .ok()?
            .checked_mul(usize::try_from(info.height).ok()?)?;
        rgb.reserve(pixel_count.saturating_mul(3));
        match info.color_type {
            png::ColorType::Rgb => rgb.extend_from_slice(pixels),
            png::ColorType::Rgba => {
                for pixel in pixels.as_chunks::<4>().0 {
                    rgb.extend_from_slice(&pixel[..3]);
                }
            }
            png::ColorType::Grayscale => {
                for value in pixels {
                    rgb.extend_from_slice(&[*value, *value, *value]);
                }
            }
            png::ColorType::GrayscaleAlpha => {
                for pixel in pixels.as_chunks::<2>().0 {
                    rgb.extend_from_slice(&[pixel[0], pixel[0], pixel[0]]);
                }
            }
            png::ColorType::Indexed => {
                self.return_decode_buffer(rgb);
                return None;
            }
        }
        if rgb.len() == pixel_count.saturating_mul(3) {
            Some(self.publish_rgb(info.width, info.height, rgb))
        } else {
            self.return_decode_buffer(rgb);
            None
        }
    }

    /// Decode a base64 `WebDriver` PNG screenshot.
    #[must_use]
    pub fn store_base64_png(&self, data: &str) -> Option<u64> {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
            self.store_png(&bytes)
        } else {
            self.record_decode_failure();
            None
        }
    }

    fn publish_rgb(&self, width: u32, height: u32, rgb: Vec<u8>) -> u64 {
        let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.next_seq += 1;
        let frame = Arc::new(FrameData {
            width,
            height,
            rgb,
            seq: inner.next_seq,
        });
        let old = inner.data.replace(frame);
        if let Some(old) = old {
            retain_frame_buffer(&mut inner, old);
        }
        self.metrics.decoded_frames.fetch_add(1, Ordering::Relaxed);
        self.metrics.published_frames.fetch_add(1, Ordering::Relaxed);
        inner.next_seq
    }

    fn return_decode_buffer(&self, buffer: Vec<u8>) {
        let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if buffer.capacity() > inner.decode_buffer.capacity() {
            inner.decode_buffer = buffer;
        }
    }

    /// Drop the stored frame (e.g. when the session stops) so the UI falls
    /// back to its placeholder instead of showing stale content.
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(data) = inner.data.take() {
            retain_frame_buffer(&mut inner, data);
        }
    }

    /// Clone the newest frame handle, if any. Pixel conversion and texture
    /// upload can then proceed without holding the publication lock.
    #[must_use]
    pub fn latest(&self) -> Option<Arc<FrameData>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .data
            .clone()
    }

    /// Claim the single outstanding UI wake-up for this slot. Further
    /// frames remain coalesced in `latest` until the UI releases the claim.
    #[must_use]
    pub(crate) fn claim_notification(&self) -> bool {
        let claimed = self
            .notification_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        let counter = if claimed {
            &self.metrics.wakeups_claimed
        } else {
            &self.metrics.wakeups_coalesced
        };
        counter.fetch_add(1, Ordering::Relaxed);
        claimed
    }

    /// Let a later frame enqueue the next UI wake-up.
    pub fn release_notification(&self) {
        self.notification_pending.store(false, Ordering::Release);
    }

    /// Capabilities negotiated by the exact active session, if startup has
    /// reached its protocol-ready boundary.
    #[must_use]
    pub fn active_backend_capabilities(&self) -> Option<crate::ActiveBackendCapabilities> {
        *self
            .active_backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn publish_backend_capabilities(&self, capabilities: crate::ActiveBackendCapabilities) {
        *self
            .active_backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(capabilities);
    }

    pub fn clear_backend_capabilities(&self) {
        *self
            .active_backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    #[must_use]
    pub fn metrics(&self) -> FrameMetrics {
        FrameMetrics {
            frames_received: self.metrics.frames_received.load(Ordering::Relaxed),
            frames_acked: self.metrics.frames_acked.load(Ordering::Relaxed),
            capture_requests: self.metrics.capture_requests.load(Ordering::Relaxed),
            capture_completions: self.metrics.capture_completions.load(Ordering::Relaxed),
            capture_failures: self.metrics.capture_failures.load(Ordering::Relaxed),
            capture_superseded: self.metrics.capture_superseded.load(Ordering::Relaxed),
            encoded_bytes: self.metrics.encoded_bytes.load(Ordering::Relaxed),
            decoded_frames: self.metrics.decoded_frames.load(Ordering::Relaxed),
            decode_failures: self.metrics.decode_failures.load(Ordering::Relaxed),
            unchanged_frames: self.metrics.unchanged_frames.load(Ordering::Relaxed),
            published_frames: self.metrics.published_frames.load(Ordering::Relaxed),
            wakeups_claimed: self.metrics.wakeups_claimed.load(Ordering::Relaxed),
            wakeups_coalesced: self.metrics.wakeups_coalesced.load(Ordering::Relaxed),
            commands_coalesced: self.metrics.commands_coalesced.load(Ordering::Relaxed),
            commands_rejected: self.metrics.commands_rejected.load(Ordering::Relaxed),
            interaction_frame_samples: self.metrics.interaction_frame_samples.load(Ordering::Relaxed),
            interaction_frame_total_us: self.metrics.interaction_frame_total_us.load(Ordering::Relaxed),
            interaction_frame_max_us: self.metrics.interaction_frame_max_us.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn record_capture_request(&self) {
        self.metrics.capture_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_frame_received(&self) {
        self.metrics.frames_received.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_frame_acked(&self) {
        self.metrics.frames_acked.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_capture_completion(&self) {
        self.metrics.capture_completions.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_capture_failure(&self) {
        self.metrics.capture_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_capture_superseded(&self) {
        self.metrics.capture_superseded.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_unchanged_frame(&self) {
        self.metrics.unchanged_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_command_coalesced(&self) {
        self.metrics.commands_coalesced.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_command_rejected(&self) {
        self.metrics.commands_rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_interaction_to_frame(&self, elapsed: std::time::Duration) {
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.metrics.interaction_frame_samples.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .interaction_frame_total_us
            .fetch_add(micros, Ordering::Relaxed);
        self.metrics
            .interaction_frame_max_us
            .fetch_max(micros, Ordering::Relaxed);
    }

    fn record_encoded_bytes(&self, bytes: usize) {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        self.metrics.encoded_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    fn record_decode_failure(&self) {
        self.metrics.decode_failures.fetch_add(1, Ordering::Relaxed);
    }
}

fn take_decode_buffer(inner: &mut FrameSlotInner) -> Vec<u8> {
    if !inner.decode_buffer.is_empty() {
        return std::mem::take(&mut inner.decode_buffer);
    }
    let Some(index) = inner
        .retired_frames
        .iter()
        .position(|frame| Arc::strong_count(frame) == 1)
    else {
        return Vec::new();
    };
    let frame = inner.retired_frames.swap_remove(index);
    Arc::try_unwrap(frame).map_or_else(|_| Vec::new(), |frame| frame.rgb)
}

fn retain_frame_buffer(inner: &mut FrameSlotInner, frame: Arc<FrameData>) {
    match Arc::try_unwrap(frame) {
        Ok(frame) => {
            if frame.rgb.capacity() > inner.decode_buffer.capacity() {
                inner.decode_buffer = frame.rgb;
            }
        }
        Err(frame) => {
            inner.retired_frames.push(frame);
            if inner.retired_frames.len() > MAX_RETIRED_FRAMES {
                inner.retired_frames.remove(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smallest plausible JPEG: a 1x1 black pixel.
    const JPEG_1X1: &[u8] = &[
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01,
        0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06, 0x07, 0x06, 0x05, 0x08, 0x07, 0x07, 0x07, 0x09,
        0x09, 0x08, 0x0A, 0x0C, 0x14, 0x0D, 0x0C, 0x0B, 0x0B, 0x0C, 0x19, 0x12, 0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F,
        0x1E, 0x1D, 0x1A, 0x1C, 0x1C, 0x20, 0x24, 0x2E, 0x27, 0x20, 0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28, 0x37, 0x29,
        0x2C, 0x30, 0x31, 0x34, 0x34, 0x34, 0x1F, 0x27, 0x39, 0x3D, 0x38, 0x32, 0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF,
        0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00, 0x1F, 0x00, 0x00,
        0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02,
        0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0xFF, 0xC4, 0x00, 0xB5, 0x10, 0x00, 0x02, 0x01, 0x03,
        0x03, 0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00, 0x01, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11,
        0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08,
        0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18,
        0x19, 0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45,
        0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67,
        0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
        0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9,
        0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9,
        0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8,
        0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01,
        0x00, 0x00, 0x3F, 0x00, 0x7B, 0x94, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x4A, 0xFF, 0xD9,
    ];

    #[test]
    fn decodes_and_bumps_seq() {
        let slot = FrameSlot::new();
        assert!(slot.latest().is_none());
        let seq = slot.store_jpeg(JPEG_1X1).expect("decode 1x1 jpeg");
        assert_eq!(seq, 1);
        let data = slot.latest().unwrap();
        assert_eq!((data.width, data.height), (1, 1));
        assert_eq!(data.rgb.len(), 3);
        assert_eq!(data.seq, 1);
        drop(data);
        let seq2 = slot.store_jpeg(JPEG_1X1).unwrap();
        assert_eq!(seq2, 2);
    }

    #[test]
    fn reuses_a_retired_frame_after_the_ui_releases_it() {
        let slot = FrameSlot::new();
        assert_eq!(slot.store_jpeg(JPEG_1X1), Some(1));
        let first = slot.latest().unwrap();
        let first_buffer = first.rgb.as_ptr();

        assert_eq!(slot.store_jpeg(JPEG_1X1), Some(2));
        drop(first);
        assert_eq!(slot.store_jpeg(JPEG_1X1), Some(3));

        assert_eq!(slot.latest().unwrap().rgb.as_ptr(), first_buffer);
    }

    #[test]
    fn rejects_garbage() {
        let slot = FrameSlot::new();
        assert!(slot.store_jpeg(b"not a jpeg at all").is_none());
        assert_eq!(
            slot.metrics(),
            FrameMetrics {
                encoded_bytes: 17,
                decode_failures: 1,
                ..FrameMetrics::default()
            }
        );
    }

    #[test]
    fn decodes_base64_jpeg() {
        let slot = FrameSlot::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(JPEG_1X1);

        assert_eq!(slot.store_base64_jpeg(&encoded), Some(1));
        assert_eq!(slot.latest().map(|frame| (frame.width, frame.height)), Some((1, 1)));
    }

    #[test]
    fn decodes_base64_webdriver_png() {
        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(&[4, 8, 15, 255]).expect("png pixel");
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(png_bytes);
        let slot = FrameSlot::new();

        assert_eq!(slot.store_base64_png(&encoded), Some(1));
        let frame = slot.latest().expect("decoded frame");
        assert_eq!((frame.width, frame.height), (1, 1));
        assert_eq!(frame.rgb, [4, 8, 15]);
    }

    #[test]
    fn frame_notifications_coalesce_until_released() {
        let slot = FrameSlot::new();

        assert!(slot.claim_notification());
        assert!(!slot.claim_notification());
        slot.release_notification();
        assert!(slot.claim_notification());
        assert_eq!(slot.metrics().wakeups_claimed, 2);
        assert_eq!(slot.metrics().wakeups_coalesced, 1);
    }

    #[test]
    fn frame_metrics_cover_capture_publication_and_latency() {
        let slot = FrameSlot::new();
        slot.record_capture_request();
        slot.record_capture_completion();
        slot.record_capture_failure();
        slot.record_capture_superseded();
        slot.record_interaction_to_frame(std::time::Duration::from_micros(125));
        assert_eq!(slot.store_jpeg(JPEG_1X1), Some(1));

        let metrics = slot.metrics();
        assert_eq!(metrics.capture_requests, 1);
        assert_eq!(metrics.capture_completions, 1);
        assert_eq!(metrics.capture_failures, 1);
        assert_eq!(metrics.capture_superseded, 1);
        assert_eq!(metrics.decoded_frames, 1);
        assert_eq!(metrics.published_frames, 1);
        assert_eq!(metrics.interaction_frame_samples, 1);
        assert_eq!(metrics.interaction_frame_total_us, 125);
        assert_eq!(metrics.interaction_frame_max_us, 125);
    }
}
