//! The rewrites of the bridge, and nothing else.
//!
//! Every JSON-RPC line crosses the bridge unchanged except for these cases:
//!
//! - a `browser_create` that names no `target` opens in the profile of the
//!   bridge, so every page of an agent lands in the profile of the person;
//! - a `set_files` or `drop_files` action names files of this computer, so the
//!   bridge stages them on the remote machine and forwards their remote paths;
//! - a finished `browser_video` or `browser_network` capture names a private file
//!   of the remote machine, so the bridge adds its bounded bytes to the result.
//!
//! A refusal of the attachment case becomes an MCP error result with the same
//! typed reason that a local path gives, because a caller must not wait for an
//! answer that the bridge will not send.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use rmcp::model::{ContentBlock, ResourceContents};
use serde_json::{Value, json};

use super::{MAX_INLINE_BYTES, MAX_STAGED_BYTES};
use horizon_browser_control::remote_host as ssh;

/// One MCP call that the bridge must answer differently from a local call.
pub(super) struct Relay {
    target: String,
    /// The runtime root of the remote host, as the host reported it.
    root: PathBuf,
    /// The profile that a `browser_create` without a target opens in.
    profile: Option<String>,
    calls: Mutex<Calls>,
    next_call: AtomicU64,
}

#[derive(Default)]
struct Calls {
    /// The tool of each in-flight call, by JSON-RPC id.
    tool_by_id: HashMap<String, String>,
    /// The remote staging directory of each in-flight attachment call.
    staging_by_id: HashMap<String, String>,
}

/// A capture whose bytes the bridge can inline, with the media type of the file.
pub(super) struct Capture {
    pub(super) path: String,
    pub(super) mime_type: &'static str,
}

impl Relay {
    pub(super) fn new(target: String, root: PathBuf, profile: Option<String>) -> Self {
        Self {
            target,
            root,
            profile,
            calls: Mutex::new(Calls::default()),
            next_call: AtomicU64::new(0),
        }
    }

