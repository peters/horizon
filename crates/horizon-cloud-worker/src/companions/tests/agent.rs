//! Agent sessions use companion access as the unprivileged account. The fixture
//! account is the test user, so group modes stand in for the agent group.
use super::super::agent::AgentAccount;
use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

fn isolated(root: &Path) -> Runtime {
    let name = ssh::checked(Command::new("id").arg("-un")).unwrap().trim().to_owned();
    Runtime {
        agent_account: Some(AgentAccount {
            name,
            group: rustix::process::getgid().as_raw(),
        }),
        ..runtime(root)
    }
}

/// Connects `grant` as the companion transport does after a successful probe.
fn connect(runtime: &Runtime, grant: &str, alias: &str, host: &str) {
    runtime.apply(&Request::Identity { grant: grant.into() }).unwrap();
    let directory = runtime.key_directory(grant);
    let known_hosts = directory.join("known_hosts-pin");
    files::write(&known_hosts, b"horizon-companion-pair ssh-ed25519 AAAA\n").unwrap();
    let ssh_alias = format!("companion-{alias}");
    let config = ssh::alias_config(
        &ssh_alias,
        &host.parse().unwrap(),
        22,
        &format!("horizon-companion-{grant}"),
        &directory.join("identity"),
        &known_hosts,
    )
    .unwrap();
    files::write(&directory.join("config"), config.as_bytes()).unwrap();
    let response = Response::Connected {
        ssh_alias,
        worktree: format!("/workspace/companions/worktrees/{grant}"),
    };
    files::write(
        &directory.join("connection.json"),
        &serde_json::to_vec(&response).unwrap(),
    )
    .unwrap();
    runtime.update_config().unwrap();
}

/// Resolves `alias` as the agent account does: the system file includes the agent copy.
fn resolve(runtime: &Runtime, root: &Path, alias: &str) -> String {
    let system = root.join("ssh_config");
    std::fs::write(&system, format!("Include {}\n", runtime.system_include.display())).unwrap();
    ssh::checked(Command::new("ssh").arg("-G").arg("-F").arg(&system).arg(alias)).unwrap()
}

fn setting<'a>(resolved: &'a str, name: &str) -> &'a str {
    resolved
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name} ")))
        .unwrap_or_default()
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn agent_sessions_resolve_the_alias_and_read_the_catalog_while_root_material_stays_private() {
    let root = tempfile::tempdir().unwrap();
    let runtime = isolated(root.path());
    files::directory(&runtime.live.join("horizon-host-keys")).unwrap();
    files::write(&runtime.live.join("horizon-host-keys/ed25519"), b"host key").unwrap();
    connect(&runtime, "pair", "app", "127.0.0.1");

    let copy = runtime.agent.join("pair");
    let resolved = resolve(&runtime, root.path(), "companion-app");
    assert_eq!(setting(&resolved, "hostname"), "127.0.0.1");
    assert_eq!(setting(&resolved, "user"), "root");
    assert_eq!(
        setting(&resolved, "identityfile"),
        copy.join("identity").to_str().unwrap()
    );
    assert_eq!(
        setting(&resolved, "userknownhostsfile"),
        copy.join("known_hosts-pin").to_str().unwrap()
    );
    assert_eq!(setting(&resolved, "stricthostkeychecking"), "true");
    // The copied key is the grant's key, readable by the agent group and by nobody else.
    // OpenSSH checks key modes only for its owner, which is root on a worker, not the agent.
    assert_eq!(
        std::fs::read(copy.join("identity")).unwrap(),
        std::fs::read(runtime.key_directory("pair").join("identity")).unwrap()
    );
    let group = runtime.agent_account.as_ref().unwrap().group;
    for path in [
        runtime.agent.join("config"),
        copy.join("identity"),
        copy.join("known_hosts-pin"),
        copy.join("connection.json"),
    ] {
        assert_eq!(mode(&path), 0o640, "{}", path.display());
        assert_eq!(std::fs::metadata(&path).unwrap().gid(), group);
    }
    for directory in [&runtime.agent, &copy] {
        assert_eq!(mode(directory), 0o750);
        assert_eq!(std::fs::metadata(directory).unwrap().gid(), group);
    }
    assert_eq!(mode(&runtime.system_include), 0o644);

    let catalog = Catalog {
        version: horizon_cloud_protocol::companion::VERSION,
        source_cloud_id: "cloud-source".into(),
        observed_at: 100,
        companions: Vec::new(),
    };
    files::write(&runtime.live.join("companions/catalog.json"), b"{}").unwrap();
    runtime
        .publish_catalog_locked(&serde_json::to_vec(&catalog).unwrap())
        .unwrap();
    let published = runtime.agent.join("catalog.json");
    assert_eq!(mode(&published), 0o640);
    assert_eq!(std::fs::metadata(&published).unwrap().gid(), group);
    assert_eq!(
        serde_json::from_slice::<Catalog>(&std::fs::read(&published).unwrap()).unwrap(),
        catalog
    );
    assert!(
        !runtime.live.join("companions/catalog.json").exists(),
        "the root-only catalog is not kept"
    );

    // Root-only material keeps its private modes.
    for directory in [runtime.live.join("companions"), runtime.key_directory("pair")] {
        assert_eq!(mode(&directory), 0o700, "{}", directory.display());
    }
    for path in [
        runtime.key_directory("pair").join("identity"),
        runtime.key_directory("pair").join("config"),
        runtime.live.join("companions/config"),
        runtime.ssh_home.join("config"),
        runtime.live.join("horizon-host-keys/ed25519"),
    ] {
        assert_eq!(mode(&path) & 0o077, 0, "{}", path.display());
    }
    assert!(
        !std::fs::read_to_string(runtime.ssh_home.join("config"))
            .unwrap()
            .contains(runtime.agent.to_str().unwrap()),
        "root never reads the agent copy"
    );
}

