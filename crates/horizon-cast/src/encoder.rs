mod backend;
mod diagnostics;
pub(crate) use backend::select;
pub use backend::{EncoderBackend, EncoderSelection};

use crate::{CastStatus, Error, MirrorSession, Result, VideoFormat, session::lock};
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

pub(crate) fn stream(
    mut mirror: MirrorSession,
    format: VideoFormat,
    frames: &Receiver<Vec<u8>>,
    status: &Arc<Mutex<CastStatus>>,
    stop: &Arc<AtomicBool>,
    process: &Arc<Mutex<Option<Child>>>,
    backend: EncoderBackend,
) -> Result<()> {
    let (width, height) = format.dimensions();
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            &format!("{width}x{height}"),
            "-framerate",
            "15",
            "-i",
            "pipe:0",
            "-an",
        ])
        .args(backend.arguments())
        .args(["-g", "30", "-bf", "0", "-flush_packets", "1", "-f", "h264", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let diagnostics = child.stderr.take().map(diagnostics::drain);
    let mut input = child.stdin.take().ok_or(Error::Protocol("encoder input unavailable"))?;
    let output = child
        .stdout
        .take()
        .ok_or(Error::Protocol("encoder output unavailable"))?;
    *lock(process) = Some(child);
    *lock(status) = CastStatus::Streaming { frames: 0 };
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
        match frames.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => {
                last_frame = Instant::now();
                if let Err(error) = input.write_all(&frame) {
                    result = Err(error.into());
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if last_frame.elapsed() > Duration::from_secs(3) {
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
    status: &Mutex<CastStatus>,
    stop: &AtomicBool,
) -> Result<()> {
    let mut pending = Vec::new();
    let mut chunk = vec![0; 32768];
    let mut access = Vec::new();
    let mut sps = Vec::new();
    let mut sent = 0;
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
        pending.extend(&chunk[..count]);
        if pending.len() > 8 * 1024 * 1024 {
            return Err(Error::Protocol("encoder NAL exceeds limit"));
        }
        while let Some(nal) = take_nal(&mut pending) {
            let kind = nal.first().copied().unwrap_or(0) & 31;
            match kind {
                9 => {
                    if !access.is_empty() {
                        mirror.send(&access.iter().map(Vec::as_slice).collect::<Vec<_>>())?;
                        access.clear();
                        sent += 1;
                        *lock(status) = CastStatus::Streaming { frames: sent };
                    }
                }
                7 => sps = nal,
                8 => {
                    mirror.configure(&sps, &nal)?;
                }
                1 | 5 | 6 => access.push(nal),
                _ => {}
            }
            if access.iter().map(Vec::len).sum::<usize>() > 8 * 1024 * 1024 {
                return Err(Error::Protocol("encoder access unit exceeds limit"));
            }
        }
    }
}
fn take_nal(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let first = start_code(buffer, 0)?;
    let next = start_code(buffer, first.0 + first.1)?;
    let nal = buffer[first.0 + first.1..next.0].to_vec();
    buffer.drain(..next.0);
    Some(nal)
}
fn start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    for at in from..data.len().saturating_sub(2) {
        if data.get(at..at + 4) == Some(&[0, 0, 0, 1]) {
            return Some((at, 4));
        }
        if data.get(at..at + 3) == Some(&[0, 0, 1]) {
            return Some((at, 3));
        }
    }
    None
}
