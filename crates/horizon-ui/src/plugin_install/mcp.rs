//! Process-scoped Horizon browser MCP attachments for agents that are not
//! wired through Claude `--plugin-dir` or Codex `-c` launch flags.
//!
//! Pi, Grok, `OpenCode`, and Antigravity read MCP servers from their own config
//! files. Horizon writes a `horizon-browser` entry when this host starts and
//! removes only that entry when the last host for the file exits.

use std::ffi::OsStr;
use std::fs::{OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

const SERVER_NAME: &str = "horizon-browser";
const BROWSER_MCP_ARG: &str = "--browser-mcp";
const LEASE_ENV: &str = "HORIZON_BROWSER_MCP_LEASE";
const LEASES_DIR: &str = ".horizon-leases";
const GROK_BEGIN: &str = "# BEGIN HORIZON-MANAGED MCP horizon-browser";
const GROK_END: &str = "# END HORIZON-MANAGED MCP horizon-browser";
const GROK_TABLE: &str = "[mcp_servers.horizon-browser]";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum McpConfigKind {
    Pi,
    Antigravity,
    GrokToml,
}

pub(super) struct McpAttachmentLease {
    path: PathBuf,
    kind: McpConfigKind,
    live_path: PathBuf,
    live_lock: Option<std::fs::File>,
}

pub(super) fn bind_browser_mcp_attachments(
    host_id: &OsStr,
    mcp_command: &Path,
    user_home: Option<&Path>,
    grok_home: Option<&Path>,
) -> Vec<McpAttachmentLease> {
    let Ok(command) = mcp_command_string(mcp_command) else {
        tracing::warn!(path = %mcp_command.display(), "Horizon executable path is not valid UTF-8");
        return Vec::new();
    };

    let mut leases = Vec::new();
    if let Some(home) = user_home {
        push_attachment(
            &mut leases,
            host_id,
            home.join(".pi").join("agent").join("mcp.json"),
            McpConfigKind::Pi,
            &command,
        );
        push_attachment(
            &mut leases,
            host_id,
            home.join(".gemini").join("config").join("mcp_config.json"),
            McpConfigKind::Antigravity,
            &command,
        );
    }
    if let Some(grok_home) = grok_home {
        push_attachment(
            &mut leases,
            host_id,
            grok_home.join("config.toml"),
            McpConfigKind::GrokToml,
            &command,
        );
    }
    leases
}

pub(super) fn release_mcp_attachments(leases: &mut [McpAttachmentLease]) {
    for lease in leases {
        let Some(coord_dir) = config_coord_dir(&lease.path) else {
            continue;
        };
        let coord = match lock_coord(&coord_dir) {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!(path = %coord_dir.display(), %error, "failed to lock MCP attachment cleanup");
                continue;
            }
        };
        drop(lease.live_lock.take());
        let live_dir = lease.live_path.parent().unwrap_or(coord_dir.as_path());
        let peer_command = match live_peer_command(live_dir, &lease.live_path) {
            Ok(command) => command,
            Err(error) => {
                tracing::warn!(path = %live_dir.display(), %error, "failed to inspect MCP attachment leases");
                None
            }
        };
        remove_live_files(&lease.live_path);
        match peer_command {
            Some(command) if !command.is_empty() => {
                if let Err(error) = attach_config(&lease.path, lease.kind, &command) {
                    tracing::warn!(path = %lease.path.display(), %error, "failed to retarget Horizon browser MCP to a live host");
                }
            }
            Some(_) => {}
            None => detach_attachment(lease),
        }
        drop(coord);
    }
}

impl Drop for McpAttachmentLease {
    fn drop(&mut self) {
        drop(self.live_lock.take());
        remove_live_files(&self.live_path);
    }
}

fn push_attachment(
    leases: &mut Vec<McpAttachmentLease>,
    host_id: &OsStr,
    path: PathBuf,
    kind: McpConfigKind,
    command: &str,
) {
    match acquire_attachment(host_id, path, kind, command) {
        Ok(Some(lease)) => leases.push(lease),
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "failed to attach Horizon browser MCP"),
    }
}

