//! Reconcile only worker-owned tool tables, preserving other TOML and comments.
use std::io::{self, Read, Write};
use toml_edit::{DocumentMut, Item, Table};

const MANAGED: [&str; 4] = [
    "horizon-browser",
    "horizon-device",
    "horizon-cloud-companions",
    "horizon-worker",
];
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    original: String,
    managed: String,
}

pub fn run() -> io::Result<()> {
    let mut bytes = Vec::new();
    io::stdin().take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(io::Error::other("Agent tool configuration is too large"));
    }
    let request: Request =
        serde_json::from_slice(&bytes).map_err(|_| io::Error::other("Invalid tool configuration request"))?;
    let result = reconcile(&request.original, &request.managed).map_err(io::Error::other)?;
    io::stdout().write_all(result.as_bytes())
}

fn reconcile(original: &str, managed: &str) -> Result<String, &'static str> {
    let mut original: DocumentMut = original
        .parse()
        .map_err(|_| "Invalid existing agent TOML configuration")?;
    let managed: DocumentMut = managed
        .parse()
        .map_err(|_| "Invalid managed agent TOML configuration")?;
    if !original.contains_key("mcp_servers") {
        original["mcp_servers"] = Item::Table(Table::new());
    }
    let servers = original["mcp_servers"]
        .as_table_like_mut()
        .ok_or("Agent MCP configuration must be a table")?;
    for name in MANAGED {
        servers.remove(name);
    }
    if let Some(selected) = managed.get("mcp_servers") {
        for (name, value) in selected.as_table_like().ok_or("Invalid managed MCP table")?.iter() {
            if !MANAGED.contains(&name) {
                return Err("Unexpected managed tool name");
            }
            servers.insert(name, value.clone());
        }
    }
    Ok(original.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_tables_and_nested_env_are_removed_without_touching_multiline_values() {
        let original = r#"# preserve this comment
model = "selected"
[mcp_servers."horizon-browser"]
command = "browser"
[mcp_servers."horizon-browser".env]
CUSTOM = "value"
[mcp_servers.project]
command = "project"
notes = '''
[mcp_servers.horizon-browser]
this is a literal string
'''
"#;
        let native = "[mcp_servers.horizon-device]\ncommand = 'device'\n";
        let result = reconcile(original, native).unwrap();
        let parsed: DocumentMut = result.parse().unwrap();
        assert!(parsed["mcp_servers"].get("horizon-browser").is_none());
        assert_eq!(
            parsed["mcp_servers"]["horizon-device"]["command"].as_str(),
            Some("device")
        );
        assert_eq!(parsed["mcp_servers"]["project"]["command"].as_str(), Some("project"));
        assert!(
            parsed["mcp_servers"]["project"]["notes"]
                .as_str()
                .unwrap()
                .contains("[mcp_servers.horizon-browser]")
        );
        assert!(result.contains("# preserve this comment"));
        let minimal = reconcile(&result, "").unwrap();
        let parsed: DocumentMut = minimal.parse().unwrap();
        assert!(parsed["mcp_servers"].get("horizon-device").is_none());
        assert!(parsed["mcp_servers"].get("project").is_some());
    }

    #[test]
    fn repeated_configuration_is_idempotent_and_invalid_input_stays_private() {
        let selected = "[mcp_servers.horizon-browser]\ncommand = 'browser'\n";
        let first = reconcile("model = 'selected'", selected).unwrap();
        assert_eq!(reconcile(&first, selected).unwrap(), first);
        assert!(
            !reconcile("secret='private-value", selected)
                .unwrap_err()
                .contains("private-value")
        );
        assert!(reconcile("", "[mcp_servers.unrelated]\ncommand='other'").is_err());
    }

    #[test]
    fn the_stop_tool_is_managed_so_opting_out_removes_it() {
        let selected = "[mcp_servers.horizon-worker]\ncommand = '/usr/local/bin/horizon-worker-stop'\nargs = ['mcp']\n";
        let enabled = reconcile("model = 'selected'", selected).unwrap();
        assert!(enabled.contains("horizon-worker-stop"));
        let disabled = reconcile(&enabled, "").unwrap();
        assert!(!disabled.contains("horizon-worker"));
        assert!(disabled.contains("model = 'selected'"));
    }
}
