//! Shared command-line and MCP entry point for device adapters.
mod dispatch;
mod mcp;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use dispatch::{Command, Dispatcher};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::PathBuf,
};

pub type ResizeFactory = fn(&crate::Target) -> crate::Result<Box<dyn crate::ResizeBackend>>;

pub async fn run() -> std::process::ExitCode {
    run_inner(None).await
}

/// Run the same CLI/MCP contract with an optional desktop resize transport.
pub async fn run_with_resize(factory: ResizeFactory) -> std::process::ExitCode {
    run_inner(Some(factory)).await
}

async fn run_inner(factory: Option<ResizeFactory>) -> std::process::ExitCode {
    match execute(factory).await {
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
async fn execute(factory: Option<ResizeFactory>) -> Result<u8, String> {
    let mut args = std::env::args();
    let executable = args.next().unwrap_or_default();
    let program = std::path::Path::new(&executable)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("horizon-device");
    let first = args.next().unwrap_or_default();
    if first == "--help" || first.is_empty() {
        println!(
            "{program} --target FILE doctor|resize JSON|screenshot [OUTPUT] [--options JSON]|act JSON|mcp\nJSON may be '-' to read up to 64 KiB from stdin.\nTarget JSON: {{\"id\":\"lab\",\"endpoint\":{{\"kind\":\"local_x11\",\"display\":\":99\"}}}}\nUse a private directory for FILE; cooperating CLI/MCP commands share FILE's .lock sibling.\nNo default display, application launching, or remote management."
        );
        return Ok(0);
    }
    if first != "--target" {
        return Err("expected --target FILE".into());
    }
    let dispatcher = Dispatcher {
        target_file: PathBuf::from(args.next().ok_or("missing target file")?),
        resize_factory: factory,
    };
    let command = args.next().ok_or("missing command")?;
    if command == "mcp" {
        if args.next().is_some() {
            eprintln!("unexpected MCP arguments");
            return Ok(2);
        }
        return Ok(mcp::serve(dispatcher).await);
    }
    let mut output = None;
    let request = match command.as_str() {
        "act" => Command::Act(
            serde_json::from_str(&read_final_json(&mut args, "act requires JSON")?)
                .map_err(|error| format!("invalid action: {error}"))?,
        ),
        "doctor" => Command::Doctor,
        "resize" => Command::Resize(
            serde_json::from_str(&read_final_json(&mut args, "resize requires JSON")?)
                .map_err(|error| format!("invalid resize: {error}"))?,
        ),
        "screenshot" => {
            let mut options = crate::CaptureOptions::default();
            if let Some(first) = args.next() {
                let option_flag = if first == "--options" {
                    Some(first)
                } else {
                    output = Some(first);
                    args.next()
                };
                if let Some(flag) = option_flag {
                    if flag != "--options" {
                        return Err("expected --options JSON after output path".into());
                    }
                    options = serde_json::from_str(&read_final_json(&mut args, "missing capture options")?)
                        .map_err(|error| format!("invalid capture options: {error}"))?;
                }
            }
            Command::Screenshot(options)
        }
        _ => return Err("expected doctor, screenshot, act, resize, or mcp".into()),
    };
    if args.next().is_some() {
        return Err("unexpected arguments".into());
    }
    let response = tokio::task::spawn_blocking(move || dispatcher.call(request))
        .await
        .map_err(|_| "device worker failed; observe before retrying".to_owned())?;
    deliver(response, output, &mut std::io::stdout().lock())
}

fn deliver(mut response: dispatch::Response, output: Option<String>, writer: &mut impl Write) -> Result<u8, String> {
    let value = &mut response.value;
    if value["ok"] == true
        && let Some(path) = output
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
    writeln!(writer, "{value}").map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())?;
    let code = u8::from(value["ok"] != true);
    if let Err(error) = response.complete_observation() {
        eprintln!("screenshot delivered but input remains gated: {error}");
        return Ok(1);
    }
    Ok(code)
}

fn read_final_json(args: &mut impl Iterator<Item = String>, missing: &str) -> Result<String, String> {
    let text = args.next().ok_or(missing)?;
    if args.next().is_some() {
        return Err("unexpected arguments".into());
    }
    read_json(text)
}

fn read_json(mut text: String) -> Result<String, String> {
    if text == "-" {
        text.clear();
        std::io::stdin()
            .take(65537)
            .read_to_string(&mut text)
            .map_err(|error| error.to_string())?;
    }
    if text.len() > 65536 {
        return Err("request exceeds 64 KiB".into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn screenshot(directory: &std::path::Path) -> std::io::Result<dispatch::Response> {
        let marker = directory.join("target.resize-observe");
        std::fs::write(&marker, b"")?;
        let lock = File::create(directory.join("target.lock"))?;
        lock.lock()?;
        Ok(dispatch::Response::new(
            json!({"ok": true, "result": {"image_base64": "AQID"}}),
            Some(lock),
            Some(marker),
        ))
    }

    #[test]
    fn failed_file_delivery_keeps_the_observation_gate() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let output = directory.path().join("existing.png");
        std::fs::write(&output, b"original")?;
        let response = screenshot(directory.path())?;
        assert!(deliver(response, Some(output.to_string_lossy().into_owned()), &mut Vec::new()).is_err());
        assert!(directory.path().join("target.resize-observe").exists());
        assert_eq!(std::fs::read(output)?, b"original");
        Ok(())
    }

    struct FailedFlush;
    impl Write for FailedFlush {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("closed output"))
        }
    }

    #[test]
    fn failed_stdout_delivery_keeps_the_observation_gate() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let response = screenshot(directory.path())?;
        assert!(deliver(response, None, &mut FailedFlush).is_err());
        assert!(directory.path().join("target.resize-observe").exists());
        Ok(())
    }

    #[test]
    fn cancelled_response_keeps_the_gate_and_holds_the_lock_until_dropped() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let response = screenshot(directory.path())?;
        let competing = File::open(directory.path().join("target.lock"))?;
        assert!(matches!(competing.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        drop(response);
        competing.try_lock()?;
        assert!(directory.path().join("target.resize-observe").exists());
        Ok(())
    }

    #[test]
    fn delivered_screenshot_releases_the_observation_gate() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let response = screenshot(directory.path())?;
        let mut output = Vec::new();
        assert_eq!(deliver(response, None, &mut output)?, 0);
        assert!(!directory.path().join("target.resize-observe").exists());
        let value: Value = serde_json::from_slice(&output)?;
        assert_eq!(value["result"]["image_base64"], "AQID");
        Ok(())
    }
}
