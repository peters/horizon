//! Live stream probe: `live <ip[:port]> <file.h264> [fps]`. Plays an Annex B
//! H.264 file (one access unit delimiter per picture) in a loop in real time.
use horizon_chromecast::{DEFAULT_PORT, LiveCast, LiveOptions};
use std::{net::SocketAddr, process::ExitCode, time::Duration, time::Instant};

fn access_units(data: &[u8]) -> Vec<(&[u8], bool)> {
    let mut starts: Vec<usize> = data
        .windows(5)
        .enumerate()
        .filter(|(_, w)| w[..4] == [0, 0, 0, 1] && w[4] & 0x1f == 9)
        .map(|(at, _)| at)
        .collect();
    starts.push(data.len());
    starts
        .windows(2)
        .map(|pair| {
            let unit = &data[pair[0]..pair[1]];
            let idr = unit.windows(4).any(|w| w[..3] == [0, 0, 1] && w[3] & 0x1f == 5);
            (unit, idr)
        })
        .collect()
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
    let units = access_units(&data);
    if units.is_empty() {
        return Err("no access unit delimiters found; encode with aud=1".to_owned());
    }
    let live = LiveCast::start(address, LiveOptions::default()).map_err(|e| e.to_string())?;
    println!("serving {}", live.url());
    let frame = Duration::from_secs(1) / fps;
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
        if matches!(
            state,
            horizon_chromecast::LiveState::Ended | horizon_chromecast::LiveState::Failed(_)
        ) {
            break;
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
