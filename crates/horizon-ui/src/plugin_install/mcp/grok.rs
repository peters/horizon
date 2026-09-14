use std::io;
use std::path::Path;

use super::persist::{read_existing, write_text_atomic};
use super::{BROWSER_MCP_ARG, SERVER_NAME};

const GROK_BEGIN: &str = "# BEGIN HORIZON-MANAGED MCP horizon-browser";
const GROK_END: &str = "# END HORIZON-MANAGED MCP horizon-browser";
const GROK_TABLE: &str = "[mcp_servers.horizon-browser]";

pub(super) fn upsert_server(path: &Path, command: &str) -> io::Result<()> {
    let contents = read_existing(path)?.unwrap_or_default();
    if has_unmanaged_grok_server(&contents) || grok_has_closed_inline_mcp_servers(&strip_grok_managed_block(&contents))
    {
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

pub(super) fn remove_server(path: &Path) -> io::Result<()> {
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

pub(super) fn grok_managed_block(command: &str) -> String {
    format!(
        "{GROK_BEGIN}\n{GROK_TABLE}\ncommand = {}\nargs = [{BROWSER_MCP_ARG:?}]\nenabled = true\nenv = {{ HORIZON_BROWSER_ACTOR = \"${{HORIZON_BROWSER_ACTOR}}\", HORIZON_BROWSER_HOST_INSTANCE = \"${{HORIZON_BROWSER_HOST_INSTANCE}}\" }}\n{GROK_END}\n",
        toml_string(command),
    )
}

pub(super) fn replace_or_append_grok_block(contents: &str, block: &str) -> String {
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

pub(super) fn strip_grok_managed_block(contents: &str) -> String {
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

pub(super) fn has_unmanaged_grok_server(contents: &str) -> bool {
    grok_remainder_has_horizon_browser(&strip_grok_managed_block(contents))
}

fn grok_managed_span(contents: &str) -> Option<(usize, usize)> {
    let start = contents.find(GROK_BEGIN)?;
    let from_start = &contents[start..];
    let end = start + from_start.find(GROK_END)? + GROK_END.len();
    Some((start, end))
}

fn grok_has_closed_inline_mcp_servers(rest: &str) -> bool {
    rest.lines().any(|line| {
        let trimmed = line.trim_start();
        let Some(after_key) = trimmed.strip_prefix("mcp_servers") else {
            return false;
        };
        if after_key.starts_with('.') || after_key.starts_with('[') || after_key.starts_with('"') {
            return false;
        }
        let after_key = after_key.trim_start();
        after_key.starts_with('=') && after_key.contains('{')
    })
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
    toml::Value::String(value.to_string()).to_string()
}
