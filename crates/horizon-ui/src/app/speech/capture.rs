//! Microphone capture thread. cpal streams are not `Send`, so the stream is
//! created, owned, and dropped entirely on this thread; the frame loop talks
//! to it over mpsc channels.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub const TARGET_SAMPLE_RATE: u32 = 16_000;
/// Hard cap on a single recording so a stuck hotkey cannot grow unbounded.
const MAX_RECORD_SECONDS: usize = 600;

pub enum CaptureCmd {
    Start,
    /// Stop and deliver the captured audio as 16 kHz mono f32.
    Stop,
    /// Stop and discard.
    Cancel,
}

pub struct CaptureHandle {
    cmd_tx: Sender<CaptureCmd>,
    pcm_rx: Receiver<Result<Vec<f32>, String>>,
}

impl CaptureHandle {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = channel();
        let (pcm_tx, pcm_rx) = channel();
        thread::Builder::new()
            .name("speech-capture".into())
            .spawn(move || capture_loop(&cmd_rx, &pcm_tx))
            .expect("spawn speech capture thread");
        Self { cmd_tx, pcm_rx }
    }

    pub fn send(&self, cmd: CaptureCmd) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn try_recv_pcm(&self) -> Option<Result<Vec<f32>, String>> {
        self.pcm_rx.try_recv().ok()
    }
}

struct ActiveRecording {
    // Keeps the input stream alive; dropped to stop capture.
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
    channels: usize,
    overflowed: Arc<AtomicBool>,
}

fn capture_loop(cmd_rx: &Receiver<CaptureCmd>, pcm_tx: &Sender<Result<Vec<f32>, String>>) {
    let mut active: Option<ActiveRecording> = None;
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            CaptureCmd::Start => {
                if active.is_none() {
                    match start_recording() {
                        Ok(recording) => active = Some(recording),
                        Err(error) => {
                            let _ = pcm_tx.send(Err(error));
                        }
                    }
                }
            }
            CaptureCmd::Stop => {
                if let Some(recording) = active.take() {
                    let samples = Arc::clone(&recording.samples);
                    let sample_rate = recording.sample_rate;
                    let channels = recording.channels;
                    let overflowed = recording.overflowed.load(Ordering::Relaxed);
                    // Dropping the recording tears down the stream before the
                    // buffer is drained, so no samples race the drain.
                    drop(recording);
                    let raw = std::mem::take(&mut *samples.lock().expect("capture buffer lock"));
                    if overflowed {
                        tracing::warn!("speech recording hit the {MAX_RECORD_SECONDS}s cap; truncated");
                    }
                    let _ = pcm_tx.send(Ok(to_mono_16k(&raw, sample_rate, channels)));
                }
            }
            CaptureCmd::Cancel => {
                active = None;
            }
        }
    }
}

fn start_recording() -> Result<ActiveRecording, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "no microphone found (no default input device)".to_string())?;
    let device_name = device.name().unwrap_or_else(|_| "unknown".to_string());
    let supported = device
        .default_input_config()
        .map_err(|error| format!("microphone `{device_name}`: {error}"))?;

    let sample_rate = supported.sample_rate().0;
    let channels = usize::from(supported.channels());
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let overflowed = Arc::new(AtomicBool::new(false));
    let max_samples = MAX_RECORD_SECONDS * sample_rate as usize * channels;

    let error_cb = |error: cpal::StreamError| tracing::warn!(%error, "speech capture stream error");
    let stream = match sample_format {
        SampleFormat::F32 => {
            let sink = Arc::clone(&samples);
            let full = Arc::clone(&overflowed);
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    push_samples(&sink, &full, max_samples, data.iter().copied());
                },
                error_cb,
                None,
            )
        }
        SampleFormat::I16 => {
            let sink = Arc::clone(&samples);
            let full = Arc::clone(&overflowed);
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    push_samples(
                        &sink,
                        &full,
                        max_samples,
                        data.iter().map(|&sample| f32::from(sample) / 32_768.0),
                    );
                },
                error_cb,
                None,
            )
        }
        SampleFormat::U16 => {
            let sink = Arc::clone(&samples);
            let full = Arc::clone(&overflowed);
            device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    push_samples(
                        &sink,
                        &full,
                        max_samples,
                        data.iter().map(|&sample| (f32::from(sample) - 32_768.0) / 32_768.0),
                    );
                },
                error_cb,
                None,
            )
        }
        other => {
            return Err(format!("microphone `{device_name}`: unsupported sample format {other:?}"));
        }
    }
    .map_err(|error| format!("microphone `{device_name}`: {error}"))?;

    stream
        .play()
        .map_err(|error| format!("microphone `{device_name}`: {error}"))?;
    tracing::info!(device = %device_name, sample_rate, channels, "speech recording started");

    Ok(ActiveRecording {
        _stream: stream,
        samples,
        sample_rate,
        channels,
        overflowed,
    })
}

fn push_samples(
    sink: &Arc<Mutex<Vec<f32>>>,
    overflowed: &Arc<AtomicBool>,
    max_samples: usize,
    data: impl Iterator<Item = f32>,
) {
    let mut buffer = sink.lock().expect("capture buffer lock");
    for sample in data {
        if buffer.len() >= max_samples {
            overflowed.store(true, Ordering::Relaxed);
            return;
        }
        buffer.push(sample);
    }
}

/// Downmix interleaved samples to mono and resample to 16 kHz.
///
/// Integer downsample factors (48 kHz, 32 kHz) use a box filter; everything
/// else (e.g. 44.1 kHz) falls back to linear interpolation. Whisper-family
/// models are robust to this level of resampling fidelity.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "sample positions/counts are far below every cast's precision limit"
)]
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
    use super::{TARGET_SAMPLE_RATE, to_mono_16k};

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
}