fn acquire_attachment(
    host_id: &OsStr,
    path: PathBuf,
    kind: McpConfigKind,
    command: &str,
) -> io::Result<Option<McpAttachmentLease>> {
    let coord_dir = config_coord_dir(&path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("MCP config path has no parent: {}", path.display()),
        )
    })?;
    let live_dir = config_live_dir(&path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("MCP config path has no name: {}", path.display()),
        )
    })?;
    std::fs::create_dir_all(&live_dir)?;
    let coord = lock_coord(&coord_dir)?;
    let live_path = {
        let mut name = host_id.to_os_string();
        name.push(".live");
        live_dir.join(name)
    };
    let live_lock = open_lock_file(&live_path)?;
    match live_lock.try_lock() {
        Ok(()) => {}
        Err(error) => {
            drop(coord);
            remove_live_files(&live_path);
            return Err(match error {
                TryLockError::WouldBlock => io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("MCP config is already leased: {}", path.display()),
                ),
                TryLockError::Error(error) => error,
            });
        }
    }
    if let Err(error) = write_text_atomic(&command_sidecar_path(&live_path), command) {
        tracing::warn!(path = %live_path.display(), %error, "failed to record Horizon browser MCP command");
        drop(coord);
        drop(live_lock);
        remove_live_files(&live_path);
        return Err(error);
    }
    let peer_live = match live_peer_command(&live_dir, &live_path) {
        Ok(command) => command.is_some(),
        Err(error) => {
            drop(coord);
            drop(live_lock);
            remove_live_files(&live_path);
            return Err(error);
        }
    };
    let attached = if peer_live {
        true
    } else {
        match attach_config(&path, kind, command) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "failed to write Horizon browser MCP attachment");
                false
            }
        }
    };
    drop(coord);
    if !attached {
        drop(live_lock);
        remove_live_files(&live_path);
        return Ok(None);
    }
    Ok(Some(McpAttachmentLease {
        path,
        kind,
        live_path,
        live_lock: Some(live_lock),
    }))
}

fn attach_config(path: &Path, kind: McpConfigKind, command: &str) -> io::Result<()> {
    match kind {
        McpConfigKind::Pi => merge_json_server(path, json_stdio_server(command)),
        McpConfigKind::Antigravity => merge_json_server(path, json_antigravity_server(command)),
        McpConfigKind::GrokToml => upsert_grok_server(path, command),
    }
}

fn detach_attachment(lease: &McpAttachmentLease) {
    if let Err(error) = match lease.kind {
        McpConfigKind::Pi | McpConfigKind::Antigravity => remove_json_server(&lease.path),
        McpConfigKind::GrokToml => remove_grok_server(&lease.path),
    } {
        tracing::warn!(path = %lease.path.display(), %error, "failed to detach Horizon browser MCP");
    }
}

fn json_stdio_server(command: &str) -> Value {
    serde_json::json!({
        "command": command,
        "args": [BROWSER_MCP_ARG],
        "env": {
            LEASE_ENV: "1",
        },
    })
}

fn json_antigravity_server(command: &str) -> Value {
    serde_json::json!({
        "command": command,
        "args": [BROWSER_MCP_ARG],
        "disabled": false,
        "env": {
            LEASE_ENV: "1",
        },
    })
}

fn merge_json_server(path: &Path, server: Value) -> io::Result<()> {
    let mut root = read_json_object(path)?;
    let container = json_object_at(&mut root, &["mcpServers"])?;
    if json_server_is_unmanaged(container.get(SERVER_NAME)) {
        tracing::warn!(path = %path.display(), "leaving user-owned horizon-browser MCP server in place");
        return Ok(());
    }
    container.insert(SERVER_NAME.to_string(), server);
    write_text_atomic(path, &pretty_json(&root)?)
}

fn remove_json_server(path: &Path) -> io::Result<()> {
    let Some(contents) = read_existing(path)? else {
        return Ok(());
    };
    if contents.trim().is_empty() {
        return Ok(());
    }
    let mut root = parse_json_object(&contents, path)?;
    let unmanaged = root
        .get("mcpServers")
        .and_then(Value::as_object)
        .is_some_and(|servers| json_server_is_unmanaged(servers.get(SERVER_NAME)));
    if unmanaged {
        return Ok(());
    }
    remove_json_key(&mut root, &["mcpServers"]);
    if root.is_empty() {
        root.insert("mcpServers".to_string(), Value::Object(Map::new()));
    }
    write_text_atomic(path, &pretty_json(&root)?)
}

