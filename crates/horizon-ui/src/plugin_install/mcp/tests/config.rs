use std::ffi::OsStr;
use std::path::Path;

use super::super::grok::{
    grok_managed_block, has_unmanaged_grok_server, replace_or_append_grok_block, strip_grok_managed_block,
};
use super::super::{SERVER_NAME, bind_browser_mcp_attachments, release_mcp_attachments};

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
fn grok_leaves_unmanaged_horizon_browser_in_place() {
    let temp = tempfile::tempdir().expect("temp dir");
    let grok_home = temp.path().join("grok");
    std::fs::create_dir_all(&grok_home).expect("grok home");
    std::fs::write(
        grok_home.join("config.toml"),
        "[mcp_servers.horizon-browser]\ncommand = \"/usr/bin/custom\"\n",
    )
    .expect("seed unmanaged");

    let _leases = bind_browser_mcp_attachments(OsStr::new("host-a"), Path::new("/opt/horizon"), None, Some(&grok_home));
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
fn grok_managed_block_escapes_control_characters_in_command() {
    let command = "a\u{0000}b\u{0008}c\u{007f}/horizon";
    let block = grok_managed_block(command);
    let table = block.parse::<toml::Table>().expect("managed block must be valid TOML");
    let stored = table
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .and_then(|servers| servers.get("horizon-browser"))
        .and_then(toml::Value::as_table)
        .and_then(|server| server.get("command"))
        .and_then(toml::Value::as_str);
    assert_eq!(stored, Some(command));
}

#[test]
fn grok_merges_horizon_browser_into_inline_mcp_servers() {
    let temp = tempfile::tempdir().expect("temp dir");
    let grok_home = temp.path().join("grok");
    std::fs::create_dir_all(&grok_home).expect("grok home");
    std::fs::write(
        grok_home.join("config.toml"),
        "mcp_servers = { github = { command = \"/usr/bin/gh\" } }\n",
    )
    .expect("seed inline mcp_servers");

    let mut leases =
        bind_browser_mcp_attachments(OsStr::new("host-a"), Path::new("/opt/horizon"), None, Some(&grok_home));
    let grok = std::fs::read_to_string(grok_home.join("config.toml")).expect("grok after attach");
    assert!(grok.contains("github"));
    assert!(grok.contains("/usr/bin/gh"));
    assert!(grok.contains(SERVER_NAME));
    assert!(grok.contains("/opt/horizon"));
    assert!(!grok.contains("[mcp_servers.horizon-browser]"));
    release_mcp_attachments(&mut leases);
    let grok = std::fs::read_to_string(grok_home.join("config.toml")).expect("grok after last host");
    assert!(grok.contains("github"));
    assert!(grok.contains("/usr/bin/gh"));
    assert!(!grok.contains(SERVER_NAME));
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

    let _leases = bind_browser_mcp_attachments(OsStr::new("host-a"), Path::new("/opt/horizon"), None, Some(&grok_home));
    let grok = std::fs::read_to_string(grok_home.join("config.toml")).expect("grok");
    assert!(grok.contains("/usr/bin/custom"));
    assert!(!grok.contains("/opt/horizon"));
    assert_eq!(grok.matches("[mcp_servers").count(), 1);
}

#[test]
fn jsonc_antigravity_config_receives_horizon_browser() {
    let temp = tempfile::tempdir().expect("temp dir");
    let home = temp.path().join("home");
    let path = home.join(".gemini/config/mcp_config.json");
    std::fs::create_dir_all(path.parent().expect("antigravity parent")).expect("antigravity dir");
    std::fs::write(
        &path,
        r#"{
  // user MCP servers
  "mcpServers": {
    "github": {
      "command": "/usr/bin/gh",
    },
  },
}"#,
    )
    .expect("seed jsonc");

    let _leases = bind_browser_mcp_attachments(
        OsStr::new("host-a"),
        Path::new("/opt/horizon"),
        Some(&home),
        Some(&home.join(".grok")),
    );
    let body = std::fs::read_to_string(path).expect("antigravity after attach");
    assert!(body.contains("github"));
    assert!(body.contains("/usr/bin/gh"));
    assert!(body.contains(SERVER_NAME));
    assert!(body.contains("/opt/horizon"));
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
