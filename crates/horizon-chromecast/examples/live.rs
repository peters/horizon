//! Live stream probe: `live [--mirror] <ip[:port]> <file.h264> [fps] [audio.aac]`.
//! Plays an Annex B H.264 file (one access unit delimiter per picture) in a
//! loop in real time, with an optional 48 kHz stereo ADTS AAC file as sound.
//! `--mirror` sends it as a screen mirroring session instead of a media stream.
use horizon_chromecast::{AudioFormat, DEFAULT_PORT, LiveCast, LiveOptions, Transport};
use horizon_media::h264::{AnnexBReader, Unit};
use std::{net::SocketAddr, process::ExitCode, time::Duration, time::Instant};

/// Annex B access units, with parameter sets in front of keyframes.
fn access_units(data: &[u8]) -> Result<Vec<(Vec<u8>, bool)>, String> {
    // Without delimiters every picture would merge into one access unit.
    let delimited = data.windows(4).any(|w| w[..3] == [0, 0, 1] && w[3] & 0x1f == 9);
    if !delimited {
        return Err("no access unit delimiters found; encode with aud=1".to_owned());
    }
    let mut reader = AnnexBReader::default();
    let (mut sps, mut pps): (std::sync::Arc<[u8]>, Vec<u8>) = (std::sync::Arc::from(&[][..]), Vec::new());
    let mut access_units = Vec::new();
    // Convert each batch as it completes, so only the rebuilt units accumulate.
    let mut collect = |units: Vec<Unit>| {
        for unit in units {
            match unit {
                Unit::ParameterSets { sps: s, pps: p } => (sps, pps) = (s, p),
                Unit::AccessUnit(access) => {
                    access_units.push((access.to_annexb(&[&sps, &pps]), access.is_keyframe()));
                }
            }
        }
    };
    for chunk in data.chunks(32 * 1024) {
        collect(reader.push(chunk).map_err(|e| e.to_string())?);
    }
    collect(reader.finish().map_err(|e| e.to_string())?);
    Ok(access_units)
}

/// IDR gaps in frames, counting the wrap from the end of the looped file
/// back to its start.
fn keyframe_gaps<T>(units: &[(T, bool)]) -> Vec<u32> {
    let idrs: Vec<usize> = units
        .iter()
        .enumerate()
        .filter(|(_, (_, idr))| *idr)
        .map(|(at, _)| at)
        .collect();
    let (Some(&first), Some(&last)) = (idrs.first(), idrs.last()) else {
        return Vec::new();
    };
    idrs.windows(2)
        .map(|pair| pair[1] - pair[0])
        .chain([units.len() - last + first])
        .filter_map(|gap| u32::try_from(gap).ok())
        .collect()
}

/// A segment length every IDR gap can close: each gap must reach the 90%
/// cut threshold of the longest one, or segments would be dropped.
fn segment_frames<T>(units: &[(T, bool)]) -> Result<u32, String> {
    let gaps = keyframe_gaps(units);
    let longest = *gaps.iter().max().ok_or("the file contains no IDR picture")?;
    let shortest = *gaps.iter().min().ok_or("the file contains no IDR picture")?;
    if u64::from(shortest) * 10 < u64::from(longest) * 9 {
        return Err(format!(
            "IDR pictures are {shortest} to {longest} frames apart; re-encode with a fixed keyframe interval"
        ));
    }
    Ok(longest)
}

/// Raw AAC frames from an ADTS stream (the 7- or 9-byte headers removed).
fn aac_frames(data: &[u8]) -> Result<Vec<&[u8]>, String> {
    let mut frames = Vec::new();
    let mut at = 0;
    while at + 7 <= data.len() {
        let header = &data[at..];
        if header[0] != 0xff || header[1] & 0xf0 != 0xf0 {
            return Err(format!("no ADTS sync word at byte {at}"));
        }
        let length =
            (usize::from(header[3] & 0x03) << 11) | (usize::from(header[4]) << 3) | usize::from(header[5] >> 5);
        let header_length = if header[1] & 0x01 == 0 { 9 } else { 7 };
        let frame = data
            .get(at + header_length..at + length)
            .ok_or("truncated ADTS frame")?;
        frames.push(frame);
        at += length;
    }
    if at != data.len() {
        return Err(format!("truncated ADTS header at byte {at}"));
    }
    if frames.is_empty() {
        return Err("the audio file holds no ADTS frames".to_owned());
    }
    Ok(frames)
}

fn run() -> Result<(), String> {
    let mirror = std::env::args().any(|arg| arg == "--mirror");
    let mut args = std::env::args().skip(1).filter(|arg| arg != "--mirror");
    let usage = "usage: live [--mirror] <ip[:port]> <file.h264> [fps] [audio.aac]";
    let target = args.next().ok_or(usage)?;
    let address: SocketAddr = target
        .parse()
        .or_else(|_| target.parse().map(|ip| SocketAddr::new(ip, DEFAULT_PORT)))
        .map_err(|_| format!("invalid receiver address {target}"))?;
    let data = std::fs::read(args.next().ok_or(usage)?).map_err(|e| e.to_string())?;
    let fps: u32 = args.next().and_then(|f| f.parse().ok()).unwrap_or(30);
    if fps == 0 {
        return Err("fps must be at least 1".to_owned());
    }
    let audio_file = args.next().map(std::fs::read).transpose().map_err(|e| e.to_string())?;
    let audio = audio_file.as_deref().map(aac_frames).transpose()?;
    let units = access_units(&data)?;
    if units.is_empty() {
        return Err("no access unit delimiters found; encode with aud=1".to_owned());
    }
    let frame = Duration::from_secs(1) / fps;
    // A file cannot be asked for keyframes, so segments follow its IDR spacing.
    let segment = frame * segment_frames(&units)?;
    let options = LiveOptions {
        transport: if mirror {
            Transport::Mirror
        } else {
            Transport::Progressive
        },
        segment,
        audio: audio.as_ref().map(|_| AudioFormat {
            sample_rate: 48_000,
            channels: 2,
        }),
        ..LiveOptions::default()
    };
    let aac_frame = Duration::from_secs(1024) / 48_000;
    let mut next_audio = 0u32;
    let live = LiveCast::start(address, options).map_err(|e| e.to_string())?;
    if mirror {
        println!("mirroring to {address}");
    } else {
        println!("serving {} ({:.2} s segments)", live.url(), segment.as_secs_f32());
    }
    let started = Instant::now();
    let mut last_state = None;
    for (index, (unit, keyframe)) in units.iter().cycle().enumerate() {
        let pts = frame * u32::try_from(index).map_err(|_| "stream too long")?;
        if let Some(wait) = pts.checked_sub(started.elapsed()) {
            std::thread::sleep(wait);
        }
        // Video first: it sets the stream's time origin, which audio follows.
        live.push_annexb(unit, pts, *keyframe);
        // Then audio up to this video frame, so both tracks advance together.
        if let Some(frames) = &audio {
            while aac_frame * next_audio <= pts {
                let index = usize::try_from(next_audio).map_err(|_| "stream too long")? % frames.len();
                live.push_aac(frames[index], aac_frame * next_audio);
                next_audio += 1;
            }
        }
        let state = live.state();
        if last_state.as_ref() != Some(&state) {
            println!("{:>6.1}s {state:?}", started.elapsed().as_secs_f32());
            last_state = Some(state.clone());
        }
        match state {
            horizon_chromecast::LiveState::Ended => break,
            horizon_chromecast::LiveState::Failed(reason) => return Err(reason),
            _ => {}
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
