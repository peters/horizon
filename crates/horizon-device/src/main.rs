#![forbid(unsafe_code)]
mod dispatch;
mod mcp;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use dispatch::Dispatcher;
use rmcp::ServiceExt;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::PathBuf,
};

#[tokio::main(worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(code) => std::process::ExitCode::from(code),
        Err(error) => {
            println!(
                "{}",
                json!({"ok":false,"error":{"code":"invalid_request","message":error}})
            );
            std::process::ExitCode::from(2)
        }
    }
}
async fn run() -> Result<u8, String> {
    let mut args = std::env::args().skip(1);
    let first = args.next().unwrap_or_default();
    if first == "--help" || first.is_empty() {
        println!(
            "horizon-device --target FILE doctor|screenshot [OUTPUT.png]|act JSON|mcp\nJSON may be '-' to read up to 64 KiB from stdin.\nTarget JSON: {{\"id\":\"lab\",\"endpoint\":{{\"kind\":\"local_x11\",\"display\":\":99\"}}}}\nUse a private directory for FILE; cooperating CLI/MCP commands share FILE's .lock sibling.\nNo default display, application launching, or remote management."
        );
        return Ok(0);
    }
    if first != "--target" {
        return Err("expected --target FILE".into());
    }
    let dispatcher = Dispatcher {
        target_file: PathBuf::from(args.next().ok_or("missing target file")?),
    };
    let command = args.next().ok_or("missing command")?;
    if command == "mcp" {
        if args.next().is_some() {
            return Err("unexpected MCP arguments".into());
        }
        let service = mcp::Server::new(dispatcher)
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| e.to_string())?;
        service.waiting().await.map_err(|e| e.to_string())?;
        return Ok(0);
    }
    let mut extra = args.next();
    if args.next().is_some() {
        return Err("unexpected arguments".into());
    }
    let request = match command.as_str() {
        "act" => {
            let mut text = extra.take().ok_or("act requires JSON")?;
            if text == "-" {
                text.clear();
                std::io::stdin()
                    .take(65537)
                    .read_to_string(&mut text)
                    .map_err(|e| e.to_string())?;
            }
            if text.len() > 65536 {
                return Err("request exceeds 64 KiB".into());
            }
            Some(serde_json::from_str(&text).map_err(|e| format!("invalid action: {e}"))?)
        }
        "doctor" if extra.is_none() => None,
        "screenshot" => None,
        _ => return Err("expected doctor, screenshot, act, or mcp".into()),
    };
    let mut value = dispatcher.call(&command, request);
    if command == "screenshot"
        && value["ok"] == true
        && let Some(path) = extra
    {
        let encoded = value["result"]["image_base64"].as_str().ok_or("missing image")?;
        let image = STANDARD.decode(encoded).map_err(|e| e.to_string())?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&path)
            .and_then(|mut f| f.write_all(&image))
            .map_err(|e| e.to_string())?;
        value["result"]
            .as_object_mut()
            .ok_or("missing result")?
            .remove("image_base64");
        value["result"]["path"] = Value::String(path);
    }
    println!("{value}");
    Ok(u8::from(value["ok"] != true))
}
