use std::ffi::OsStr;
use std::path::Path;

use super::super::{LEASES_DIR, SERVER_NAME, bind_browser_mcp_attachments, release_mcp_attachments};

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

    let antigravity = std::fs::read_to_string(home.join(".gemini/config/mcp_config.json")).expect("antigravity config");
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
    assert_eq!(leases.len(), 2, "grok and antigravity still bind");
}

#[cfg(target_os = "linux")]
#[test]
fn attach_preserves_live_process_executable_path() {
    let command = std::path::PathBuf::from(format!("/proc/{}/exe", std::process::id()));
    let canonical = std::fs::canonicalize(&command).expect("live exe");
    assert_ne!(
        canonical, command,
        "test requires /proc/<pid>/exe to resolve to a different path"
    );

    let temp = tempfile::tempdir().expect("temp dir");
    let home = temp.path().join("home");
    let _leases = bind_browser_mcp_attachments(OsStr::new("host-a"), &command, Some(&home), Some(&home.join(".grok")));
    let pi = std::fs::read_to_string(home.join(".pi/agent/mcp.json")).expect("pi config");
    assert!(pi.contains(&command.display().to_string()));
    assert!(
        !pi.contains(canonical.to_str().expect("utf-8 canonical path")),
        "live /proc/<pid>/exe must not be canonicalized to the install path"
    );
}