fn read_json_object(path: &Path) -> io::Result<Map<String, Value>> {
    match read_existing(path)? {
        None => Ok(Map::new()),
        Some(contents) if contents.trim().is_empty() => Ok(Map::new()),
        Some(contents) => parse_json_object(&contents, path),
    }
}

fn json_server_is_unmanaged(server: Option<&Value>) -> bool {
    let Some(server) = server else {
        return false;
    };
    !matches!(
        server
            .get("env")
            .and_then(Value::as_object)
            .and_then(|env| env.get(LEASE_ENV)),
        Some(Value::String(flag)) if flag == "1"
    )
}

fn parse_json_object(contents: &str, path: &Path) -> io::Result<Map<String, Value>> {
    match serde_json::from_str::<Value>(contents) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("MCP config is not a JSON object: {}", path.display()),
        )),
        Err(error) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("MCP config is not valid JSON ({}): {}", path.display(), error),
        )),
    }
}

fn json_object_at<'a>(root: &'a mut Map<String, Value>, keys: &[&str]) -> io::Result<&'a mut Map<String, Value>> {
    let mut current = root;
    for key in keys {
        if !current.contains_key(*key) {
            current.insert((*key).to_string(), Value::Object(Map::new()));
        }
        let next = current
            .get_mut(*key)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("MCP config is missing `{key}`")))?;
        let Some(object) = next.as_object_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("MCP config `{key}` is not a JSON object"),
            ));
        };
        current = object;
    }
    Ok(current)
}

fn remove_json_key(root: &mut Map<String, Value>, keys: &[&str]) {
    let Some((last, parents)) = keys.split_last() else {
        return;
    };
    let mut current = root;
    for key in parents {
        match current.get_mut(*key) {
            Some(Value::Object(next)) => current = next,
            _ => return,
        }
    }
    let empty = match current.get_mut(*last) {
        Some(Value::Object(servers)) => {
            servers.remove(SERVER_NAME);
            servers.is_empty()
        }
        _ => false,
    };
    if empty {
        current.remove(*last);
    }
}

fn pretty_json(root: &Map<String, Value>) -> io::Result<String> {
    let mut encoded = serde_json::to_string_pretty(&Value::Object(root.clone())).map_err(io::Error::other)?;
    if !encoded.ends_with('\n') {
        encoded.push('\n');
    }
    Ok(encoded)
}

fn upsert_grok_server(path: &Path, command: &str) -> io::Result<()> {
    let contents = read_existing(path)?.unwrap_or_default();
    if has_unmanaged_grok_server(&contents) {
        tracing::warn!(
            path = %path.display(),
            "leaving user-owned [mcp_servers.horizon-browser] in place"
        );
        return Ok(());
    }
    write_text_atomic(
        path,
        &replace_or_append_grok_block(&contents, &grok_managed_block(command)),
    )
}

fn remove_grok_server(path: &Path) -> io::Result<()> {
    let Some(contents) = read_existing(path)? else {
        return Ok(());
    };
    let stripped = strip_grok_managed_block(&contents);
    if stripped.trim().is_empty() {
        return write_text_atomic(path, "");
    }
    if stripped == contents {
        return Ok(());
    }
    write_text_atomic(path, &stripped)
}

fn grok_managed_block(command: &str) -> String {
    format!(
        "{GROK_BEGIN}\n{GROK_TABLE}\ncommand = {}\nargs = [{BROWSER_MCP_ARG:?}]\nenabled = true\nenv = {{ HORIZON_BROWSER_ACTOR = \"${{HORIZON_BROWSER_ACTOR}}\", HORIZON_BROWSER_HOST_INSTANCE = \"${{HORIZON_BROWSER_HOST_INSTANCE}}\" }}\n{GROK_END}\n",
        toml_string(command),
    )
}

fn replace_or_append_grok_block(contents: &str, block: &str) -> String {
    if let Some((start, end)) = grok_managed_span(contents) {
        let mut replaced = String::new();
        replaced.push_str(&contents[..start]);
        replaced.push_str(block);
        let rest = contents[end..].trim_start_matches(['\r', '\n']);
        if !rest.is_empty() {
            if !replaced.ends_with('\n') {
                replaced.push('\n');
            }
            replaced.push('\n');
            replaced.push_str(rest);
        }
        return replaced;
    }
    if contents.trim().is_empty() {
        return block.to_string();
    }
    let mut appended = contents.trim_end().to_string();
    appended.push_str("\n\n");
    appended.push_str(block);
    appended
}

