//! Host-neutral Grok registration. Each in-process agent inherits its own panel
//! identity and executable; configuration never stores a host's private identity.

use std::fs::{self, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use toml_edit::{DocumentMut, Item, Value};

const REGISTRATION: &str = r#"
# Horizon browser tools receive their executable and workspace identity at launch.
[mcp_servers.horizon-browser]
command = "${HORIZON_BROWSER_MCP_EXECUTABLE:-}"
args = ["--browser-mcp"]
enabled = true
startup_timeout_sec = 30
tool_timeout_sec = 3660
[mcp_servers.horizon-browser.env]
HORIZON_BROWSER_ACTOR = "${HORIZON_BROWSER_ACTOR:-}"
HORIZON_BROWSER_HOST_INSTANCE = "${HORIZON_BROWSER_HOST_INSTANCE:-}"
"#;

pub(super) fn bind_browser_skill(home: &Path, host_id: &std::ffi::OsStr) -> io::Result<Vec<super::SkillRootLease>> {
    let dir = home.join("skills").join(super::HORIZON_BROWSER_SKILL);
    let lease = super::user_skills::bind_prepared_skill_root(host_id, dir.clone(), || {
        validate_browser_skill(&dir)?;
        register(home)?;
        super::sync_plugin_files(&dir, super::BROWSER_SKILL_FILES)?;
        Ok(())
    })?;
    Ok(vec![lease])
}

fn validate_browser_skill(dir: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() {
        let mut entries = fs::read_dir(dir)?;
        let entry = entries.next().transpose()?;
        if let Some(entry) = entry
            && entries.next().is_none()
            && entry.file_name() == "SKILL.md"
            && entry.file_type()?.is_file()
            && fs::read_to_string(entry.path())? == super::BROWSER_SKILL_FILES[0].content
        {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Grok browser skill is not Horizon-owned; preserving existing content",
    ))
}

pub(super) fn register(home: &Path) -> io::Result<bool> {
    fs::create_dir_all(home)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join(".config-init.lock"))?;
    lock_config(&lock)?;
    let path = home.join("config.toml");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(io::Error::other(
                "Grok configuration is a symlink; registration left unchanged",
            ));
        }
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let original = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let Some(updated) = append_registration(&original)? else {
        return Ok(false);
    };
    let mut temporary = tempfile::NamedTempFile::new_in(home)?;
    if let Some(metadata) = &metadata {
        temporary.as_file().set_permissions(metadata.permissions())?;
    }
    temporary.write_all(updated.as_bytes())?;
    temporary.as_file().sync_all()?;
    // The lock coordinates Horizon hosts. Also preserve an external edit made
    // while we prepared the replacement rather than silently overwriting it.
    let current = fs::read_to_string(&path);
    let unchanged = match current {
        Ok(text) => metadata.is_some() && text == original,
        Err(error) => metadata.is_none() && error.kind() == io::ErrorKind::NotFound,
    };
    if !unchanged {
        return Err(io::Error::other(
            "Grok configuration changed during registration; retry on next launch",
        ));
    }
    temporary.persist(&path).map_err(|error| error.error)?;
    Ok(true)
}

// Share Grok's config-writer lock; a separate Horizon-only lock would still
// race with the provider's own settings writes. Keep the stable lock inode.
fn lock_config(file: &fs::File) -> io::Result<()> {
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) if started.elapsed() < Duration::from_secs(1) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Grok configuration is busy; registration deferred",
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

fn append_registration(original: &str) -> io::Result<Option<String>> {
    let document = parse(original)?;
    let expected = parse(REGISTRATION)?;
    if let Some(servers) = document.get("mcp_servers") {
        let Some(servers) = servers.as_table_like() else {
            return Err(io::Error::other(
                "Grok MCP settings are not a table; registration left unchanged",
            ));
        };
        if let Some(existing) = servers.get("horizon-browser") {
            if equivalent(existing, &expected["mcp_servers"]["horizon-browser"]) {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Grok already has a different horizon-browser server; registration left unchanged",
            ));
        }
    }
    let updated = format!("{original}\n{REGISTRATION}");
    // Inline/dotted table declarations can forbid appending a child table.
    // Validate the combined document before touching the user's file.
    parse(&updated)?;
    Ok(Some(updated))
}

fn parse(text: &str) -> io::Result<DocumentMut> {
    text.parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid Grok TOML; registration left unchanged",
        )
    })
}

fn equivalent(left: &Item, right: &Item) -> bool {
    match (left.as_table_like(), right.as_table_like()) {
        (Some(left), Some(right)) => {
            left.len() == right.len()
                && right
                    .iter()
                    .all(|(key, value)| left.get(key).is_some_and(|left| equivalent(left, value)))
        }
        _ => match (left.as_value(), right.as_value()) {
            (Some(left), Some(right)) => equivalent_value(left, right),
            _ => false,
        },
    }
}

fn equivalent_value(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::String(left), Value::String(right)) => left.value() == right.value(),
        (Value::Integer(left), Value::Integer(right)) => left.value() == right.value(),
        (Value::Boolean(left), Value::Boolean(right)) => left.value() == right.value(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| equivalent_value(left, right))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
