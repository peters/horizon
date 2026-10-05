//! Encoder pipeline pieces shared by the casting transports: capture frame
//! input, `ffmpeg` backend selection and encoder diagnostics.
mod backend;
mod diagnostics;
mod frames;

pub use backend::{EncoderBackend, EncoderSelection, select};
pub use diagnostics::drain as drain_diagnostics;
pub use frames::{Frame, FrameError, FrameInput};

fn lock<T>(value: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