#[test]
fn disconnect_and_forget_remove_the_agent_copy_and_keep_other_grants() {
    let root = tempfile::tempdir().unwrap();
    let runtime = isolated(root.path());
    connect(&runtime, "pair", "app", "127.0.0.1");
    connect(&runtime, "other", "service", "127.0.0.2");
    // A stale host-key pin from an earlier connection is not kept.
    files::write(&runtime.agent.join("pair/known_hosts-old"), b"old").unwrap();
    runtime.update_config().unwrap();
    assert!(!runtime.agent.join("pair/known_hosts-old").exists());

    runtime.apply(&Request::Disconnect { grant: "pair".into() }).unwrap();
    assert!(!runtime.agent.join("pair").exists(), "the key copy is withdrawn");
    assert!(
        runtime.key_directory("pair").join("identity").exists(),
        "the root identity waits for target revocation"
    );
    let config = std::fs::read_to_string(runtime.agent.join("config")).unwrap();
    assert!(!config.contains("companion-app") && config.contains("companion-service"));
    assert_eq!(
        setting(&resolve(&runtime, root.path(), "companion-app"), "hostname"),
        "companion-app",
        "the alias no longer resolves"
    );
    assert_eq!(
        setting(&resolve(&runtime, root.path(), "companion-service"), "hostname"),
        "127.0.0.2"
    );

    // Forget also clears a copy that an interrupted disconnect left behind.
    files::shared_directory(&runtime.agent.join("pair"), None).unwrap();
    runtime.apply(&Request::Forget { grant: "pair".into() }).unwrap();
    assert!(!runtime.agent.join("pair").exists());
    assert!(!runtime.key_directory("pair").exists());
    assert!(runtime.agent.join("other/identity").exists());
}

#[test]
fn a_grant_that_cannot_be_copied_does_not_keep_another_withdrawn_grant() {
    let root = tempfile::tempdir().unwrap();
    let runtime = isolated(root.path());
    connect(&runtime, "pair", "app", "127.0.0.1");
    connect(&runtime, "other", "service", "127.0.0.2");
    std::fs::remove_file(runtime.key_directory("other").join("identity")).unwrap();
    assert!(runtime.apply(&Request::Disconnect { grant: "pair".into() }).is_err());
    assert!(!runtime.agent.join("pair").exists());
    assert!(!runtime.agent.join("other").exists(), "an incomplete copy is withdrawn");
    let config = std::fs::read_to_string(runtime.agent.join("config")).unwrap();
    assert!(!config.contains("companion-app") && !config.contains("companion-service"));
    // No catalog claims access that the agent copies do not provide.
    files::write(&runtime.agent.join("catalog.json"), b"{}").unwrap();
    assert!(runtime.publish_catalog_locked(b"{}").is_err());
    assert!(!runtime.agent.join("catalog.json").exists());
}

