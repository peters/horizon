//! The `ffmpeg` encoder process shared by the casting transports: capture
//! frames go in, access units come out to a transport-specific sink.
use super::{EncoderBackend, FrameInput, drain_diagnostics, lock};
use crate::{
    format::VideoFormat,
    h264::{AccessUnit, AnnexBReader, H264Error, Unit},
};
use std::{
    fmt, io,
    io::Read,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex, mpsc::RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

/// Frames per second the capture input is declared at (`-framerate`).
pub const INPUT_FRAME_RATE: u32 = 15;

/// Encoder settings that differ between transports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncoderConfig {
    /// Frames between keyframes (`-g`).
    pub keyframe_interval: u32,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self { keyframe_interval: 30 }
    }
}

impl EncoderConfig {
    /// One keyframe per segment, so segmenters can cut on schedule.
    #[must_use]
    pub fn for_segments(segment: Duration) -> Self {
        // Round up: a shorter interval would land before the segmenter's cut
        // threshold and push every cut to the following keyframe.
        let frames = (segment.as_millis() * u128::from(INPUT_FRAME_RATE)).div_ceil(1000);
        Self {
            keyframe_interval: u32::try_from(frames.clamp(1, 600)).unwrap_or(600),
        }
    }
}

/// Receives encoder output. Runs on the encoder's reader thread.
pub trait AccessUnitSink: Send + 'static {
    type Error: From<io::Error> + From<H264Error> + From<PipelineError> + Send + 'static;

    /// Called once the encoder process is running, before any output.
    fn started(&mut self) {}

    /// The current SPS and PPS; sent before the first access unit and on change.
    /// # Errors
    /// Returns the transport's error, which ends the stream.
    fn configure(&mut self, sps: &[u8], pps: &[u8]) -> Result<(), Self::Error>;

    /// One encoded picture. `pts` counts from the first encoded picture at the
    /// declared input frame rate.
    /// # Errors
    /// Returns the transport's error, which ends the stream.
    fn send(&mut self, unit: &AccessUnit, pts: Duration) -> Result<(), Self::Error>;
}

/// Failures of the encoder pipeline itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineError {
    InputUnavailable,
    OutputUnavailable,
    CaptureStalled,
    EncoderEnded,
    WorkerStopped,
    /// A fixed, privacy-safe category from the encoder's stderr.
    Diagnosed(&'static str),
}

impl PipelineError {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InputUnavailable => "encoder input unavailable",
            Self::OutputUnavailable => "encoder output unavailable",
            Self::CaptureStalled => "capture stopped producing frames",
            Self::EncoderEnded => "H.264 encoder ended",
            Self::WorkerStopped => "video worker stopped unexpectedly",
            Self::Diagnosed(reason) => reason,
        }
    }
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for PipelineError {}

