//! Microphone capture thread. cpal streams are not `Send`, so the stream is
//! created, owned, and dropped entirely on this thread; the frame loop talks
//! to it over mpsc channels. Every result message is tagged with the
//! recording generation so a stale error or buffer from an earlier,
//! cancelled recording can never affect a newer one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub const TARGET_SAMPLE_RATE: u32 = 16_000;
/// Hard cap on a single recording so a stuck hotkey cannot grow unbounded.
/// Applied to the 16 kHz mono output (≈37 MiB), not the raw device stream —
/// audio is downmixed and resampled incrementally inside the callback, so
/// high native rates cannot balloon memory before conversion.
const MAX_RECORD_SECONDS: usize = 600;
const MAX_OUTPUT_SAMPLES: usize = MAX_RECORD_SECONDS * TARGET_SAMPLE_RATE as usize;
/// The frame loop pings the capture thread every frame while recording. If
/// no ping arrives for this long, frames have stopped (e.g. a Wayland
/// surface minimized mid-dictation, which delivers no redraw callbacks) —
/// the microphone must not stay open, so the capture thread self-cancels.
const HEARTBEAT_STALE: Duration = Duration::from_millis(1500);
const HEARTBEAT_POLL: Duration = Duration::from_millis(250);

pub enum CaptureCmd {
    /// Begin recording under the given generation.
    Start(u64),
    /// Stop and deliver the captured audio as 16 kHz mono f32.
    Stop,
    /// Stop and discard.
    Cancel,
}

pub struct CaptureHandle {
    cmd_tx: Sender<CaptureCmd>,
    pcm_rx: Receiver<(u64, Result<Vec<f32>, String>)>,
    heartbeat: Arc<Mutex<Instant>>,
}

impl CaptureHandle {
    pub fn spawn() -> std::io::Result<Self> {
        let (cmd_tx, cmd_rx) = channel();
        let (pcm_tx, pcm_rx) = channel();
        let heartbeat = Arc::new(Mutex::new(Instant::now()));
        let worker_heartbeat = Arc::clone(&heartbeat);
        thread::Builder::new()
            .name("speech-capture".into())
            .spawn(move || capture_loop(&cmd_rx, &pcm_tx, &worker_heartbeat))?;
        Ok(Self {
            cmd_tx,
            pcm_rx,
            heartbeat,
        })
    }

    pub fn send(&self, cmd: CaptureCmd) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Called every frame while the engine is active: proves the frame loop
    /// is still running so the capture thread keeps recording.
    pub fn heartbeat(&self) {
        *self.heartbeat.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    pub fn try_recv_pcm(&self) -> Option<(u64, Result<Vec<f32>, String>)> {
        self.pcm_rx.try_recv().ok()
    }
}

struct ActiveRecording {
    // Keeps the input stream alive; dropped to stop capture.
    _stream: cpal::Stream,
    generation: u64,
    /// Already downmixed to mono and resampled to 16 kHz.
    samples: Arc<Mutex<Vec<f32>>>,
    overflowed: Arc<AtomicBool>,
}

/// Stateful downmix-to-mono + linear resample to 16 kHz, fed chunk by chunk
/// from the audio callback so only converted output is ever buffered.
struct MonoResampler {
    channels: usize,
    /// Source frames advanced per output sample.
    step: f64,
    /// Fractional read position within `pending` (mono, source rate).
    position: f64,
    /// Last mono sample of the previous chunk plus this chunk's mono frames.
    pending: Vec<f32>,
    /// Partial interleaved frame carried across callback chunks: the audio
    /// callback is under no obligation to deliver whole frames per chunk.
    frame_acc: f32,
    frame_fill: usize,
}

impl MonoResampler {
    fn new(sample_rate: u32, channels: usize) -> Self {
        Self {
            channels: channels.max(1),
            step: f64::from(sample_rate) / f64::from(TARGET_SAMPLE_RATE),
            position: 0.0,
            pending: Vec::new(),
            frame_acc: 0.0,
            frame_fill: 0,
        }
    }