    /// The line to forward for one request of the client. A request that names
    /// local files gets the remote paths of their staged copies; a request that
    /// the bridge refuses gets its own error answer, and is not forwarded.
    pub(super) fn request(&self, line: &[u8]) -> Vec<u8> {
        let Ok(mut message) = serde_json::from_slice::<Value>(line) else {
            return line.to_vec();
        };
        if message.get("method").and_then(Value::as_str) != Some("tools/call") {
            return line.to_vec();
        }
        let Some(tool) = tool_name(&message).map(str::to_string) else {
            return line.to_vec();
        };
        let Some(id) = message.get("id").cloned() else {
            return line.to_vec();
        };
        let key = id_key(&id);
        if tool == "browser_create" {
            return self.create_in_profile(message, line);
        }
        if let Some(mime_type) = capture_tool(&tool) {
            self.calls().tool_by_id.insert(key, mime_type.to_string());
            return line.to_vec();
        }
        if tool != "browser_act" {
            return line.to_vec();
        }
        let Some(action) = message
            .pointer("/params/arguments/action")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return line.to_vec();
        };
        if action != "set_files" && action != "drop_files" {
            return line.to_vec();
        }
        let files = message
            .pointer("/params/arguments/files")
            .and_then(Value::as_array)
            .map(|files| {
                files
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if files.is_empty() {
            return line.to_vec();
        }
        match self.stage(&key, &files) {
            Ok(remote_paths) => {
                let replaced = remote_paths.into_iter().map(Value::String).collect::<Vec<_>>();
                if let Some(slot) = message.pointer_mut("/params/arguments/files") {
                    *slot = Value::Array(replaced);
                }
                serde_json::to_vec(&message).unwrap_or_else(|_| line.to_vec())
            }
            Err(reason) => {
                tracing::warn!(%reason, action, "the bridge refused a remote attachment");
                refusal(&id, &format!("browser {action} failed ({reason})"))
            }
        }
    }

    /// The line to forward for one `browser_create`. A create that names no
    /// target opens in the profile of this bridge; a create that names its own
    /// target keeps it, so an agent that knows a profile of the host is not
    /// limited to the one profile of the settings.
    fn create_in_profile(&self, mut message: Value, line: &[u8]) -> Vec<u8> {
        let Some(profile) = self.profile.as_deref() else {
            return line.to_vec();
        };
        let Some(arguments) = message.pointer_mut("/params/arguments").and_then(Value::as_object_mut) else {
            return line.to_vec();
        };
        let names_target = arguments
            .get("target")
            .and_then(Value::as_str)
            .is_some_and(|target| !target.trim().is_empty());
        if names_target {
            return line.to_vec();
        }
        arguments.insert("target".to_string(), Value::String(profile.to_string()));
        serde_json::to_vec(&message).unwrap_or_else(|_| line.to_vec())
    }

    /// The line to forward for one answer of the remote server. A finished
    /// capture gets its bounded bytes added inline, and the staging of a
    /// finished attachment call is removed.
    pub(super) fn response(&self, line: &[u8]) -> Vec<u8> {
        let Ok(mut message) = serde_json::from_slice::<Value>(line) else {
            return line.to_vec();
        };
        let Some(id) = message.get("id").cloned() else {
            return line.to_vec();
        };
        let key = id_key(&id);
        let (tool, staging) = {
            let mut calls = self.calls();
            (calls.tool_by_id.remove(&key), calls.staging_by_id.remove(&key))
        };
        if let Some(directory) = staging {
            ssh::remove_directory(&self.target, &directory);
        }
        let Some(mime_type) = tool.as_deref().and_then(capture_tool) else {
            return line.to_vec();
        };
        if message.pointer("/result/isError").and_then(Value::as_bool) == Some(true) {
            return line.to_vec();
        }
        let Some(capture) = capture_path(&message, mime_type) else {
            return line.to_vec();
        };
        match ssh::read_file(&self.target, Path::new(&capture.path), MAX_INLINE_BYTES) {
            Ok(Some(bytes)) => {
                if add_capture(&mut message, &capture, &bytes).is_err() {
                    return line.to_vec();
                }
                serde_json::to_vec(&message).unwrap_or_else(|_| line.to_vec())
            }
            Ok(None) => {
                tracing::info!(
                    path = %capture.path,
                    limit = MAX_INLINE_BYTES,
                    "the capture is larger than the inline bound and stays a remote path"
                );
                line.to_vec()
            }
            Err(error) => {
                tracing::warn!(%error, path = %capture.path, "the capture stays a remote path");
                line.to_vec()
            }
        }
    }

    /// Remove every staging directory that a call left behind, when the stream
    /// ends before its answer did.
    pub(super) fn remove_leftover_staging(&self) {
        let directories = {
            let mut calls = self.calls();
            std::mem::take(&mut calls.staging_by_id)
        };
        for directory in directories.into_values() {
            ssh::remove_directory(&self.target, &directory);
        }
    }

    fn calls(&self) -> std::sync::MutexGuard<'_, Calls> {
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Stage each local file on the remote machine and give its remote path.
    fn stage(&self, key: &str, files: &[String]) -> Result<Vec<String>, String> {
        let paths = files.iter().map(PathBuf::from).collect::<Vec<_>>();
        let authorized = horizon_browser_control::AttachmentPolicy::from_environment()
            .authorize(&paths)
            .map_err(|error| error.to_string())?;
        let total = authorized
            .iter()
            .map(horizon_browser_control::AuthorizedFile::size)
            .sum::<u64>();
        if total > MAX_STAGED_BYTES {
            return Err(format!(
                "the remote staging of {total} bytes is above the {MAX_STAGED_BYTES} byte budget"
            ));
        }
        let directory = command_directory(&self.root, &self.call_name(key));
        mkdir(&self.target, &directory)?;
        let mut remote_paths = Vec::with_capacity(authorized.len());
        for (index, file) in authorized.iter().enumerate() {
            let name = staged_name(file.path());
            match ssh::stage(&self.target, file.path(), &directory, index, &name) {
                Ok(output) if output.succeeded() => remote_paths.push(format!("{directory}/{index}/{name}")),
                Ok(output) => {
                    ssh::remove_directory(&self.target, &directory);
                    return Err(format!("the remote host refused the copy ({})", output.stderr));
                }
                Err(error) => {
                    ssh::remove_directory(&self.target, &directory);
                    return Err(format!("the remote copy failed ({error})"));
                }
            }
        }
        self.calls().staging_by_id.insert(key.to_string(), directory);
        Ok(remote_paths)
    }

    /// A name that is unique among the live calls of every bridge on that root.
    fn call_name(&self, key: &str) -> String {
        let count = self.next_call.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        format!("call-{}-{count}-{nanos}-{}", std::process::id(), sanitize(key))
    }
}

/// The literal remote path of the staging directory of one call. The root comes
/// from the record of the remote host, so this path is known to the bridge.
fn command_directory(root: &Path, call: &str) -> String {
    root.join(super::STAGING_DIR).join(call).to_string_lossy().into_owned()
}

fn mkdir(target: &str, directory: &str) -> Result<(), String> {
    let parent = Path::new(directory)
        .parent()
        .map_or_else(|| directory.to_string(), |parent| parent.to_string_lossy().into_owned());
    ssh::create_directory(target, &parent, directory).map_err(|error| format!("the remote staging failed ({error})"))
}

/// The media type of a tool whose result names a file that the bridge inlines.
fn capture_tool(tool: &str) -> Option<&'static str> {
    match tool {
        "browser_video" => Some("video/webm"),
        "browser_network" => Some("application/x-ndjson"),
        _ => None,
    }
}

