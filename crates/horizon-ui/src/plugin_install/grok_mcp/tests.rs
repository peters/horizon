use super::*;

#[test]
fn registration_preserves_settings_and_is_idempotent() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("config.toml");
    let original = "# User settings\n[cli]\nuse_leader = true\n[permissions]\nmode = 'default'\n[mcp_servers.other]\ncommand = 'other-server'\n";
    fs::write(&path, original).expect("settings");
    assert!(register(home.path()).expect("register"));
    let registered = fs::read_to_string(&path).expect("registered settings");
    assert!(registered.starts_with(original));
    assert!(!register(home.path()).expect("idempotent registration"));
    assert_eq!(fs::read_to_string(&path).expect("settings"), registered);
    let document = parse(&registered).expect("valid TOML");
    assert_eq!(
        document["mcp_servers"]["horizon-browser"]["tool_timeout_sec"].as_integer(),
        Some(3660)
    );
}

#[test]
fn registration_preserves_unmanaged_collisions_and_invalid_configuration() {
    for original in [
        "[mcp_servers.horizon-browser]\ncommand='my-server'\n",
        "mcp_servers = { 'horizon-browser' = { command = 'custom' } }\n",
        "invalid = [",
        "mcp_servers = false\n",
    ] {
        let home = tempfile::tempdir().expect("home");
        let path = home.path().join("config.toml");
        fs::write(&path, original).expect("settings");
        assert!(register(home.path()).is_err());
        assert_eq!(fs::read_to_string(path).expect("preserved"), original);
    }
}

#[cfg(unix)]
#[test]
fn registration_preserves_permissions_and_refuses_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("config.toml");
    fs::write(&path, "# settings\n").expect("settings");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("permissions");
    register(home.path()).expect("register");
    assert_eq!(
        fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
        0o640
    );
    fs::remove_file(&path).expect("remove test file");
    let target = home.path().join("original.toml");
    fs::write(&target, "# original\n").expect("target");
    symlink(&target, &path).expect("symlink");
    assert!(register(home.path()).is_err());
    assert_eq!(fs::read_to_string(target).expect("target unchanged"), "# original\n");
}

#[test]
fn registration_respects_the_providers_config_lock() {
    let home = tempfile::tempdir().expect("home");
    let lock = fs::File::create(home.path().join(".config-init.lock")).expect("provider lock");
    lock.lock().expect("hold provider lock");
    let error = register(home.path()).expect_err("busy provider config");
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert!(!home.path().join("config.toml").exists());
    drop(lock);
    assert!(register(home.path()).expect("register after provider finishes"));
}

#[test]
fn registration_accepts_equivalent_settings_after_provider_reserialization() {
    let normalized = r"[mcp_servers.horizon-browser]
tool_timeout_sec=3660
startup_timeout_sec=30
args=['--browser-mcp']
enabled=true
command='${HORIZON_BROWSER_MCP_EXECUTABLE:-}'
env={HORIZON_BROWSER_HOST_INSTANCE='${HORIZON_BROWSER_HOST_INSTANCE:-}',HORIZON_BROWSER_ACTOR='${HORIZON_BROWSER_ACTOR:-}'}
";
    assert!(append_registration(normalized).expect("equivalent table").is_none());
    let altered = normalized.replace("tool_timeout_sec=3660", "tool_timeout_sec=60");
    assert!(append_registration(&altered).is_err());
}
