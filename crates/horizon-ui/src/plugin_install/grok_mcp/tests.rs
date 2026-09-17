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

#[test]
fn rejected_registration_never_leases_or_removes_existing_browser_skill() {
    use super::super::{BROWSER_SKILL_FILES, release_skill_roots};
    for (settings, skill) in [
        ("[mcp_servers.horizon-browser]\ncommand='custom'\n", "user skill"),
        (
            "[mcp_servers.horizon-browser]\ncommand='custom'\n",
            BROWSER_SKILL_FILES[0].content,
        ),
        ("# no server yet\n", "user skill"),
    ] {
        let home = tempfile::tempdir().expect("home");
        let dir = home.path().join("skills/horizon-browser");
        fs::create_dir_all(&dir).expect("skill directory");
        fs::write(dir.join("SKILL.md"), skill).expect("skill");
        fs::write(home.path().join("config.toml"), settings).expect("config");
        let mut roots = bind_browser_skill(home.path(), std::ffi::OsStr::new("host")).unwrap_or_default();
        assert!(roots.is_empty());
        release_skill_roots(&mut roots);
        assert_eq!(
            fs::read_to_string(dir.join("SKILL.md")).expect("preserved skill"),
            skill
        );
        assert_eq!(
            fs::read_to_string(home.path().join("config.toml")).expect("preserved config"),
            settings
        );
    }
}

#[test]
fn browser_skill_survives_until_last_registered_host_exits() {
    use super::super::{BROWSER_SKILL_FILES, release_skill_roots, sync_plugin_files};
    let home = tempfile::tempdir().expect("home");
    let dir = home.path().join("skills/horizon-browser");
    let mut first = bind_browser_skill(home.path(), std::ffi::OsStr::new("first")).expect("first lease");
    sync_plugin_files(&dir, BROWSER_SKILL_FILES).expect("install");
    let mut second = bind_browser_skill(home.path(), std::ffi::OsStr::new("second")).expect("second lease");
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    release_skill_roots(&mut first);
    assert!(dir.join("SKILL.md").exists());
    release_skill_roots(&mut second);
    assert!(!dir.exists());
}

#[test]
fn browser_skill_with_extra_user_content_is_preserved() {
    use super::super::{BROWSER_SKILL_FILES, sync_plugin_files};
    let home = tempfile::tempdir().expect("home");
    let dir = home.path().join("skills/horizon-browser");
    sync_plugin_files(&dir, BROWSER_SKILL_FILES).expect("install");
    fs::write(dir.join("notes.txt"), "user notes").expect("notes");
    assert!(bind_browser_skill(home.path(), std::ffi::OsStr::new("host")).is_err());
    assert_eq!(
        fs::read_to_string(dir.join("notes.txt")).expect("notes preserved"),
        "user notes"
    );
    assert!(!home.path().join("config.toml").exists());
}

#[test]
fn concurrent_startup_waits_for_complete_skill_installation() {
    use super::super::{BROWSER_SKILL_FILES, release_skill_roots, sync_plugin_files, user_skills};
    use std::sync::mpsc;
    use std::time::Duration;

    let home = tempfile::tempdir().expect("home");
    let dir = home.path().join("skills/horizon-browser");
    let (partial_tx, partial_rx) = mpsc::channel();
    let (finish_tx, finish_rx) = mpsc::channel();
    let (second_tx, second_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let first_dir = &dir;
        let first_home = &home;
        let first = scope.spawn(move || {
            let dir = first_dir;
            let home = first_home;
            user_skills::bind_prepared_skill_root(std::ffi::OsStr::new("first"), dir.clone(), || {
                register(home.path())?;
                fs::create_dir_all(dir)?;
                fs::write(dir.join("SKILL.md"), "partial")?;
                partial_tx.send(()).expect("partial signal");
                finish_rx.recv().expect("finish signal");
                sync_plugin_files(dir, BROWSER_SKILL_FILES)?;
                Ok(())
            })
            .expect("first lease")
        });
        partial_rx.recv().expect("partial installation");
        let second = scope.spawn(|| {
            let result = bind_browser_skill(home.path(), std::ffi::OsStr::new("second"));
            second_tx.send(()).expect("second finished");
            result
        });
        let second_waited = second_rx.recv_timeout(Duration::from_millis(100)).is_err();
        finish_tx.send(()).expect("finish installation");
        let mut first = vec![first.join().expect("first thread")];
        let mut second = second.join().expect("second thread").expect("second lease");
        assert!(second_waited, "second host must wait for the completed skill");
        release_skill_roots(&mut first);
        assert!(dir.join("SKILL.md").is_file());
        release_skill_roots(&mut second);
        assert!(!dir.exists());
    });
}
