//! Receiver probe: `list [seconds]`, `status <ip[:port]>` or `play <ip[:port]> <url> <content-type>`.
use horizon_chromecast::{CastClient, DEFAULT_MEDIA_RECEIVER, DEFAULT_PORT, MediaLoad, MediaStatus, StreamType};
use std::{net::SocketAddr, process::ExitCode, time::Duration};

fn address(arg: Option<String>) -> Result<SocketAddr, String> {
    let arg = arg.ok_or("missing receiver address")?;
    arg.parse()
        .or_else(|_| arg.parse().map(|ip| SocketAddr::new(ip, DEFAULT_PORT)))
        .map_err(|_| format!("invalid receiver address {arg}"))
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("list") => {
            let seconds = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);
            for receiver in horizon_chromecast::discover(Duration::from_secs(seconds)).map_err(|e| e.to_string())? {
                let kind = if receiver.video { "video" } else { "audio-only" };
                println!(
                    "{} {} {} [{kind}] {}",
                    receiver.address, receiver.model, receiver.id, receiver.name
                );
            }
        }
        Some("status") => {
            let client = CastClient::connect(address(args.next())?).map_err(|e| e.to_string())?;
            println!("{:#?}", client.receiver_status().map_err(|e| e.to_string())?);
        }
        Some("play") => {
            let client = CastClient::connect(address(args.next())?).map_err(|e| e.to_string())?;
            let url = args.next().ok_or("missing media URL")?;
            let content_type = args.next().ok_or("missing content type")?;
            let app = client.launch(DEFAULT_MEDIA_RECEIVER).map_err(|e| e.to_string())?;
            let mut media = client.media(&app);
            let status = media
                .load(&MediaLoad {
                    url,
                    content_type,
                    stream_type: StreamType::Buffered,
                    title: Some("horizon-chromecast probe".to_owned()),
                    ..MediaLoad::default()
                })
                .map_err(|e| e.to_string())?;
            println!("loaded: {status:?}");
            for _ in 0..20 {
                if let Some(event) = client.next_event(Duration::from_secs(1)).map_err(|e| e.to_string())? {
                    for status in MediaStatus::from_event(&event) {
                        println!("status: {status:?}");
                    }
                }
            }
            media.stop().map_err(|e| e.to_string())?;
            client.stop_application(&app.session_id).map_err(|e| e.to_string())?;
        }
        _ => {
            return Err(
                "usage: chromecast list [seconds] | status <ip[:port]> | play <ip[:port]> <url> <content-type>"
                    .to_owned(),
            );
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