pub(super) fn capture_path(message: &Value, mime_type: &'static str) -> Option<Capture> {
    let path = message
        .pointer("/result/structuredContent/path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())?;
    Some(Capture {
        path: path.to_string(),
        mime_type,
    })
}

/// Add the bytes of a capture as an MCP resource block. The client keeps the
/// private path of the remote host in the structured result; the block is what
/// makes the file usable on this computer.
pub(super) fn add_capture(message: &mut Value, capture: &Capture, bytes: &[u8]) -> Result<(), ()> {
    let block = ContentBlock::resource(
        ResourceContents::blob(
            base64::engine::general_purpose::STANDARD.encode(bytes),
            format!("horizon-browser://capture/{}", resource_name(&capture.path)),
        )
        .with_mime_type(capture.mime_type),
    );
    let block = serde_json::to_value(block).map_err(|_| ())?;
    let content = message
        .pointer_mut("/result/content")
        .and_then(Value::as_array_mut)
        .ok_or(())?;
    content.push(block);
    Ok(())
}

fn tool_name(message: &Value) -> Option<&str> {
    message.pointer("/params/name").and_then(Value::as_str)
}

/// The JSON-RPC id as one map key. An id is a number or a string.
fn id_key(id: &Value) -> String {
    id.to_string()
}

fn refusal(id: &Value, message: &str) -> Vec<u8> {
    let answer = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {"content": [{"type": "text", "text": message}], "isError": true}
    });
    serde_json::to_vec(&answer).unwrap_or_default()
}

/// A file name that the remote shell reads as one word and that the page shows
/// with its original spelling wherever the spelling is safe.
fn staged_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let safe = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(120)
        .collect::<String>();
    if safe.trim_matches('.').is_empty() {
        "file".to_string()
    } else {
        safe
    }
}

fn resource_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| "capture".to_string(), |name| name.to_string_lossy().into_owned())
}

fn sanitize(value: &str) -> String {
    value.chars().filter(char::is_ascii_alphanumeric).take(16).collect()
}

/// Forward each request line of the client, and write the answer of a refusal
/// back to the client itself.
pub(super) fn forward_requests(
    mut client: impl BufRead,
    mut server: impl Write,
    client_out: &mut impl Write,
    relay: &Relay,
) -> io::Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if client.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        let forwarded = relay.request(&line);
        if forwarded == line {
            server.write_all(&line)?;
        } else if is_refusal(&forwarded) {
            client_out.write_all(&forwarded)?;
            client_out.write_all(b"\n")?;
            client_out.flush()?;
            continue;
        } else {
            server.write_all(&forwarded)?;
            server.write_all(b"\n")?;
        }
        server.flush()?;
    }
}

/// Forward each answer line of the server to the client.
pub(super) fn forward_responses(mut server: impl BufRead, client: &mut impl Write, relay: &Relay) -> io::Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if server.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        let forwarded = relay.response(&line);
        client.write_all(&forwarded)?;
        if !forwarded.ends_with(b"\n") {
            client.write_all(b"\n")?;
        }
        client.flush()?;
    }
}