/// Runs the encoder until `stop` is set, capture ends or a side fails. The
/// child is kept in `process` so the caller can kill it on cancellation.
/// # Errors
/// Returns the sink's error, an I/O error, or a [`PipelineError`]; when the
/// encoder printed a known failure, that category replaces the error.
pub fn stream<S: AccessUnitSink>(
    mut sink: S,
    format: VideoFormat,
    mut frames: FrameInput,
    config: EncoderConfig,
    stop: &Arc<AtomicBool>,
    process: &Arc<Mutex<Option<Child>>>,
    backend: EncoderBackend,
) -> Result<(), S::Error> {
    let keyframes = config.keyframe_interval.max(1).to_string();
    let mut child = Command::new("ffmpeg")
        .args(backend.input_arguments(format))
        .args(backend.arguments())
        .args([
            "-g",
            &keyframes,
            "-bf",
            "0",
            "-flush_packets",
            "1",
            "-f",
            "h264",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let diagnostics = child.stderr.take().map(drain_diagnostics);
    let mut input = child.stdin.take().ok_or(PipelineError::InputUnavailable)?;
    let output = child.stdout.take().ok_or(PipelineError::OutputUnavailable)?;
    *lock(process) = Some(child);
    sink.started();
    let cancel = stop.clone();
    let killer = process.clone();
    let reader = thread::spawn(move || {
        let result = consume(output, &mut sink, &cancel);
        if let Some(child) = lock(&killer).as_mut() {
            let _ = child.kill();
        }
        result
    });
    let mut result = Ok(());
    let mut last_frame = Instant::now();
    while !stop.load(Ordering::Relaxed) && !reader.is_finished() {
        match frames.next_frame() {
            Ok(frame) => {
                last_frame = Instant::now();
                if let Err(error) = frame.write(&mut input, backend.source_frames()) {
                    result = Err(error.into());
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if frames.stalled(&mut last_frame, Instant::now()) {
                    result = Err(PipelineError::CaptureStalled.into());
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(input);
    if let Some(child) = lock(process).as_mut() {
        let _ = child.kill();
    }
    let transport = reader.join().map_err(|_| PipelineError::WorkerStopped)?;
    let result = result.and(transport);
    if result.is_err()
        && let Some(reason) = diagnostics
            .and_then(|diagnostics| diagnostics.recv_timeout(Duration::from_millis(200)).ok())
            .flatten()
    {
        return Err(PipelineError::Diagnosed(reason).into());
    }
    result
}

fn consume<S: AccessUnitSink>(mut reader: impl Read, sink: &mut S, stop: &AtomicBool) -> Result<(), S::Error> {
    let mut stream = AnnexBReader::default();
    let mut chunk = vec![0; 32768];
    let frame = Duration::from_secs(1) / INPUT_FRAME_RATE;
    let mut pictures = 0u32;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            return if stop.load(Ordering::Relaxed) {
                Ok(())
            } else {
                Err(PipelineError::EncoderEnded.into())
            };
        }
        for unit in stream.push(&chunk[..count])? {
            match unit {
                Unit::ParameterSets { sps, pps } => sink.configure(&sps, &pps)?,
                Unit::AccessUnit(access) => {
                    sink.send(&access, frame * pictures)?;
                    pictures = pictures.saturating_add(1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    enum TestError {
        Io,
        H264,
        Pipeline(PipelineError),
    }
    impl From<io::Error> for TestError {
        fn from(_: io::Error) -> Self {
            Self::Io
        }
    }
    impl From<H264Error> for TestError {
        fn from(_: H264Error) -> Self {
            Self::H264
        }
    }
    impl From<PipelineError> for TestError {
        fn from(error: PipelineError) -> Self {
            Self::Pipeline(error)
        }
    }

    #[derive(Default)]
    struct Recorder {
        configured: Vec<(Vec<u8>, Vec<u8>)>,
        sent: Vec<(bool, Duration)>,
    }
    impl AccessUnitSink for Recorder {
        type Error = TestError;
        fn configure(&mut self, sps: &[u8], pps: &[u8]) -> Result<(), TestError> {
            self.configured.push((sps.to_vec(), pps.to_vec()));
            Ok(())
        }
        fn send(&mut self, unit: &AccessUnit, pts: Duration) -> Result<(), TestError> {
            self.sent.push((unit.is_keyframe(), pts));
            Ok(())
        }
    }

    fn picture(keyframe: bool) -> Vec<u8> {
        let mut bytes = vec![0, 0, 0, 1, 0x09, 0xf0];
        if keyframe {
            bytes.extend_from_slice(&[0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2]);
        }
        bytes.extend_from_slice(&[0, 0, 0, 1, if keyframe { 0x65 } else { 0x41 }, 7]);
        bytes
    }

    #[test]
    fn consume_configures_then_sends_pictures_at_the_input_rate() {
        let mut output: Vec<u8> = [picture(true), picture(false), picture(false)].concat();
        // A delimiter completes the last picture once the next start code follows it.
        output.extend_from_slice(&[0, 0, 0, 1, 0x09, 0xf0, 0, 0, 0, 1, 0x09, 0xf0]);
        let mut sink = Recorder::default();
        let result = consume(output.as_slice(), &mut sink, &AtomicBool::new(false));
        assert!(matches!(result, Err(TestError::Pipeline(PipelineError::EncoderEnded))));
        assert_eq!(sink.configured, [(vec![0x67, 1], vec![0x68, 2])]);
        let frame = Duration::from_secs(1) / INPUT_FRAME_RATE;
        assert_eq!(sink.sent, [(true, Duration::ZERO), (false, frame), (false, frame * 2)]);
    }

    #[test]
    fn consume_ends_quietly_when_stopped() {
        let mut sink = Recorder::default();
        assert!(consume(picture(true).as_slice(), &mut sink, &AtomicBool::new(true)).is_ok());
        assert!(sink.sent.is_empty());
    }

    #[test]
    fn keyframe_interval_follows_segment_length() {
        assert_eq!(EncoderConfig::default().keyframe_interval, 30);
        assert_eq!(
            EncoderConfig::for_segments(Duration::from_secs(1)).keyframe_interval,
            15
        );
        assert_eq!(
            EncoderConfig::for_segments(Duration::from_millis(500)).keyframe_interval,
            8
        );
        // Two frames (133 ms) would fall below a 150 ms segment's cut threshold.
        for short in [150, 160] {
            assert_eq!(
                EncoderConfig::for_segments(Duration::from_millis(short)).keyframe_interval,
                3
            );
        }
        assert_eq!(EncoderConfig::for_segments(Duration::ZERO).keyframe_interval, 1);
        assert_eq!(
            EncoderConfig::for_segments(Duration::from_secs(3600)).keyframe_interval,
            600
        );
    }
}
