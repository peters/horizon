//! Live stream probe: `live <ip[:port]> <file.h264> [fps]`. Plays an Annex B
//! H.264 file (one access unit delimiter per picture) in a loop in real time.
use horizon_chromecast::{DEFAULT_PORT, LiveCast, LiveOptions};
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
    let (mut sps, mut pps) = (Vec::new(), Vec::new());
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

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: live <ip[:port]> <file.h264> [fps]";
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
    let units = access_units(&data)?;
    if units.is_empty() {
        return Err("no access unit delimiters found; encode with aud=1".to_owned());
    }
    let frame = Duration::from_secs(1) / fps;
    // A file cannot be asked for keyframes, so segments follow its IDR spacing.
    let segment = frame * segment_frames(&units)?;
    let options = LiveOptions {
        segment,
        ..LiveOptions::default()
    };
    let live = LiveCast::start(address, options).map_err(|e| e.to_string())?;
    println!("serving {} ({:.2} s segments)", live.url(), segment.as_secs_f32());
    let started = Instant::now();
    let mut last_state = None;
    for (index, (unit, keyframe)) in units.iter().cycle().enumerate() {
        let pts = frame * u32::try_from(index).map_err(|_| "stream too long")?;
        if let Some(wait) = pts.checked_sub(started.elapsed()) {
            std::thread::sleep(wait);
        }
        live.push_annexb(unit, pts, *keyframe);
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