    /// Convert an interleaved chunk, appending 16 kHz mono samples to `out`.
    ///
    /// Downsampling (mic rate > 16 kHz, the usual case) uses a box filter:
    /// each output sample averages the source samples across its window, so
    /// content above 8 kHz is attenuated instead of aliasing into the speech
    /// band (point-sampling e.g. every third sample at 48 kHz would alias).
    /// Upsampling falls back to linear interpolation. Holds back up to one
    /// window (< 1 ms) at the stream tail, irrelevant for recognition.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "sample positions are far below every cast's precision limit"
    )]
    fn feed(&mut self, chunk: impl Iterator<Item = f32>, out: &mut Vec<f32>) {
        for sample in chunk {
            self.frame_acc += sample;
            self.frame_fill += 1;
            if self.frame_fill == self.channels {
                self.pending.push(self.frame_acc / usize_to_f32(self.channels));
                self.frame_acc = 0.0;
                self.frame_fill = 0;
            }
        }

        let available = self.pending.len() as f64;
        // Emit each output sample whose full source window is buffered.
        while self.position + self.step <= available {
            let window_end = self.position + self.step;
            if self.step > 1.0 {
                // Box-average the samples covered by [position, window_end).
                let start = self.position.floor() as usize;
                let end = (window_end.ceil() as usize).min(self.pending.len());
                let slice = &self.pending[start..end.max(start + 1)];
                let sum: f32 = slice.iter().sum();
                out.push(sum / usize_to_f32(slice.len()));
            } else {
                // Upsampling / near-unity: linear interpolation.
                let base = self.position.floor() as usize;
                let frac = (self.position - self.position.floor()) as f32;
                let current = self.pending[base];
                let next = self.pending.get(base + 1).copied().unwrap_or(current);
                out.push(current + (next - current) * frac);
            }
            self.position = window_end;
        }

        // Keep only the tail still needed for the next window.
        let keep_from = (self.position.floor() as usize).min(self.pending.len());
        self.pending.drain(..keep_from);
        self.position -= keep_from as f64;
    }
}

/// Recover a poisoned buffer lock instead of panicking: the protected data
/// is plain PCM samples, valid regardless of where another thread panicked.
fn lock_samples(samples: &Mutex<Vec<f32>>) -> MutexGuard<'_, Vec<f32>> {
    samples.lock().unwrap_or_else(PoisonError::into_inner)
}

