pub use horizon_media::encoder::{EncoderBackend, EncoderSelection};
pub(crate) use horizon_media::encoder::{Frame, FrameInput, select};

use crate::{
    CastStatus, Error, MirrorSession, Result, VideoFormat,
    session::{Progress, lock},
};
use horizon_media::{
    encoder::{AccessUnitSink, EncoderConfig},
    h264::AccessUnit,
};
use std::{
    process::Child,
    sync::atomic::AtomicBool,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Feeds the shared encoder pipeline into an authenticated mirror session.
struct MirrorSink {
    mirror: MirrorSession,
    status: Arc<Mutex<Progress>>,
}

impl AccessUnitSink for MirrorSink {
    type Error = Error;

    fn started(&mut self) {
        lock(&self.status).state = CastStatus::Streaming { frames: 0 };
    }

    fn configure(&mut self, sps: &[u8], pps: &[u8]) -> Result<()> {
        self.mirror.configure(sps, pps)
    }

    fn send(&mut self, unit: &AccessUnit, _pts: Duration) -> Result<()> {
        // The receiver clock, not the input frame time, timestamps mirrored pictures.
        self.mirror.send(&unit.nal_refs())?;
        lock(&self.status).record_transmission();
        Ok(())
    }
}

pub(crate) fn stream(
    mirror: MirrorSession,
    format: VideoFormat,
    frames: FrameInput,
    status: &Arc<Mutex<Progress>>,
    stop: &Arc<AtomicBool>,
    process: &Arc<Mutex<Option<Child>>>,
    backend: EncoderBackend,
) -> Result<()> {
    let sink = MirrorSink {
        mirror,
        status: status.clone(),
    };
    horizon_media::encoder::stream(sink, format, frames, EncoderConfig::default(), stop, process, backend)
}
