use super::*;
use std::process::Command;

fn runtime(root: &Path) -> Runtime {
    Runtime {
        workspace: root.join("workspace"),
        live: root.join("run"),
        ssh_home: root.join("home/.ssh"),
        source_helper: "/usr/bin/true".into(),
    }
}

#[test]
fn rejects_grant_paths_before_any_mutation() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    for grant in ["../outside", "/outside", "x y", "x;id", ""] {
        assert!(runtime.apply(&Request::Identity { grant: grant.into() }).is_err());
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn private_identity_is_idempotent_and_response_contains_only_public_material() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let request = Request::Identity {
        grant: "pair-a-b".into(),
    };
    let first = runtime.apply(&request).unwrap();
    assert_eq!(runtime.apply(&request).unwrap(), first);
    let serialized = serde_json::to_string(&first).unwrap();
    assert!(serialized.contains("ssh-ed25519"));
    assert!(!serialized.contains("PRIVATE"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(runtime.key_directory("pair-a-b").join("identity"))
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
    }
}

#[test]
fn revoke_keeps_owner_and_other_grants_and_never_deletes_work() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    files::directory(&runtime.live).unwrap();
    let path = runtime.live.join("horizon-authorized-keys");
    std::fs::write(&path, "owner-key owner\n").unwrap();
    runtime.authorized_key("one", Some("key-one")).unwrap();
    runtime.authorized_key("two", Some("key-two")).unwrap();
    runtime.authorized_key("one", Some("replacement-key")).unwrap();
    let work = runtime.worktree("one");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("dirty.txt"), "preserve").unwrap();
    runtime.apply(&Request::Revoke { grant: "one".into() }).unwrap();
    runtime.apply(&Request::Revoke { grant: "one".into() }).unwrap();
    let keys = std::fs::read_to_string(path).unwrap();
    assert!(keys.contains("owner-key owner"));
    assert!(keys.contains("key-two"));
    assert!(!keys.contains("key-one") && !keys.contains("replacement-key"));
    assert_eq!(std::fs::read_to_string(work.join("dirty.txt")).unwrap(), "preserve");
}

#[test]
fn public_keys_cannot_inject_options_or_extra_lines() {
    for key in [
        "command=evil ssh-ed25519 AAAA",
        "ssh-rsa AAAA",
        "ssh-ed25519 AAAA",
        "ssh-ed25519 AAAA\nssh-ed25519 BBBB",
    ] {
        assert!(ssh::public_key(key).is_err());
    }
}

#[test]
fn generated_include_preserves_user_configuration_and_resets_host_scope() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    files::directory(&runtime.ssh_home).unwrap();
    let path = runtime.ssh_home.join("config");
    std::fs::write(&path, "# original\nHost existing\n  HostName example.invalid\n").unwrap();
    let grant = runtime.key_directory("one");
    files::directory(&grant).unwrap();
    files::write(&grant.join("config"), b"Host companion-app\n  HostName 127.0.0.1\n").unwrap();
    runtime.update_config().unwrap();
    let first = std::fs::read_to_string(&path).unwrap();
    runtime.update_config().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
    assert!(first.ends_with("# original\nHost existing\n  HostName example.invalid\n"));
    let config = std::fs::read_to_string(runtime.live.join("companions/config")).unwrap();
    assert!(config.ends_with("Host *\n"));
    let output = ssh::checked(Command::new("ssh").arg("-G").arg("-F").arg(path).arg("existing")).unwrap();
    assert!(output.contains("hostname example.invalid"));
}

#[test]
fn authorizing_again_preserves_dirty_worktree_and_reuses_the_grant() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    files::directory(&runtime.workspace).unwrap();
    files::directory(&runtime.live.join("horizon-host-keys")).unwrap();
    std::fs::write(runtime.live.join("horizon-authorized-keys"), "owner-key\n").unwrap();
    ssh::checked(
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(runtime.live.join("horizon-host-keys/ed25519")),
    )
    .unwrap();
    let seed = root.path().join("seed");
    ssh::checked(Command::new("git").args(["init", "-q"]).arg(&seed)).unwrap();
    std::fs::write(seed.join("input.txt"), "original").unwrap();
    ssh::checked(Command::new("git").arg("-C").arg(&seed).args(["add", "input.txt"])).unwrap();
    ssh::checked(Command::new("git").arg("-C").arg(&seed).args([
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-qm",
        "Initial fixture",
    ]))
    .unwrap();
    let revision = ssh::checked(Command::new("git").arg("-C").arg(&seed).args(["rev-parse", "HEAD"])).unwrap();
    ssh::checked(
        Command::new("git")
            .args(["clone", "--bare", "-q"])
            .arg(&seed)
            .arg(runtime.workspace.join("repository.git")),
    )
    .unwrap();
    let Response::Identity { public_key } = runtime.apply(&Request::Identity { grant: "pair".into() }).unwrap() else {
        panic!("expected public key")
    };
    let request = Request::Authorize {
        grant: "pair".into(),
        public_key,
        revision: revision.trim().into(),
    };
    let first = runtime.apply(&request).unwrap();
    std::fs::write(runtime.worktree("pair").join("input.txt"), "dirty edit").unwrap();
    assert_eq!(runtime.apply(&request).unwrap(), first);
    assert_eq!(
        std::fs::read_to_string(runtime.worktree("pair").join("input.txt")).unwrap(),
        "dirty edit"
    );
    let keys = std::fs::read_to_string(runtime.live.join("horizon-authorized-keys")).unwrap();
    assert_eq!(keys.lines().count(), 2);
    assert_eq!(keys.matches("horizon-companion:pair").count(), 1);

    std::fs::remove_file(runtime.workspace.join("companions/prepared/pair")).unwrap();
    runtime.apply(&Request::Revoke { grant: "pair".into() }).unwrap();
    assert!(
        runtime.apply(&request).is_err(),
        "an interrupted initial checkout cannot be authorized"
    );
    assert!(
        !std::fs::read_to_string(runtime.live.join("horizon-authorized-keys"))
            .unwrap()
            .contains("horizon-companion:pair")
    );
    assert_eq!(
        std::fs::read_to_string(runtime.worktree("pair").join("input.txt")).unwrap(),
        "dirty edit"
    );
}