fn capture_loop(
    cmd_rx: &Receiver<CaptureCmd>,
    pcm_tx: &Sender<(u64, Result<Vec<f32>, String>)>,
    heartbeat: &Arc<Mutex<Instant>>,
) {
    let mut active: Option<ActiveRecording> = None;
    loop {
        // Poll while recording so a stalled frame loop (minimized Wayland
        // surface) can be detected; block indefinitely when idle.
        let cmd = if active.is_some() {
            match cmd_rx.recv_timeout(HEARTBEAT_POLL) {
                Ok(cmd) => Some(cmd),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match cmd_rx.recv() {
                Ok(cmd) => Some(cmd),
                Err(_) => return,
            }
        };

        let Some(cmd) = cmd else {
            // Timeout while recording: finalize if the frame loop went silent
            // (heartbeat stale — minimized Wayland surface) or the recording
            // hit the length cap. Either releases the OS microphone even
            // though no frame will run to do it.
            let finalize = active.as_ref().is_some_and(|recording| {
                let stale = heartbeat.lock().unwrap_or_else(PoisonError::into_inner).elapsed() > HEARTBEAT_STALE;
                stale || recording.overflowed.load(Ordering::Relaxed)
            });
            if let Some(recording) = finalize.then(|| active.take()).flatten() {
                let capped = recording.overflowed.load(Ordering::Relaxed);
                {
                    let generation = recording.generation;
                    let samples = Arc::clone(&recording.samples);
                    drop(recording); // drops the stream, closing the mic
                    if capped {
                        // Deliver what was captured up to the cap.
                        let pcm = std::mem::take(&mut *lock_samples(&samples));
                        tracing::warn!("speech recording hit the {MAX_RECORD_SECONDS}s cap; stopping");
                        let _ = pcm_tx.send((generation, Ok(pcm)));
                    } else {
                        tracing::warn!("speech capture heartbeat stale; microphone released");
                        let _ = pcm_tx.send((
                            generation,
                            Err("recording stopped: window no longer visible".to_string()),
                        ));
                    }
                }
            }
            continue;
        };

        match cmd {
            CaptureCmd::Start(generation) => {
                if active.is_none() {
                    *heartbeat.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
                    match start_recording(generation, pcm_tx.clone()) {
                        Ok(recording) => active = Some(recording),
                        Err(error) => {
                            let _ = pcm_tx.send((generation, Err(error)));
                        }
                    }
                }
            }
            CaptureCmd::Stop => {
                if let Some(recording) = active.take() {
                    let generation = recording.generation;
                    let samples = Arc::clone(&recording.samples);
                    let overflowed = recording.overflowed.load(Ordering::Relaxed);
                    // Dropping the recording tears down the stream before the
                    // buffer is drained, so no samples race the drain.
                    drop(recording);
                    let pcm = std::mem::take(&mut *lock_samples(&samples));
                    if overflowed {
                        tracing::warn!("speech recording hit the {MAX_RECORD_SECONDS}s cap; truncated");
                    }
                    let _ = pcm_tx.send((generation, Ok(pcm)));
                }
            }
            CaptureCmd::Cancel => {
                active = None;
            }
        }
    }
}

fn start_recording(
    generation: u64,
    pcm_tx: Sender<(u64, Result<Vec<f32>, String>)>,
) -> Result<ActiveRecording, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "no microphone found (no default input device)".to_string())?;
    let device_name = device
        .description()
        .map_or_else(|_| "unknown".to_string(), |description| description.name().to_string());
    let supported = device
        .default_input_config()
        .map_err(|error| format!("microphone `{device_name}`: {error}"))?;

    let sample_rate = supported.sample_rate();
    let channels = usize::from(supported.channels());
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let overflowed = Arc::new(AtomicBool::new(false));

    // Runtime stream failures (e.g. the microphone being unplugged) must
    // reach the engine over the channel, not just the log, or the UI would
    // stay in Recording until manually cancelled.
    let error_device = device_name.clone();
    let error_cb = move |error: cpal::Error| {
        tracing::warn!(%error, "speech capture stream error");
        let _ = pcm_tx.send((generation, Err(format!("microphone `{error_device}`: {error}"))));
    };
    // One arm per interleaved sample type, each normalizing to f32 in
    // [-1, 1] before the shared downmix/resample. Covers the formats real
    // capture endpoints report: F32/I16/U16 plus 8-bit, 32-bit (incl. the
    // I32 that 24-bit WASAPI/ALSA endpoints deliver), and F64.
    macro_rules! input_stream {
        ($sample:ty, $to_f32:expr) => {{
            let sink = Arc::clone(&samples);
            let full = Arc::clone(&overflowed);
            let mut resampler = MonoResampler::new(sample_rate, channels);
            device.build_input_stream(
                config,
                move |data: &[$sample], _: &cpal::InputCallbackInfo| {
                    convert_into(&mut resampler, &sink, &full, data.iter().map($to_f32));
                },
                error_cb,
                None,
            )
        }};
    }
    let stream = match sample_format {
        SampleFormat::F32 => input_stream!(f32, |&sample| sample),
        SampleFormat::F64 => input_stream!(f64, |&sample| f64_to_sample(sample)),
        SampleFormat::I8 => input_stream!(i8, |&sample| f32::from(sample) / 128.0),
        SampleFormat::I16 => input_stream!(i16, |&sample| f32::from(sample) / 32_768.0),
        SampleFormat::I32 => input_stream!(i32, |&sample| i32_to_sample(sample)),
        SampleFormat::U8 => input_stream!(u8, |&sample| (f32::from(sample) - 128.0) / 128.0),
        SampleFormat::U16 => input_stream!(u16, |&sample| (f32::from(sample) - 32_768.0) / 32_768.0),
        other => {
            return Err(format!(
                "microphone `{device_name}`: unsupported sample format {other:?}"
            ));
        }
    }
    .map_err(|error| format!("microphone `{device_name}`: {error}"))?;

    stream
        .play()
        .map_err(|error| format!("microphone `{device_name}`: {error}"))?;
    tracing::info!(device = %device_name, sample_rate, channels, "speech recording started");

    Ok(ActiveRecording {
        _stream: stream,
        generation,
        samples,
        overflowed,
    })
}

