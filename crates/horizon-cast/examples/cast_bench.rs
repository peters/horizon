//! Synthetic encoder/transport load generator; never discovers or connects to a TV.
#![forbid(unsafe_code)]

use horizon_cast::{CastSession, CastStatus, Error, Orientation, Resolution, VideoFormat};
use std::{
    net::SocketAddr,
    thread,
    time::{Duration, Instant},
};

struct Measurement {
    frames: u64,
    submitted: u32,
    seconds: f64,
    encoder: &'static str,
}

fn main() -> Result<(), Error> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.len() != 6 {
        return Err(Error::Protocol(
            "expected loopback-address resolution orientation seconds input-fps encoder",
        ));
    }
    let address: SocketAddr = arguments[0]
        .parse()
        .map_err(|_| Error::Protocol("invalid benchmark address"))?;
    if !address.ip().is_loopback() {
        return Err(Error::Protocol("benchmark receiver must be loopback"));
    }
    let format = VideoFormat {
        resolution: match arguments[1].as_str() {
            "720p" => Resolution::Hd720,
            "1080p" => Resolution::FullHd1080,
            "4k" => Resolution::Uhd4k,
            _ => return Err(Error::Protocol("invalid benchmark resolution")),
        },
        orientation: match arguments[2].as_str() {
            "landscape" => Orientation::Landscape,
            "portrait" => Orientation::Portrait,
            _ => return Err(Error::Protocol("invalid benchmark orientation")),
        },
    };
    let seconds: u64 = arguments[3]
        .parse()
        .map_err(|_| Error::Protocol("invalid benchmark duration"))?;
    let fps: u32 = arguments[4]
        .parse()
        .map_err(|_| Error::Protocol("invalid benchmark input rate"))?;
    if !(1..=120).contains(&seconds) || !(1..=120).contains(&fps) {
        return Err(Error::Protocol(
            "benchmark duration and input rate must be between 1 and 120",
        ));
    }
    let session = CastSession::start(address, format)?;
    let outcome = run(&session, format, seconds, fps, &arguments[5]);
    session.stop();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !session.finished() {
        if Instant::now() >= deadline {
            return Err(Error::Protocol("benchmark teardown timed out"));
        }
        thread::sleep(Duration::from_millis(10));
    }
    let measured = outcome?;
    let (width, height) = format.dimensions();
    println!(
        "{{\"width\":{width},\"height\":{height},\"encoder\":\"{}\",\"frames\":{},\"total_frames\":{},\"submitted\":{},\"seconds\":{:.6}}}",
        measured.encoder,
        measured.frames,
        session.frames_sent(),
        measured.submitted,
        measured.seconds
    );
    Ok(())
}

fn run(
    session: &CastSession,
    format: VideoFormat,
    seconds: u64,
    fps: u32,
    expected: &str,
) -> Result<Measurement, Error> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match session.status() {
            CastStatus::PinRequired => session.pair("1234".to_owned().into())?,
            CastStatus::Streaming { .. } => break,
            CastStatus::Failed(error) => return Err(Error::Backend(error)),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(Error::Protocol("benchmark startup timed out"));
        }
        thread::sleep(Duration::from_millis(10));
    }
    let selected = session
        .encoding()
        .ok_or(Error::Protocol("benchmark encoder unavailable"))?;
    if selected.backend.as_str() != expected || selected.fallback_reason.is_some() {
        return Err(Error::Protocol(
            "requested benchmark encoder unavailable; fallback is not a GPU result",
        ));
    }
    let (width, height) = format.dimensions();
    let (width, height) = (usize::from(width), usize::from(height));
    let mut image = vec![0; width * height * 4];
    for (index, pixel) in image.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (index % width, index / width);
        let level = if (x / 16 + y / 16) % 2 == 0 { 30 } else { 180 };
        pixel.copy_from_slice(&[level, level, level, 255]);
    }
    let started = Instant::now();
    let mut submitted = 0u32;
    let warmup = Duration::from_secs(2);
    let mut first_sample = None;
    while started.elapsed() < warmup + Duration::from_secs(seconds) {
        if first_sample.is_none() && started.elapsed() >= warmup {
            first_sample = Some((Instant::now(), session.frames_sent()));
        }
        submitted += 1;
        for y in 0..32 {
            for x in 0..width {
                let bit = x * 32 / width;
                let level = if submitted & (1 << bit) == 0 { 0 } else { 255 };
                let offset = (y * width + x) * 4;
                image[offset..offset + 4].copy_from_slice(&[level, level, level, 255]);
            }
        }
        session.submit(image.clone())?;
        if let CastStatus::Failed(error) = session.status() {
            return Err(Error::Backend(error));
        }
        let next = Duration::from_secs_f64(f64::from(submitted) / f64::from(fps));
        thread::sleep(next.saturating_sub(started.elapsed()));
    }
    let (sampled, before) = first_sample.ok_or(Error::Protocol("benchmark sampling never started"))?;
    let elapsed = sampled.elapsed().as_secs_f64();
    let frames = session.frames_sent().saturating_sub(before);
    if frames == 0 {
        return Err(Error::Protocol("benchmark sent no measured frames"));
    }
    Ok(Measurement {
        frames,
        submitted,
        seconds: elapsed,
        encoder: selected.backend.as_str(),
    })
}