fn strip_grok_managed_block(contents: &str) -> String {
    let Some((start, end)) = grok_managed_span(contents) else {
        return contents.to_string();
    };
    let mut stripped = String::new();
    stripped.push_str(contents[..start].trim_end());
    let rest = contents[end..].trim_start_matches(['\r', '\n']);
    if !rest.is_empty() {
        if !stripped.is_empty() {
            stripped.push_str("\n\n");
        }
        stripped.push_str(rest);
    }
    if !stripped.is_empty() && !stripped.ends_with('\n') {
        stripped.push('\n');
    }
    stripped
}

fn grok_managed_span(contents: &str) -> Option<(usize, usize)> {
    let start = contents.find(GROK_BEGIN)?;
    let from_start = &contents[start..];
    let end = start + from_start.find(GROK_END)? + GROK_END.len();
    Some((start, end))
}

fn has_unmanaged_grok_server(contents: &str) -> bool {
    grok_remainder_has_horizon_browser(&strip_grok_managed_block(contents))
}

fn grok_remainder_has_horizon_browser(rest: &str) -> bool {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Ok(table) = trimmed.parse::<toml::Table>() else {
        return true;
    };
    table
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .is_some_and(|servers| servers.contains_key(SERVER_NAME))
}

fn toml_string(value: &str) -> String {
    let mut encoded = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => encoded.push_str("\\\\"),
            '"' => encoded.push_str("\\\""),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            '\t' => encoded.push_str("\\t"),
            character => encoded.push(character),
        }
    }
    encoded.push('"');
    encoded
}

fn mcp_command_string(command: &Path) -> io::Result<String> {
    durable_mcp_command(command)
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Horizon executable path is not valid UTF-8",
            )
        })
}

fn durable_mcp_command(command: &Path) -> PathBuf {
    std::fs::canonicalize(command).unwrap_or_else(|_| command.to_path_buf())
}

fn config_coord_dir(path: &Path) -> Option<PathBuf> {
    path.parent().map(|parent| parent.join(LEASES_DIR))
}

fn config_live_dir(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    Some(config_coord_dir(path)?.join(name))
}

fn lock_coord(leases_dir: &Path) -> io::Result<std::fs::File> {
    std::fs::create_dir_all(leases_dir)?;
    let file = open_lock_file(&leases_dir.join(".lock"))?;
    file.lock()?;
    Ok(file)
}

fn live_peer_command(live_dir: &Path, current: &Path) -> io::Result<Option<String>> {
    let entries = match std::fs::read_dir(live_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut live_command = None;
    let mut found_live = false;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path == current || !is_live_lock(entry.file_name().as_os_str()) {
            continue;
        }
        let file = open_lock_file(&path)?;
        match file.try_lock() {
            Ok(()) => {
                drop(file);
                remove_live_files(&path);
            }
            Err(TryLockError::WouldBlock) => {
                found_live = true;
                if live_command.is_none() {
                    live_command = read_command_sidecar(&path);
                }
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
    if found_live {
        Ok(Some(live_command.unwrap_or_default()))
    } else {
        Ok(None)
    }
}

fn command_sidecar_path(live_path: &Path) -> PathBuf {
    live_path.with_extension("command")
}

fn read_command_sidecar(live_path: &Path) -> Option<String> {
    let path = command_sidecar_path(live_path);
    let mut file = std::fs::File::open(&path).ok()?;
    let mut command = String::new();
    file.read_to_string(&mut command).ok()?;
    let command = command.trim();
    (!command.is_empty()).then(|| command.to_string())
}

fn remove_live_files(live_path: &Path) {
    for path in [live_path, &command_sidecar_path(live_path)] {
        if let Err(error) = std::fs::remove_file(path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), %error, "failed to remove MCP attachment lock file");
        }
    }
}

fn is_live_lock(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    !name.starts_with('.')
        && Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("live"))
}

fn open_lock_file(path: &Path) -> io::Result<std::fs::File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
}

