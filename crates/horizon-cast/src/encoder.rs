pub use horizon_media::encoder::{EncoderBackend, EncoderSelection};
pub(crate) use horizon_media::encoder::{Frame, FrameInput, drain_diagnostics, select};

use crate::{
    CastStatus, Error, MirrorSession, Result, VideoFormat,
    session::{Progress, lock},
};
use horizon_media::h264::{AnnexBReader, Unit};
use std::{
    io::Read,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex, mpsc::RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

pub(crate) fn stream(
    mut mirror: MirrorSession,
    format: VideoFormat,
    mut frames: FrameInput,
    status: &Arc<Mutex<Progress>>,
    stop: &Arc<AtomicBool>,
    process: &Arc<Mutex<Option<Child>>>,
    backend: EncoderBackend,
) -> Result<()> {
    let mut child = Command::new("ffmpeg")
        .args(backend.input_arguments(format))
        .args(backend.arguments())
        .args(["-g", "30", "-bf", "0", "-flush_packets", "1", "-f", "h264", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let diagnostics = child.stderr.take().map(drain_diagnostics);
    let mut input = child.stdin.take().ok_or(Error::Protocol("encoder input unavailable"))?;
    let output = child
        .stdout
        .take()
        .ok_or(Error::Protocol("encoder output unavailable"))?;
    *lock(process) = Some(child);
    lock(status).state = CastStatus::Streaming { frames: 0 };
    let state = status.clone();
    let cancel = stop.clone();
    let killer = process.clone();
    let reader = thread::spawn(move || {
        let result = consume(output, &mut mirror, &state, &cancel);
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
                    result = Err(Error::Protocol("capture stopped producing frames"));
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
    let transport = reader
        .join()
        .map_err(|_| Error::Backend("video worker stopped unexpectedly".into()))?;
    let result = result.and(transport);
    if result.is_err()
        && let Some(reason) = diagnostics
            .and_then(|diagnostics| diagnostics.recv_timeout(Duration::from_millis(200)).ok())
            .flatten()
    {
        return Err(Error::Backend(reason.into()));
    }
    result
}

fn consume(
    mut reader: impl Read,
    mirror: &mut MirrorSession,
    status: &Mutex<Progress>,
    stop: &AtomicBool,
) -> Result<()> {
    let mut stream = AnnexBReader::default();
    let mut chunk = vec![0; 32768];
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            return if stop.load(Ordering::Relaxed) {
                Ok(())
            } else {
                Err(Error::Backend("H.264 encoder ended".into()))
            };
        }
        for unit in stream.push(&chunk[..count])? {
            match unit {
                Unit::ParameterSets { sps, pps } => mirror.configure(&sps, &pps)?,
                Unit::AccessUnit(access) => {
                    mirror.send(&access.nal_refs())?;
                    lock(status).record_transmission();
                }
            }
        }
    }
}