/// Audio-callback path: downmix + resample the chunk and append to the
/// shared 16 kHz mono buffer, enforcing the output-sample cap.
fn convert_into(
    resampler: &mut MonoResampler,
    sink: &Arc<Mutex<Vec<f32>>>,
    overflowed: &Arc<AtomicBool>,
    chunk: impl Iterator<Item = f32>,
) {
    let mut buffer = lock_samples(sink);
    if buffer.len() >= MAX_OUTPUT_SAMPLES {
        overflowed.store(true, Ordering::Relaxed);
        return;
    }
    resampler.feed(chunk, &mut buffer);
    if buffer.len() > MAX_OUTPUT_SAMPLES {
        buffer.truncate(MAX_OUTPUT_SAMPLES);
        overflowed.store(true, Ordering::Relaxed);
    }
}

/// Downmix interleaved samples to mono and resample to 16 kHz.
///
/// Integer downsample factors (48 kHz, 32 kHz) use a box filter; everything
/// else (e.g. 44.1 kHz) falls back to linear interpolation. Whisper-family
/// models are robust to this level of resampling fidelity.
/// Narrow an f64 device sample to the f32 capture pipeline.
#[expect(clippy::cast_possible_truncation, reason = "audio downconvert to the f32 pipeline")]
fn f64_to_sample(sample: f64) -> f32 {
    sample as f32
}

/// Normalize a 32-bit integer device sample to f32 in [-1, 1).
#[expect(clippy::cast_precision_loss, reason = "audio downconvert to the f32 pipeline")]
fn i32_to_sample(sample: i32) -> f32 {
    sample as f32 / 2_147_483_648.0
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "sample positions/counts are far below every cast's precision limit"
)]
#[cfg(test)]
pub fn to_mono_16k(samples: &[f32], sample_rate: u32, channels: usize) -> Vec<f32> {
    let channels = channels.max(1);
    let mono: Vec<f32> = if channels == 1 {
        samples.to_vec()
    } else {
        samples
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / usize_to_f32(channels))
            .collect()
    };

    if sample_rate == TARGET_SAMPLE_RATE {
        return mono;
    }
    if sample_rate > TARGET_SAMPLE_RATE && sample_rate.is_multiple_of(TARGET_SAMPLE_RATE) {
        let factor = (sample_rate / TARGET_SAMPLE_RATE) as usize;
        return mono
            .chunks_exact(factor)
            .map(|window| window.iter().sum::<f32>() / usize_to_f32(factor))
            .collect();
    }

    let ratio = f64::from(sample_rate) / f64::from(TARGET_SAMPLE_RATE);
    let out_len = (mono.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for index in 0..out_len {
        let position = index as f64 * ratio;
        let base = position.floor() as usize;
        let frac = (position - position.floor()) as f32;
        let current = mono.get(base).copied().unwrap_or(0.0);
        let next = mono.get(base + 1).copied().unwrap_or(current);
        out.push(current + (next - current) * frac);
    }
    out
}

