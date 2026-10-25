use super::*;

#[test]
fn registration_preserves_settings_and_is_idempotent() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("config.toml");
    let original = "# User settings\n[cli]\nuse_leader = true\n[permissions]\nmode = 'default'\n[mcp_servers.other]\ncommand = 'other-server'\n";
    fs::write(&path, original).expect("settings");
    assert!(register(home.path(), None).expect("register"));
    let registered = fs::read_to_string(&path).expect("registered settings");
    assert!(registered.starts_with(original));
    assert!(!register(home.path(), None).expect("idempotent registration"));
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
        assert!(register(home.path(), None).is_err());
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
    register(home.path(), None).expect("register");
    assert_eq!(
        fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
        0o640
    );
    fs::remove_file(&path).expect("remove test file");
    let target = home.path().join("original.toml");
    fs::write(&target, "# original\n").expect("target");
    symlink(&target, &path).expect("symlink");
    assert!(register(home.path(), None).is_err());
    assert_eq!(fs::read_to_string(target).expect("target unchanged"), "# original\n");
}

#[test]
fn registration_respects_the_providers_config_lock() {
    let home = tempfile::tempdir().expect("home");
    let lock = fs::File::create(home.path().join(".config-init.lock")).expect("provider lock");
    lock.lock().expect("hold provider lock");
    let error = register(home.path(), None).expect_err("busy provider config");
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert!(!home.path().join("config.toml").exists());
    drop(lock);
    assert!(register(home.path(), None).expect("register after provider finishes"));
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
    assert!(
        append_registration(normalized, None)
            .expect("equivalent table")
            .is_none()
    );
    let altered = normalized.replace("tool_timeout_sec=3660", "tool_timeout_sec=60");
    assert!(append_registration(&altered, None).is_err());
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
        let mut roots = bind_browser_skill(home.path(), std::ffi::OsStr::new("host"), None).unwrap_or_default();
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
    let mut first = bind_browser_skill(home.path(), std::ffi::OsStr::new("first"), None).expect("first lease");
    sync_plugin_files(&dir, BROWSER_SKILL_FILES).expect("install");
    let mut second = bind_browser_skill(home.path(), std::ffi::OsStr::new("second"), None).expect("second lease");
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
    assert!(bind_browser_skill(home.path(), std::ffi::OsStr::new("host"), None).is_err());
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
            user_skills::bind_prepared_skill_root(
                std::ffi::OsStr::new("first"),
                dir.clone(),
                validate_browser_skill,
                || {
                    register(home.path(), None)?;
                    fs::create_dir_all(dir)?;
                    fs::write(dir.join("SKILL.md"), "partial")?;
                    partial_tx.send(()).expect("partial signal");
                    finish_rx.recv().expect("finish signal");
                    sync_plugin_files(dir, BROWSER_SKILL_FILES)?;
                    Ok(())
                },
            )
            .expect("first lease")
        });
        partial_rx.recv().expect("partial installation");
        let second = scope.spawn(|| {
            let result = bind_browser_skill(home.path(), std::ffi::OsStr::new("second"), None);
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

#[test]
fn cleanup_preserves_skill_content_added_or_replaced_after_startup() {
    use super::super::release_skill_roots;
    for replace in [false, true] {
        let home = tempfile::tempdir().expect("home");
        let dir = home.path().join("skills/horizon-browser");
        let mut leases = bind_browser_skill(home.path(), std::ffi::OsStr::new("host"), None).expect("lease");
        let path = dir.join(if replace { "SKILL.md" } else { "notes.txt" });
        fs::write(&path, "user content").expect("user edit");
        release_skill_roots(&mut leases);
        assert_eq!(
            fs::read_to_string(path).expect("user content preserved"),
            "user content"
        );
    }
}

#[test]
fn registration_extends_inline_server_tables_without_changing_existing_settings() {
    let original = "# custom settings\nmcp_servers = { other = { command = 'other', args = ['--flag'] } } # keep comment\n[cli]\nuse_leader = true\n";
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("config.toml");
    fs::write(&path, original).expect("original");
    assert!(register(home.path(), None).expect("register inline"));
    let text = fs::read_to_string(&path).expect("registered");
    let document = parse(&text).expect("valid config");
    let prior = parse(original).expect("prior config");
    assert!(equivalent(
        &document["mcp_servers"]["other"],
        &prior["mcp_servers"]["other"]
    ));
    assert!(equivalent(&document["cli"], &prior["cli"]));
    assert!(text.contains("# custom settings"));
    assert!(text.contains("# keep comment"));
    assert!(!register(home.path(), None).expect("idempotent"));
    assert_eq!(fs::read_to_string(path).expect("unchanged second pass"), text);
}

#[test]
fn a_named_browser_host_becomes_the_arguments_of_the_grok_registration() {
    let local = registration(None);
    assert!(local.contains("args = [\"--browser-mcp\"]"), "{local}");
    assert!(!local.contains("--remote"), "the local registration names no host");

    let browser_host = horizon_core::BrowserHostConfig {
        ssh_target: Some("browser-vm".to_string()),
        profile: Some("mail".to_string()),
    };
    let remote = registration(Some(&browser_host));
    assert!(
        remote.contains("args = [\"--browser-mcp\", \"--remote\", \"browser-vm\", \"--profile\", \"mail\"]"),
        "{remote}"
    );
    let document = parse(&remote).expect("valid TOML");
    assert_eq!(
        document["mcp_servers"]["horizon-browser"]["tool_timeout_sec"].as_integer(),
        Some(3660),
        "the remote registration keeps the handoff timeout"
    );

    let home = tempfile::tempdir().expect("home");
    assert!(register(home.path(), Some(&browser_host)).expect("register"));
    let text = fs::read_to_string(home.path().join("config.toml")).expect("registered");
    assert!(text.contains("\"--remote\", \"browser-vm\""), "{text}");
}

fn args_of(text: &str) -> Vec<String> {
    parse(text).expect("valid TOML")["mcp_servers"]["horizon-browser"]["args"]
        .as_array()
        .expect("the args of the entry")
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

#[test]
fn an_entry_of_the_local_server_is_rewritten_for_a_named_host_and_keeps_its_lease() {
    use super::super::release_skill_roots;

    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("config.toml");
    fs::write(&path, "# User settings\n[cli]\nuse_leader = true\n").expect("settings");
    let mut local = bind_browser_skill(home.path(), std::ffi::OsStr::new("host"), None).expect("local registration");
    assert_eq!(local.len(), 1);
    release_skill_roots(&mut local);
    assert_eq!(
        args_of(&fs::read_to_string(&path).expect("registered")),
        ["--browser-mcp"],
        "a start without a host writes the entry of the local server"
    );

    let browser_host = horizon_core::BrowserHostConfig {
        ssh_target: Some("browser-vm".to_string()),
        profile: Some("mail".to_string()),
    };
    let mut remote = bind_browser_skill(home.path(), std::ffi::OsStr::new("host"), Some(&browser_host))
        .expect("the entry that Horizon wrote is updated for the named host");
    assert_eq!(remote.len(), 1, "the update takes the skill lease");
    release_skill_roots(&mut remote);
    let text = fs::read_to_string(&path).expect("registered");
    assert_eq!(
        args_of(&text),
        ["--browser-mcp", "--remote", "browser-vm", "--profile", "mail"]
    );
    let document = parse(&text).expect("valid TOML");
    assert_eq!(
        document["mcp_servers"]["horizon-browser"]["tool_timeout_sec"].as_integer(),
        Some(3660),
        "the rest of the entry is unchanged"
    );
    assert!(
        text.starts_with("# User settings\n[cli]\nuse_leader = true\n"),
        "{text}"
    );
    assert_eq!(
        document["cli"]["use_leader"].as_bool(),
        Some(true),
        "the update touches the args of the entry and nothing else"
    );

    assert!(
        register(home.path(), None).expect("the entry goes back to the local server"),
        "removing the host updates the entry of the host"
    );
    let text = fs::read_to_string(&path).expect("registered");
    assert_eq!(args_of(&text), ["--browser-mcp"]);
    assert!(!text.contains("--remote"), "the entry of the host is gone: {text}");
}

#[test]
fn an_absent_server_table_is_created_as_before() {
    let home = tempfile::tempdir().expect("home");
    assert!(register(home.path(), None).expect("register"));
    assert_eq!(
        fs::read_to_string(home.path().join("config.toml")).expect("registered"),
        format!("\n{}", registration(None)),
        "an absent entry keeps the text that Horizon wrote before this setting"
    );
}

#[test]
fn a_server_table_that_horizon_did_not_write_is_still_refused() {
    let browser_host = horizon_core::BrowserHostConfig {
        ssh_target: Some("browser-vm".to_string()),
        profile: Some("mail".to_string()),
    };
    for original in [
        // Horizon's keys, but an argument list that Horizon never writes.
        r#"[mcp_servers.horizon-browser]
command = "${HORIZON_BROWSER_MCP_EXECUTABLE:-}"
args = ["--browser-mcp", "--other"]
enabled = true
startup_timeout_sec = 30
tool_timeout_sec = 3660
[mcp_servers.horizon-browser.env]
HORIZON_BROWSER_ACTOR = "${HORIZON_BROWSER_ACTOR:-}"
HORIZON_BROWSER_HOST_INSTANCE = "${HORIZON_BROWSER_HOST_INSTANCE:-}"
"#,
        // Horizon's arguments, but another program and another timeout.
        r#"[mcp_servers.horizon-browser]
command = "my-server"
args = ["--browser-mcp"]
enabled = true
startup_timeout_sec = 30
tool_timeout_sec = 60
[mcp_servers.horizon-browser.env]
HORIZON_BROWSER_ACTOR = "${HORIZON_BROWSER_ACTOR:-}"
HORIZON_BROWSER_HOST_INSTANCE = "${HORIZON_BROWSER_HOST_INSTANCE:-}"
"#,
        // A remote target without a value is no entry of Horizon's.
        r#"[mcp_servers.horizon-browser]
command = "${HORIZON_BROWSER_MCP_EXECUTABLE:-}"
args = ["--browser-mcp", "--remote"]
enabled = true
startup_timeout_sec = 30
tool_timeout_sec = 3660
[mcp_servers.horizon-browser.env]
HORIZON_BROWSER_ACTOR = "${HORIZON_BROWSER_ACTOR:-}"
HORIZON_BROWSER_HOST_INSTANCE = "${HORIZON_BROWSER_HOST_INSTANCE:-}"
"#,
    ] {
        let home = tempfile::tempdir().expect("home");
        let path = home.path().join("config.toml");
        fs::write(&path, original).expect("settings");
        let error = register(home.path(), Some(&browser_host)).expect_err("a table that is not Horizon's");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists, "{original}");
        assert!(
            error.to_string().contains("different horizon-browser server"),
            "the typed reason stays: {error}"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("preserved"),
            original,
            "another server is left unchanged"
        );
    }
}

#[test]
fn interrupted_skill_staging_does_not_block_a_later_host() {
    use super::super::release_skill_roots;
    let home = tempfile::tempdir().expect("home");
    register(home.path(), None).expect("registration before interrupted install");
    let staging = home.path().join("skills/.horizon-browser-interrupted");
    fs::create_dir_all(&staging).expect("interrupted staging");
    fs::write(staging.join("partial.tmp"), "partial").expect("incomplete write");
    let dir = home.path().join("skills/horizon-browser");
    assert!(!dir.exists());
    let mut leases = bind_browser_skill(home.path(), std::ffi::OsStr::new("retry"), None).expect("recover install");
    assert_eq!(leases.len(), 1);
    validate_browser_skill(&dir).expect("complete skill");
    release_skill_roots(&mut leases);
    assert!(!dir.exists());
}
