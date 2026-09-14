//! Process-scoped Horizon browser MCP attachments for agents that are not
//! wired through Claude `--plugin-dir` or Codex `-c` launch flags.
//!
//! Pi, Grok, `OpenCode`, and Antigravity read MCP servers from their own config
//! files. Horizon writes a `horizon-browser` entry when this host starts and
//! removes only that entry when the last host for the file exits.

use std::ffi::OsStr;
use std::fs::{OpenOptions, TryLockError};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

mod grok;
mod json;
mod persist;

use persist::write_text_atomic;

const SERVER_NAME: &str = "horizon-browser";
const BROWSER_MCP_ARG: &str = "--browser-mcp";
const LEASE_ENV: &str = "HORIZON_BROWSER_MCP_LEASE";
const LEASES_DIR: &str = ".horizon-leases";

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
        McpConfigKind::Pi => json::merge_server(path, json::stdio_server(command)),
        McpConfigKind::Antigravity => json::merge_server(path, json::antigravity_server(command)),
        McpConfigKind::GrokToml => grok::upsert_server(path, command),
    }
}

fn detach_attachment(lease: &McpAttachmentLease) {
    if let Err(error) = match lease.kind {
        McpConfigKind::Pi | McpConfigKind::Antigravity => json::remove_server(&lease.path),
        McpConfigKind::GrokToml => grok::remove_server(&lease.path),
    } {
        tracing::warn!(path = %lease.path.display(), %error, "failed to detach Horizon browser MCP");
    }
}

fn mcp_command_string(command: &Path) -> io::Result<String> {
    command.to_str().map(str::to_string).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Horizon executable path is not valid UTF-8",
        )
    })
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
    (!command.is_empty()).then_some(command)
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

#[cfg(test)]
mod tests;
