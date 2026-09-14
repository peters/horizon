use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use super::persist::{read_existing, write_text_atomic};
use super::{BROWSER_MCP_ARG, LEASE_ENV, SERVER_NAME};

pub(super) fn stdio_server(command: &str) -> Value {
    serde_json::json!({
        "command": command,
        "args": [BROWSER_MCP_ARG],
        "env": {
            LEASE_ENV: "1",
        },
    })
}

pub(super) fn antigravity_server(command: &str) -> Value {
    serde_json::json!({
        "command": command,
        "args": [BROWSER_MCP_ARG],
        "disabled": false,
        "env": {
            LEASE_ENV: "1",
        },
    })
}

pub(super) fn merge_server(path: &Path, server: Value) -> io::Result<()> {
    let mut root = read_json_object(path)?;
    let container = json_object_at(&mut root, &["mcpServers"])?;
    if json_server_is_unmanaged(container.get(SERVER_NAME)) {
        tracing::warn!(path = %path.display(), "leaving user-owned horizon-browser MCP server in place");
        return Ok(());
    }
    container.insert(SERVER_NAME.to_string(), server);
    write_text_atomic(path, &pretty_json(&root)?)
}

pub(super) fn remove_server(path: &Path) -> io::Result<()> {
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
    let mut jsonc = contents.to_string();
    if let Err(error) = json_strip_comments::strip(&mut jsonc) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("MCP config is not valid JSONC ({}): {}", path.display(), error),
        ));
    }
    match serde_json::from_str::<Value>(jsonc.trim()) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("MCP config is not a JSON object: {}", path.display()),
        )),
        Err(error) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("MCP config is not valid JSONC ({}): {}", path.display(), error),
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