/// A line that the bridge answers itself starts with an error result, and every
/// forwarded line keeps the shape that the client sent.
fn is_refusal(forwarded: &[u8]) -> bool {
    serde_json::from_slice::<Value>(forwarded)
        .ok()
        .and_then(|message| message.pointer("/result/isError").and_then(Value::as_bool))
        == Some(true)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use serde_json::{Value, json};

    use super::{Relay, capture_tool, forward_requests, id_key, staged_name};

    fn relay() -> Relay {
        Relay::new("browser-vm".to_string(), std::path::PathBuf::from("/srv/state"), None)
    }

    #[test]
    fn every_line_that_is_not_one_of_the_two_rewrites_crosses_unchanged() {
        let relay = relay();
        for line in [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}).to_string(),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}).to_string(),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                   "params": {"name": "browser_navigate", "arguments": {"panel_id": "p", "url": "example.com"}}})
            .to_string(),
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                   "params": {"name": "browser_act", "arguments": {"panel_id": "p", "action": "click", "ref": "r"}}})
            .to_string(),
            "not json at all".to_string(),
        ] {
            assert_eq!(relay.request(line.as_bytes()), line.as_bytes(), "{line}");
        }
        let answer = json!({"jsonrpc": "2.0", "id": 3, "result": {"content": [{"type": "text", "text": "ok"}]}});
        assert_eq!(
            relay.response(answer.to_string().as_bytes()),
            answer.to_string().as_bytes(),
            "an answer of a tool without a file stays as it is"
        );
    }

    #[test]
    fn a_capture_tool_is_known_by_the_media_type_of_its_file() {
        assert_eq!(capture_tool("browser_video"), Some("video/webm"));
        assert_eq!(capture_tool("browser_network"), Some("application/x-ndjson"));
        assert_eq!(capture_tool("browser_screenshot"), None);
        assert_eq!(capture_tool("browser_act"), None);
    }

    #[test]
    fn a_create_without_a_target_opens_in_the_profile_of_the_bridge() {
        let relay = Relay::new(
            "browser-vm".to_string(),
            std::path::PathBuf::from("/srv/state"),
            Some("mail".to_string()),
        );
        let request = json!({"jsonrpc": "2.0", "id": 11, "method": "tools/call",
             "params": {"name": "browser_create", "arguments": {"url": "https://example.com"}}});
        let forwarded: Value =
            serde_json::from_slice(&relay.request(request.to_string().as_bytes())).expect("a request line");
        assert_eq!(
            forwarded.pointer("/params/arguments/target"),
            Some(&json!("mail")),
            "the create lands in the profile that the person chose"
        );
        assert_eq!(
            forwarded.pointer("/params/arguments/url"),
            Some(&json!("https://example.com")),
            "the rest of the call is unchanged"
        );

        let blank = json!({"jsonrpc": "2.0", "id": 12, "method": "tools/call",
             "params": {"name": "browser_create", "arguments": {"target": "  ", "url": "https://example.com"}}});
        let forwarded: Value =
            serde_json::from_slice(&relay.request(blank.to_string().as_bytes())).expect("a request line");
        assert_eq!(
            forwarded.pointer("/params/arguments/target"),
            Some(&json!("mail")),
            "a blank target names no profile"
        );
    }

    #[test]
    fn a_create_that_names_its_own_target_is_left_alone() {
        let relay = Relay::new(
            "browser-vm".to_string(),
            std::path::PathBuf::from("/srv/state"),
            Some("mail".to_string()),
        );
        let request = json!({"jsonrpc": "2.0", "id": 13, "method": "tools/call",
             "params": {"name": "browser_create", "arguments": {"target": "chat", "url": "https://example.com"}}})
        .to_string();
        assert_eq!(
            relay.request(request.as_bytes()),
            request.as_bytes(),
            "an agent that names a profile of the host keeps it"
        );

        let bridge_without_profile = Relay::new("browser-vm".to_string(), std::path::PathBuf::from("/srv/state"), None);
        let target_less = json!({"jsonrpc": "2.0", "id": 14, "method": "tools/call",
             "params": {"name": "browser_create", "arguments": {"url": "https://example.com"}}})
        .to_string();
        assert_eq!(
            bridge_without_profile.request(target_less.as_bytes()),
            target_less.as_bytes(),
            "a bridge without a profile of its own rewrites no create"
        );
    }

    #[test]
    fn a_refused_stage_answers_the_client_with_the_typed_reason_and_sends_nothing() {
        let relay = relay();
        let request = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
             "params": {"name": "browser_act",
                        "arguments": {"panel_id": "p", "action": "set_files", "ref": "r",
                                      "files": ["/definitely/not/a/file.pdf"]}}})
        .to_string();
        let mut server = Vec::new();
        let mut client = Vec::new();
        forward_requests(
            Cursor::new(format!("{request}\n").into_bytes()),
            &mut server,
            &mut client,
            &relay,
        )
        .expect("relay");
        assert!(server.is_empty(), "a refused request never reaches the remote server");
        let answer: Value = serde_json::from_slice(&client).expect("an answer line");
        assert_eq!(answer["id"], json!(7));
        assert_eq!(answer["result"]["isError"], json!(true));
        let reason = answer["result"]["content"][0]["text"].as_str().expect("text");
        assert!(
            reason.starts_with("browser set_files failed ("),
            "the refusal keeps the tool prefix: {reason}"
        );
        assert!(
            reason.contains("attachment") || reason.contains("root"),
            "the refusal keeps the reason of the local path policy: {reason}"
        );
    }

    #[test]
    fn a_staged_file_keeps_its_spelling_and_a_name_of_the_call_is_a_safe_word() {
        assert_eq!(
            staged_name(std::path::Path::new("/work/report 2026.pdf")),
            "report_2026.pdf"
        );
        assert_eq!(staged_name(std::path::Path::new("/work/..")), "file");
        assert_eq!(staged_name(std::path::Path::new("/work/.hidden")), ".hidden");
        let relay = Relay::new("target".to_string(), std::path::PathBuf::from("/srv/state"), None);
        let name = relay.call_name(&id_key(&json!("a/../b 'quoted'")));
        assert!(name.starts_with("call-"), "{name}");
        assert!(
            name.chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-'),
            "the call name is one shell word: {name}"
        );
    }
}