#[test]
fn rejected_alias_collision_does_not_poison_other_connections() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let first = runtime.key_directory("first");
    files::directory(&first).unwrap();
    files::write(&first.join("config"), b"Host companion-app\n  HostName 127.0.0.1\n").unwrap();
    runtime.update_config().unwrap();
    let published = std::fs::read(runtime.live.join("companions/config")).unwrap();
    let Response::Identity { public_key } = runtime.apply(&Request::Identity { grant: "second".into() }).unwrap()
    else {
        panic!("expected public key")
    };
    assert!(
        runtime
            .apply(&Request::Connect {
                grant: "second".into(),
                alias: "app".into(),
                host: "127.0.0.1".parse().unwrap(),
                port: 22,
                host_key: public_key,
            })
            .is_err()
    );
    assert!(!runtime.key_directory("second").join("config").exists());
    runtime.update_config().unwrap();
    assert_eq!(
        std::fs::read(runtime.live.join("companions/config")).unwrap(),
        published
    );
}

#[test]
fn command_deadline_stops_descendants_even_after_the_parent_exits() {
    use std::time::{Duration, Instant};
    for parent in ["wait", "exit 0"] {
        let root = tempfile::tempdir().unwrap();
        let late = root.path().join("late-write");
        let started = root.path().join("started");
        let script = format!("printf started > \"$2\"; (sleep 1; printf escaped > \"$1\") & {parent}");
        let before = Instant::now();
        let result = ssh::checked_with_timeout(
            Command::new("sh")
                .args(["-c", &script, "fixture"])
                .arg(&late)
                .arg(&started),
            Duration::from_millis(300),
        );
        assert!(result.is_err());
        assert!(before.elapsed() < Duration::from_secs(2));
        assert!(started.exists());
        std::thread::sleep(Duration::from_millis(900));
        assert!(!late.exists(), "descendant mutated the worktree after timeout");
    }
}

#[test]
fn readiness_preview_uses_the_preserved_user_transport_configuration() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    files::directory(&runtime.ssh_home).unwrap();
    let grant = runtime.key_directory("one");
    files::directory(&grant).unwrap();
    let user_config = runtime.ssh_home.join("config");
    files::write(
        &user_config,
        b"Host *\n  ProxyCommand false\n  RemoteCommand inherited-command\n",
    )
    .unwrap();
    let candidate = "Host companion-app\n  HostName 127.0.0.1\n  Port 22002\n  User root\n";
    let preview = runtime.preview_config(&grant.join("config"), candidate).unwrap();
    let preview_path = grant.join("probe");
    files::write(&preview_path, preview.as_bytes()).unwrap();
    let observed = ssh::checked(
        Command::new("ssh")
            .args(["-G", "-F"])
            .arg(&preview_path)
            .arg("companion-app"),
    )
    .unwrap();
    assert!(observed.contains("proxycommand false"));
    assert!(observed.contains("remotecommand inherited-command"));
    files::write(&grant.join("config"), candidate.as_bytes()).unwrap();
    runtime.update_config().unwrap();
    assert_eq!(
        runtime.preview_config(&grant.join("config"), candidate).unwrap(),
        preview
    );
    let published = ssh::checked(
        Command::new("ssh")
            .args(["-G", "-F"])
            .arg(&user_config)
            .arg("companion-app"),
    )
    .unwrap();
    for field in ["hostname", "port", "user", "proxycommand", "remotecommand"] {
        let value = |text: &str| {
            text.lines()
                .find(|line| line.starts_with(&format!("{field} ")))
                .unwrap()
                .to_owned()
        };
        assert_eq!(value(&observed), value(&published));
    }
}

#[test]
fn readiness_preview_resets_user_host_scope_before_system_configuration() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    files::directory(&runtime.ssh_home).unwrap();
    let grant = runtime.key_directory("one");
    files::directory(&grant).unwrap();
    files::write(
        &runtime.ssh_home.join("config"),
        b"Host unrelated\n  HostName unrelated.invalid\n",
    )
    .unwrap();
    let system = root.path().join("system-config");
    files::write(&system, b"Host *\n  ProxyCommand false\n").unwrap();
    let preview = runtime
        .preview_config(&grant.join("config"), "Host companion-app\n  HostName 127.0.0.1\n")
        .unwrap();
    let preview = preview.replace("/etc/ssh/ssh_config", system.to_str().unwrap());
    let path = grant.join("probe");
    files::write(&path, preview.as_bytes()).unwrap();
    let observed = ssh::checked(Command::new("ssh").args(["-G", "-F"]).arg(&path).arg("companion-app")).unwrap();
    assert!(observed.contains("proxycommand false"));
}
