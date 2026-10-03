//! Explicit receiver pairing probe. PIN is read from stdin, never argv.
use horizon_cast::{Pairing, VideoFormat};
use std::{io, net::SocketAddr, process::ExitCode};
use zeroize::Zeroizing;

fn run() -> Result<(), String> {
    if std::env::args().nth(1).as_deref() == Some("--discover") {
        for receiver in horizon_cast::discover().map_err(|e| e.to_string())? {
            println!("{} {} {}", receiver.id, receiver.address, receiver.name);
        }
        return Ok(());
    }
    let address: SocketAddr = std::env::args()
        .nth(1)
        .ok_or("usage: pair <receiver-ip:port>")?
        .parse()
        .map_err(|_| "invalid address")?;
    let pairing = Pairing::begin(address).map_err(|e| e.to_string())?;
    println!("PIN_REQUIRED: enter the new code displayed on the TV");
    let mut pin = Zeroizing::new(String::new());
    io::stdin().read_line(&mut pin).map_err(|e| e.to_string())?;
    while pin.ends_with(['\r', '\n']) {
        pin.pop();
    }
    let mut receiver = pairing.finish(pin).map_err(|e| e.to_string())?;
    let info = receiver.info().map_err(|e| e.to_string())?;
    let fields = info.as_dictionary().ok_or("invalid receiver info")?;
    println!(
        "AUTHENTICATED: encrypted receiver info received; model={:?} version={:?}",
        fields.get("model"),
        fields.get("sourceVersion")
    );
    if std::env::args().any(|arg| arg == "--mirror") {
        let mut mirror = receiver.mirror(VideoFormat::default()).map_err(|e| e.to_string())?;
        println!("MIRROR_READY: configure SPS/PPS and send complete access units over stdin");
        loop {
            let mut line = String::new();
            if io::stdin().read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                break;
            }
            let mut fields = line.split_whitespace();
            let command = fields.next().unwrap_or("stop");
            if command == "stop" {
                break;
            }
            let data = fields.map(hex).collect::<Result<Vec<_>, _>>()?;
            match command {
                "config" if data.len() == 2 => mirror.configure(&data[0], &data[1]),
                "frame" => mirror.send(&data.iter().map(Vec::as_slice).collect::<Vec<_>>()),
                _ => return Err("expected config, frame or stop".into()),
            }
            .map_err(|e| e.to_string())?;
        }
        mirror.stop();
        println!("STOPPED");
    }
    Ok(())
}
fn hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) || text.len() > 16 * 1024 * 1024 {
        return Err("invalid NAL hex".into());
    }
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let pair = std::str::from_utf8(pair).map_err(|_| "invalid hex")?;
            u8::from_str_radix(pair, 16).map_err(|_| "invalid hex".into())
        })
        .collect()
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