#[test]
fn catalog_publication_adds_copies_when_a_worker_becomes_isolated() {
    let root = tempfile::tempdir().unwrap();
    connect(&runtime(root.path()), "pair", "app", "127.0.0.1");
    let runtime = isolated(root.path());
    assert!(!runtime.agent.join("pair").exists());
    runtime.publish_catalog_locked(b"{}").unwrap();
    assert!(runtime.agent.join("pair/identity").exists());
    assert_eq!(
        setting(&resolve(&runtime, root.path(), "companion-app"), "hostname"),
        "127.0.0.1"
    );
}

#[test]
fn workers_without_agent_isolation_publish_no_key_copies() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    connect(&runtime, "pair", "app", "127.0.0.1");
    assert_eq!(mode(&runtime.agent), 0o750);
    assert_eq!(std::fs::read_dir(&runtime.agent).unwrap().count(), 0);
    assert!(!runtime.system_include.exists());
}

#[test]
fn a_redirected_agent_directory_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let runtime = isolated(root.path());
    let elsewhere = root.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &runtime.agent).unwrap();
    runtime.apply(&Request::Identity { grant: "pair".into() }).unwrap();
    assert!(runtime.update_config().is_err());
    assert!(runtime.publish_catalog_locked(b"{}").is_err());
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn agent_inspection_reads_the_published_record_without_the_lock() {
    let root = tempfile::tempdir().unwrap();
    let runtime = Runtime {
        privileged: false,
        ..isolated(root.path())
    };
    connect(&runtime, "pair", "app", "127.0.0.1");
    let access = Access {
        grant: "pair".into(),
        ssh_alias: "companion-app".into(),
        worktree: "/workspace/companions/worktrees/pair".into(),
    };
    // The root lock is held throughout, as during companion setup.
    runtime
        .with_lock(|| {
            assert!(runtime.probe_access(&access, || Ok(true)).unwrap());
            Ok(())
        })
        .unwrap();
    let other = Access {
        ssh_alias: "companion-other".into(),
        ..access.clone()
    };
    assert!(!runtime.probe_access(&other, || panic!("probed another alias")).unwrap());
    // A record replaced during the probe withholds the verdict.
    let changed = runtime.probe_access(&access, || {
        files::write_with(
            &runtime.agent.join("pair/connection.json"),
            &serde_json::to_vec(&Response::Disconnected)?,
            0o640,
            None,
        )?;
        Ok(true)
    });
    assert!(is_busy(&changed.unwrap_err()));
    runtime.apply(&Request::Disconnect { grant: "pair".into() }).unwrap();
    assert!(
        runtime
            .probe_access(&access, || panic!("probed a withdrawn grant"))
            .is_err()
    );
}

#[test]
fn ready_requires_that_the_agent_account_can_use_the_alias() {
    let root = tempfile::tempdir().unwrap();
    let launcher = root.path().join("launcher");
    std::fs::write(
        &launcher,
        "#!/bin/sh\n[ \"$1 $2 $3\" = 'agent ssh companion-app' ] && [ \"$4\" = 'git -C /w rev-parse --is-inside-work-tree' ] && echo true\n",
    )
    .unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut runtime = Runtime {
        workspace_launcher: Some(launcher),
        ..isolated(root.path())
    };
    let command = "git -C /w rev-parse --is-inside-work-tree";
    let verify = |runtime: &Runtime, alias: &str| {
        // Another test thread can briefly hold the new script open while it forks.
        for _ in 0..200 {
            match runtime.verify_agent_access(alias, command) {
                Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                result => return result,
            }
        }
        runtime.verify_agent_access(alias, command)
    };
    verify(&runtime, "companion-app").unwrap();
    assert!(verify(&runtime, "companion-other").is_err());
    runtime.workspace_launcher = Some("/usr/bin/false".into());
    assert!(runtime.verify_agent_access("companion-app", command).is_err());
    // Workers without isolation have no separate agent account to check.
    runtime.agent_account = None;
    runtime.verify_agent_access("companion-app", command).unwrap();
}

#[test]
fn the_catalog_path_is_inside_the_published_directory() {
    assert_eq!(
        Path::new(crate::companion_tools::CATALOG).parent(),
        Some(Path::new(PUBLISHED))
    );
}