fn read_existing(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_text_atomic(path: &Path, content: &str) -> io::Result<()> {
    let destination = write_destination(path)?;
    if std::fs::read_to_string(&destination).ok().as_deref() == Some(content) {
        return Ok(());
    }
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temp_file = tempfile::NamedTempFile::new_in(parent)?;
    temp_file.write_all(content.as_bytes())?;
    temp_file.flush()?;
    temp_file.persist(&destination).map_err(|error| error.error)?;
    Ok(())
}

fn write_destination(path: &Path) -> io::Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = std::fs::read_link(path)?;
            if target.is_absolute() {
                Ok(target)
            } else {
                Ok(path.parent().unwrap_or_else(|| Path::new(".")).join(target))
            }
        }
        Ok(_) => Ok(path.to_path_buf()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use super::{
        LEASES_DIR, SERVER_NAME, bind_browser_mcp_attachments, durable_mcp_command, grok_managed_block,
        has_unmanaged_grok_server, release_mcp_attachments, replace_or_append_grok_block, strip_grok_managed_block,
    };

    #[test]
    fn attach_writes_horizon_browser_into_each_agent_config() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let leases = bind_browser_mcp_attachments(
            OsStr::new("host-a"),
            Path::new("/opt/horizon"),
            Some(&home),
            Some(&home.join(".grok")),
        );
        assert_eq!(leases.len(), 3);

        let pi = std::fs::read_to_string(home.join(".pi/agent/mcp.json")).expect("pi config");
        assert!(pi.contains(SERVER_NAME));
        assert!(pi.contains("/opt/horizon"));
        assert!(pi.contains("--browser-mcp"));
        assert!(pi.contains("HORIZON_BROWSER_MCP_LEASE"));
        assert!(!home.join(".config/opencode/opencode.json").exists());

        let antigravity =
            std::fs::read_to_string(home.join(".gemini/config/mcp_config.json")).expect("antigravity config");
        assert!(antigravity.contains("\"disabled\": false"));
        assert!(antigravity.contains("/opt/horizon"));

        let grok = std::fs::read_to_string(home.join(".grok/config.toml")).expect("grok config");
        assert!(grok.contains("[mcp_servers.horizon-browser]"));
        assert!(grok.contains("command = \"/opt/horizon\""));
        assert!(grok.contains("# BEGIN HORIZON-MANAGED MCP horizon-browser"));
    }

    #[test]
    fn attach_preserves_unrelated_servers() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let pi_path = home.join(".pi/agent/mcp.json");
        std::fs::create_dir_all(pi_path.parent().expect("pi parent")).expect("pi dir");
        std::fs::write(
            &pi_path,
            r#"{
  "mcpServers": {
    "github": { "url": "https://api.githubcopilot.com/mcp" }
  },
  "imports": ["claude-code"]
}
"#,
        )
        .expect("seed pi");
        let grok_path = home.join(".grok/config.toml");
        std::fs::create_dir_all(grok_path.parent().expect("grok parent")).expect("grok dir");
        std::fs::write(&grok_path, "[models]\ndefault = \"grok-4.5\"\n").expect("seed grok");

        let _leases = bind_browser_mcp_attachments(
            OsStr::new("host-a"),
            Path::new("/opt/horizon"),
            Some(&home),
            Some(&home.join(".grok")),
        );

        let pi = std::fs::read_to_string(pi_path).expect("pi after attach");
        assert!(pi.contains("github"));
        assert!(pi.contains("claude-code"));
        assert!(pi.contains(SERVER_NAME));
        let grok = std::fs::read_to_string(grok_path).expect("grok after attach");
        assert!(grok.contains("[models]"));
        assert!(grok.contains("grok-4.5"));
        assert!(grok.contains("[mcp_servers.horizon-browser]"));
    }

    #[test]
    fn last_host_removes_only_horizon_browser() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let pi_path = home.join(".pi/agent/mcp.json");
        std::fs::create_dir_all(pi_path.parent().expect("pi parent")).expect("pi dir");
        std::fs::write(&pi_path, r#"{"mcpServers":{"github":{"url":"https://example"}}}"#).expect("seed pi");

        let mut leases = bind_browser_mcp_attachments(
            OsStr::new("host-a"),
            Path::new("/opt/horizon"),
            Some(&home),
            Some(&home.join(".grok")),
        );
        release_mcp_attachments(&mut leases);

        let pi = std::fs::read_to_string(pi_path).expect("pi after last host");
        assert!(pi.contains("github"));
        assert!(!pi.contains(SERVER_NAME));
        assert!(!home.join(".config/opencode/opencode.json").exists());
        let grok = std::fs::read_to_string(home.join(".grok/config.toml")).unwrap_or_default();
        assert!(!grok.contains("horizon-browser"));
    }

    #[test]
    fn json_leaves_unmanaged_horizon_browser_in_place() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let pi_path = home.join(".pi/agent/mcp.json");
        std::fs::create_dir_all(pi_path.parent().expect("pi parent")).expect("pi dir");
        std::fs::write(
            &pi_path,
            r#"{"mcpServers":{"horizon-browser":{"command":"/usr/bin/custom"}}}"#,
        )
        .expect("seed unmanaged pi");

        let mut leases = bind_browser_mcp_attachments(
            OsStr::new("host-a"),
            Path::new("/opt/horizon"),
            Some(&home),
            Some(&home.join(".grok")),
        );
        let pi = std::fs::read_to_string(&pi_path).expect("pi after attach");
        assert!(pi.contains("/usr/bin/custom"));
        assert!(!pi.contains("/opt/horizon"));
        release_mcp_attachments(&mut leases);
        let pi = std::fs::read_to_string(pi_path).expect("pi after last host");
        assert!(pi.contains("/usr/bin/custom"));
        assert!(pi.contains(SERVER_NAME));
    }

    #[test]
    fn live_peer_keeps_attached_mcp() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let mut first = bind_browser_mcp_attachments(
            OsStr::new("host-a"),
            Path::new("/opt/horizon-a"),
            Some(&home),
            Some(&home.join(".grok")),
        );
        let mut second = bind_browser_mcp_attachments(
            OsStr::new("host-b"),
            Path::new("/opt/horizon-b"),
            Some(&home),
            Some(&home.join(".grok")),
        );
        let pi = std::fs::read_to_string(home.join(".pi/agent/mcp.json")).expect("pi after second attach");
        assert!(pi.contains("/opt/horizon-a"));
        assert!(!pi.contains("/opt/horizon-b"));
        release_mcp_attachments(&mut first);
        let pi = std::fs::read_to_string(home.join(".pi/agent/mcp.json")).expect("pi after first host exit");
        assert!(pi.contains("/opt/horizon-b"));
        assert!(!pi.contains("/opt/horizon-a"));
        release_mcp_attachments(&mut second);
        let pi = std::fs::read_to_string(home.join(".pi/agent/mcp.json")).expect("pi after last host");
        assert!(!pi.contains(SERVER_NAME));
    }

    #[test]
    fn invalid_json_is_left_untouched() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let pi_path = home.join(".pi/agent/mcp.json");
        std::fs::create_dir_all(pi_path.parent().expect("pi parent")).expect("pi dir");
        std::fs::write(&pi_path, "not-json").expect("seed invalid");

        let leases = bind_browser_mcp_attachments(
            OsStr::new("host-a"),
            Path::new("/opt/horizon"),
            Some(&home),
            Some(&home.join(".grok")),
        );
        assert_eq!(std::fs::read_to_string(pi_path).expect("invalid pi"), "not-json");
        assert_eq!(
            leases.len(),
            2,
            "pi attach should be skipped; grok and antigravity still bind"
        );
    }

    #[test]
    fn grok_leaves_unmanaged_horizon_browser_in_place() {
        let temp = tempfile::tempdir().expect("temp dir");
        let grok_home = temp.path().join("grok");
        std::fs::create_dir_all(&grok_home).expect("grok home");
        std::fs::write(
            grok_home.join("config.toml"),
            "[mcp_servers.horizon-browser]\ncommand = \"/usr/bin/custom\"\n",
        )
        .expect("seed unmanaged");

        let _leases =
            bind_browser_mcp_attachments(OsStr::new("host-a"), Path::new("/opt/horizon"), None, Some(&grok_home));
        let grok = std::fs::read_to_string(grok_home.join("config.toml")).expect("grok");
        assert!(grok.contains("/usr/bin/custom"));
        assert!(!grok.contains("/opt/horizon"));
        assert!(!grok.contains("BEGIN HORIZON-MANAGED"));
    }

    #[test]
    fn grok_block_round_trips_without_clobbering_neighbors() {
        let original = "[models]\ndefault = \"grok-4.5\"\n\n[ui]\nsimple_mode = true\n";
        let with_block = replace_or_append_grok_block(original, &grok_managed_block("/opt/horizon"));
        assert!(with_block.contains("[models]"));
        assert!(with_block.contains("[ui]"));
        assert!(with_block.contains("[mcp_servers.horizon-browser]"));
        let stripped = strip_grok_managed_block(&with_block);
        assert!(stripped.contains("[models]"));
        assert!(stripped.contains("[ui]"));
        assert!(!stripped.contains("[mcp_servers.horizon-browser]"));
        assert!(!has_unmanaged_grok_server(&with_block));
        assert!(has_unmanaged_grok_server(
            "[mcp_servers.horizon-browser]\ncommand = \"/usr/bin/custom\"\n"
        ));
        assert!(has_unmanaged_grok_server(
            "[mcp_servers]\nhorizon-browser = { command = \"/usr/bin/custom\" }\n"
        ));
        assert!(has_unmanaged_grok_server(
            "[mcp_servers.\"horizon-browser\"]\ncommand = \"/usr/bin/custom\"\n"
        ));
    }

    #[test]
    fn grok_leaves_quoted_horizon_browser_table_in_place() {
        let temp = tempfile::tempdir().expect("temp dir");
        let grok_home = temp.path().join("grok");
        std::fs::create_dir_all(&grok_home).expect("grok home");
        std::fs::write(
            grok_home.join("config.toml"),
            "[mcp_servers.\"horizon-browser\"]\ncommand = \"/usr/bin/custom\"\n",
        )
        .expect("seed quoted unmanaged");

        let _leases =
            bind_browser_mcp_attachments(OsStr::new("host-a"), Path::new("/opt/horizon"), None, Some(&grok_home));
        let grok = std::fs::read_to_string(grok_home.join("config.toml")).expect("grok");
        assert!(grok.contains("/usr/bin/custom"));
        assert!(!grok.contains("/opt/horizon"));
        assert_eq!(grok.matches("[mcp_servers").count(), 1);
    }

    #[test]
    fn sidecar_write_failure_skips_live_lease() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let pi_path = home.join(".pi/agent/mcp.json");
        let live_dir = home.join(".pi/agent").join(LEASES_DIR).join("mcp.json");
        std::fs::create_dir_all(pi_path.parent().expect("pi parent")).expect("pi dir");
        std::fs::create_dir_all(live_dir.join("host-a.command")).expect("block sidecar");
        std::fs::write(&pi_path, r#"{"mcpServers":{"github":{"url":"https://example"}}}"#).expect("seed pi");

        let leases = bind_browser_mcp_attachments(
            OsStr::new("host-a"),
            Path::new("/opt/horizon"),
            Some(&home),
            Some(&home.join(".grok")),
        );
        let pi = std::fs::read_to_string(&pi_path).expect("pi after failed sidecar");
        assert!(pi.contains("github"));
        assert!(!pi.contains(SERVER_NAME));
        assert!(
            leases.iter().all(|lease| lease.path != pi_path),
            "pi must not stay leased without a command sidecar"
        );
        assert_eq!(leases.len(), 2, "grok and antigravity still bind");
    }

    #[test]
    fn durable_command_keeps_nonexistent_paths() {
        assert_eq!(
            durable_mcp_command(Path::new("/opt/horizon")),
            Path::new("/opt/horizon")
        );
    }

    #[cfg(unix)]
    #[test]
    fn attach_writes_through_existing_symlinks() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let store = temp.path().join("dotfiles");
        std::fs::create_dir_all(store.join("pi")).expect("store");
        std::fs::create_dir_all(home.join(".pi/agent")).expect("pi dir");
        let target = store.join("pi/mcp.json");
        std::fs::write(&target, "{\"mcpServers\":{}}\n").expect("seed target");
        std::os::unix::fs::symlink(&target, home.join(".pi/agent/mcp.json")).expect("symlink");

        let _leases = bind_browser_mcp_attachments(
            OsStr::new("host-a"),
            Path::new("/opt/horizon"),
            Some(&home),
            Some(&home.join(".grok")),
        );

        let link = home.join(".pi/agent/mcp.json");
        assert!(std::fs::symlink_metadata(&link).expect("meta").file_type().is_symlink());
        let body = std::fs::read_to_string(&target).expect("target");
        assert!(body.contains("/opt/horizon"));
        assert!(body.contains(SERVER_NAME));
    }
}
