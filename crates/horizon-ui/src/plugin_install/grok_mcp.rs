//! Host-neutral Grok registration. Each in-process agent inherits its own panel
//! identity and executable; configuration never stores a host's private identity.

use std::fs::{self, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use horizon_core::{BrowserHostConfig, browser_mcp_args};
use toml_edit::{Array, DocumentMut, Item, Value};

/// The registration, with the arguments that a named remote browser host needs.
/// Without a host the text is the local server of this computer.
fn registration(browser_host: Option<&BrowserHostConfig>) -> String {
    let args = browser_mcp_args(browser_host)
        .iter()
        .map(|argument| format!("\"{}\"", argument.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
# Horizon browser tools receive their executable and workspace identity at launch.
[mcp_servers.horizon-browser]
command = "${{HORIZON_BROWSER_MCP_EXECUTABLE:-}}"
args = [{args}]
enabled = true
startup_timeout_sec = 30
tool_timeout_sec = 3660
[mcp_servers.horizon-browser.env]
HORIZON_BROWSER_ACTOR = "${{HORIZON_BROWSER_ACTOR:-}}"
HORIZON_BROWSER_HOST_INSTANCE = "${{HORIZON_BROWSER_HOST_INSTANCE:-}}"
"#
    )
}

pub(super) fn bind_browser_skill(
    home: &Path,
    host_id: &std::ffi::OsStr,
    browser_host: Option<&BrowserHostConfig>,
) -> io::Result<Vec<super::SkillRootLease>> {
    let dir = home.join("skills").join(super::HORIZON_BROWSER_SKILL);
    let lease = super::user_skills::bind_prepared_skill_root(host_id, dir.clone(), validate_browser_skill, || {
        validate_browser_skill(&dir)?;
        register(home, browser_host)?;
        install_browser_skill(&dir)
    })?;
    Ok(vec![lease])
}

fn install_browser_skill(dir: &Path) -> io::Result<()> {
    if dir.try_exists()? {
        return Ok(());
    }
    let parent = dir
        .parent()
        .ok_or_else(|| io::Error::other("skill directory has no parent"))?;
    // Publish only a complete directory. An interrupted write leaves a sibling
    // staging directory, never a partial skill that looks like user content.
    let staging = tempfile::Builder::new()
        .prefix(".horizon-browser-")
        .tempdir_in(parent)?;
    super::sync_plugin_files(staging.path(), super::BROWSER_SKILL_FILES)?;
    fs::rename(staging.path(), dir)
}

fn validate_browser_skill(dir: &Path) -> io::Result<()> {
    super::mcp_skills::validate_skill_files(dir, super::BROWSER_SKILL_FILES)
}

pub(super) fn register(home: &Path, browser_host: Option<&BrowserHostConfig>) -> io::Result<bool> {
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
    let Some(updated) = append_registration(&original, browser_host)? else {
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

fn append_registration(original: &str, browser_host: Option<&BrowserHostConfig>) -> io::Result<Option<String>> {
    let mut document = parse(original)?;
    let registration = registration(browser_host);
    let expected = parse(&registration)?;
    let expected_table = &expected["mcp_servers"]["horizon-browser"];
    let mut rewrite_args = false;
    if let Some(servers) = document.get("mcp_servers") {
        let Some(servers) = servers.as_table_like() else {
            return Err(io::Error::other(
                "Grok MCP settings are not a table; registration left unchanged",
            ));
        };
        if let Some(existing) = servers.get("horizon-browser") {
            if equivalent(existing, expected_table) {
                return Ok(None);
            }
            // Horizon owns the tables it wrote, so a change of the entry, such
            // as a person who now names a browser host, updates that table
            // instead of leaving the entry of an earlier start in place. A
            // table that Horizon did not write is still another server.
            if !is_horizon_table(existing, expected_table) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Grok already has a different horizon-browser server; registration left unchanged",
                ));
            }
            rewrite_args = true;
        }
    }
    if rewrite_args {
        return replace_args(&mut document, &browser_mcp_args(browser_host)).map(Some);
    }
    if let Some(servers) = document.get_mut("mcp_servers").and_then(Item::as_inline_table_mut) {
        let registration = expected["mcp_servers"]["horizon-browser"]
            .as_table()
            .ok_or_else(|| io::Error::other("embedded registration is not a table"))?
            .clone()
            .into_inline_table();
        servers.insert("horizon-browser", Value::InlineTable(registration));
        return Ok(Some(document.to_string()));
    }
    let updated = format!("{original}\n{registration}");
    // Inline/dotted table declarations can forbid appending a child table.
    // Validate the combined document before touching the user's file.
    parse(&updated)?;
    Ok(Some(updated))
}

/// Replace only the `args` of the `horizon-browser` table that Horizon wrote, so
/// a configuration that a person already has keeps every other line and value.
fn replace_args(document: &mut DocumentMut, args: &[String]) -> io::Result<String> {
    let table = document
        .get_mut("mcp_servers")
        .and_then(Item::as_table_like_mut)
        .and_then(|servers| servers.get_mut("horizon-browser"))
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| io::Error::other("embedded registration is not a table"))?;
    let mut array = Array::new();
    for argument in args {
        array.push(argument.as_str());
    }
    if let Some(previous) = table.get("args").and_then(Item::as_array) {
        *array.decor_mut() = previous.decor().clone();
    }
    table.insert("args", Item::Value(Value::Array(array)));
    let updated = document.to_string();
    // A value that the table can not hold must not reach the person's file.
    parse(&updated)?;
    Ok(updated)
}

/// Whether an existing `horizon-browser` table is one that Horizon wrote for
/// some browser host: every key and every value is the one of the expected
/// registration, except `args`, which is an argument list of
/// `horizon --browser-mcp` for any host. Horizon never wrote another shape.
fn is_horizon_table(existing: &Item, expected: &Item) -> bool {
    let (Some(existing), Some(expected)) = (existing.as_table_like(), expected.as_table_like()) else {
        return false;
    };
    existing.len() == expected.len()
        && expected.iter().all(|(key, value)| {
            existing.get(key).is_some_and(|existing| match key {
                "args" => is_browser_mcp_args(existing),
                _ => equivalent(existing, value),
            })
        })
}

/// Whether a value is the `args` of a browser MCP server that Horizon writes:
/// `--browser-mcp` first, then only `--remote <target>` and `--profile <name>`.
fn is_browser_mcp_args(item: &Item) -> bool {
    let Some(args) = item.as_array() else {
        return false;
    };
    let mut values = Vec::with_capacity(args.len());
    for value in args {
        let Some(text) = value.as_str() else {
            return false;
        };
        values.push(text);
    }
    let Some((&"--browser-mcp", mut rest)) = values.split_first() else {
        return false;
    };
    let mut remote = false;
    let mut profile = false;
    while let Some((flag, tail)) = rest.split_first() {
        let Some((value, tail)) = tail.split_first() else {
            return false;
        };
        match (*flag, remote, profile) {
            ("--remote", false, _) => remote = true,
            ("--profile", true, false) => profile = true,
            _ => return false,
        }
        // A value that `ssh` reads as an option is no value of the entry.
        if value.is_empty() || value.starts_with('-') {
            return false;
        }
        rest = tail;
    }
    true
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