#[expect(clippy::cast_precision_loss, reason = "channel/window counts are tiny")]
fn usize_to_f32(value: usize) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::{CaptureCmd, CaptureHandle, TARGET_SAMPLE_RATE, to_mono_16k};

    #[test]
    fn stereo_48k_downmixes_and_decimates() {
        // 1 second of stereo 48 kHz: left = 0.5, right = -0.5 → mono 0.0.
        let samples: Vec<f32> = (0..48_000).flat_map(|_| [0.5, -0.5]).collect();
        let out = to_mono_16k(&samples, 48_000, 2);
        assert_eq!(out.len(), TARGET_SAMPLE_RATE as usize);
        assert!(out.iter().all(|sample| sample.abs() < 1e-6));
    }

    #[test]
    fn mono_44100_resamples_to_16k_length() {
        let samples = vec![0.25_f32; 44_100];
        let out = to_mono_16k(&samples, 44_100, 1);
        let expected = 16_000_usize;
        assert!(
            out.len().abs_diff(expected) <= 2,
            "expected ~{expected} samples, got {}",
            out.len()
        );
        assert!((out[100] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn native_rate_passes_through() {
        let samples = vec![0.1_f32; 1600];
        assert_eq!(to_mono_16k(&samples, 16_000, 1), samples);
    }

    /// The streaming path must produce the same signal as the batch helper
    /// regardless of how the audio callback happens to chunk the input.
    #[test]
    #[expect(clippy::cast_precision_loss, reason = "test signal generation; indices are tiny")]
    fn streaming_resampler_matches_batch_conversion() {
        // 48 kHz -> 16 kHz is the common integer factor (3), where the
        // streaming box filter and the batch box filter agree sample-for-
        // sample; that is what the production path uses most.
        let rate = 48_000_u32;
        let samples: Vec<f32> = (0..rate as usize)
            .flat_map(|index| {
                let t = index as f32 / rate as f32;
                let value = (t * std::f32::consts::TAU * 220.0).sin();
                [value, value]
            })
            .collect();
        let batch = to_mono_16k(&samples, rate, 2);

        let mut resampler = super::MonoResampler::new(rate, 2);
        let mut streamed = Vec::new();
        for chunk in samples.chunks(1337) {
            resampler.feed(chunk.iter().copied(), &mut streamed);
        }

        // The streaming path holds back up to one interpolation window at
        // the tail (< 1 ms) because nothing flushes at stream teardown.
        assert!(
            streamed.len().abs_diff(batch.len()) <= 16,
            "length mismatch: streamed {} vs batch {}",
            streamed.len(),
            batch.len()
        );
        for (index, (a, b)) in streamed.iter().zip(batch.iter()).enumerate() {
            assert!((a - b).abs() < 1e-4, "sample {index} diverged: {a} vs {b}");
        }
    }

    /// A8 smoke check: on a machine with no usable input device, starting
    /// capture must surface an error over the channel without panicking — the
    /// worker keeps running and the mic returns to idle. This covers both the
    /// "no default input device" case and a phantom device whose input config
    /// cannot be opened (a Mac Studio with no mic reports the latter); the
    /// frame loop logs either as a `speech input error`. Gated on an env var so
    /// `cargo test --features speech` stays green on machines with a working
    /// microphone:
    ///
    /// ```sh
    /// HORIZON_SPEECH_TEST_NO_MIC=1 \
    ///   cargo test --features speech no_microphone -- --nocapture
    /// ```
    #[test]
    fn no_microphone_start_reports_error_without_panicking() {
        if std::env::var_os("HORIZON_SPEECH_TEST_NO_MIC").is_none() {
            eprintln!("skipped: HORIZON_SPEECH_TEST_NO_MIC not set");
            return;
        }
        let handle = CaptureHandle::spawn().expect("spawn capture thread");
        handle.send(CaptureCmd::Start(7));
        let mut received = None;
        for _ in 0..200 {
            if let Some(pcm) = handle.try_recv_pcm() {
                received = Some(pcm);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        match received {
            Some((generation, Err(message))) => {
                assert_eq!(generation, 7, "error must carry the requesting generation");
                eprintln!("capture-start error surfaced without panic: {message}");
                assert!(!message.trim().is_empty(), "error message should not be empty");
            }
            Some((_, Ok(_))) => panic!("expected an error with no usable input device, got audio"),
            None => panic!("capture worker sent no result within the timeout"),
        }
    }
}
