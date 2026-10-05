//! The encoder pipeline shared by the casting transports: capture frame
//! input, `ffmpeg` backend selection, the encoder process and diagnostics.
//! Transports receive access units through [`AccessUnitSink`].
mod backend;
mod diagnostics;
mod frames;
mod pipeline;

pub use backend::{EncoderBackend, EncoderSelection, select};
pub use diagnostics::drain as drain_diagnostics;
pub use frames::{Frame, FrameError, FrameInput};
pub use pipeline::{AccessUnitSink, EncoderConfig, INPUT_FRAME_RATE, PipelineError, stream};

fn lock<T>(value: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
