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

/// Prepares the identity and the pin of `grant` and returns its alias configuration.
fn prepare(runtime: &Runtime, grant: &str, alias: &str, host: &str) -> String {
    runtime.apply(&Request::Identity { grant: grant.into() }).unwrap();
    let directory = runtime.key_directory(grant);
    let known_hosts = directory.join("known_hosts-pin");
    files::write(&known_hosts, b"horizon-companion-pair ssh-ed25519 AAAA\n").unwrap();
    ssh::alias_config(
        &format!("companion-{alias}"),
        &host.parse().unwrap(),
        22,
        &format!("horizon-companion-{grant}"),
        &directory.join("identity"),
        &known_hosts,
    )
    .unwrap()
}

fn connected(grant: &str, alias: &str) -> Response {
    Response::Connected {
        ssh_alias: format!("companion-{alias}"),
        worktree: format!("/workspace/companions/worktrees/{grant}"),
    }
}

/// Connects `grant` as the companion transport does after a successful probe.
fn connect(runtime: &Runtime, grant: &str, alias: &str, host: &str) {
    let config = prepare(runtime, grant, alias, host);
    let directory = runtime.key_directory(grant);
    files::write(&directory.join("config"), config.as_bytes()).unwrap();
    let response = connected(grant, alias);
    files::write(
        &directory.join("connection.json"),
        &serde_json::to_vec(&response).unwrap(),
    )
    .unwrap();
    runtime.update_config().unwrap();
}

/// The system file of the agent account, which includes the agent copy.
fn system_config(runtime: &Runtime, root: &Path) -> PathBuf {
    let system = root.join("ssh_config");
    std::fs::write(&system, format!("Include {}\n", runtime.system_include.display())).unwrap();
    system
}

/// Resolves `alias` as the agent account does.
fn resolve(runtime: &Runtime, root: &Path, alias: &str) -> String {
    let system = system_config(runtime, root);
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

/// An isolation launcher that runs `script` with the launcher arguments.
fn launcher(root: &Path, script: &str) -> PathBuf {
    let path = root.join("launcher");
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

/// Every file under `root` whose content contains `secret`.
fn files_containing(root: &Path, secret: &[u8]) -> Vec<PathBuf> {
    let mut holders = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if std::fs::read(&path)
                .unwrap()
                .windows(secret.len())
                .any(|window| window == secret)
            {
                holders.push(path);
            }
        }
    }
    holders
}

/// Another test thread can briefly hold a new launcher script open while it forks.
fn retry_busy<T>(operation: impl Fn() -> io::Result<T>) -> io::Result<T> {
    for _ in 0..200 {
        match operation() {
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => return result,
        }
    }
    operation()
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
    let mut runtime = isolated(root.path());
    let staged = runtime.staged_probe("pair");
    runtime.workspace_launcher = Some(launcher(
        root.path(),
        &format!(
            "[ \"$1 $2 $3 $4 $5\" = 'agent ssh -G -F {}/config' ] && shift && exec \"$@\"",
            staged.display()
        ),
    ));
    let config = prepare(&runtime, "pair", "app", "127.0.0.1");
    let verify = |runtime: &Runtime, alias: &str| {
        retry_busy(|| runtime.verify_agent_route("pair", &config, alias, Duration::from_secs(30)))
    };
    verify(&runtime, "companion-app").unwrap();
    assert!(!staged.exists(), "the staged configuration is removed after the check");
    // Another alias does not resolve to the copy of this grant.
    assert!(verify(&runtime, "companion-other").is_err());
    assert!(!staged.exists(), "a failed check also removes the staged configuration");
    // An exhausted Connect budget refuses Ready without waiting.
    assert!(
        runtime
            .verify_agent_route("pair", &config, "companion-app", Duration::ZERO)
            .is_err()
    );
    runtime.workspace_launcher = Some("/usr/bin/false".into());
    assert!(
        runtime
            .verify_agent_route("pair", &config, "companion-app", Duration::from_secs(30))
            .is_err()
    );
    assert!(
        !runtime.agent.join("pair").exists(),
        "a probe never publishes the grant"
    );
    // An alias that the published configuration does not resolve refuses Ready.
    assert!(
        runtime
            .verify_published_alias("pair", "companion-app", Duration::from_secs(30))
            .is_err()
    );
    // Workers without isolation have no separate agent account to check.
    runtime.agent_account = None;
    runtime
        .verify_agent_route("pair", &config, "companion-app", Duration::from_secs(30))
        .unwrap();
    runtime
        .verify_published_alias("pair", "companion-app", Duration::from_secs(30))
        .unwrap();
}

#[test]
fn an_interrupted_connect_never_publishes_its_grant_to_agents() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = isolated(root.path());
    connect(&runtime, "other", "service", "127.0.0.2");
    // The agent probe saves the worker state, as an interruption during the probe
    // leaves it. The final check resolves the alias through the agent system file.
    let snapshot = root.path().join("snapshot");
    let system = system_config(&runtime, root.path());
    runtime.workspace_launcher = Some(launcher(
        root.path(),
        &format!(
            "shift\nif [ \"$3\" != -F ]; then exec ssh -G -F '{}' \"$3\"; fi\nmkdir '{snapshot}' && cp -a '{}' '{}' '{snapshot}/' && exec \"$@\"",
            system.display(),
            runtime.live.display(),
            runtime.agent.display(),
            snapshot = snapshot.display()
        ),
    ));
    let config = prepare(&runtime, "pair", "app", "127.0.0.1");
    let response = connected("pair", "app");
    retry_busy(|| {
        runtime.commit_connection(
            "pair",
            &config,
            &response,
            std::time::Instant::now() + Duration::from_secs(30),
        )
    })
    .unwrap();
    assert!(runtime.agent.join("pair/connection.json").exists());
    assert!(!runtime.staged_probe("pair").exists());
    assert_eq!(
        setting(&resolve(&runtime, root.path(), "companion-app"), "hostname"),
        "127.0.0.1"
    );

    // During the probe, only the staged copy named the alias.
    let saved_agent = snapshot.join("agent");
    let saved_grant = snapshot.join("run/companions/pair");
    assert!(!saved_grant.join("config").exists() && !saved_grant.join("connection.json").exists());
    assert!(!saved_agent.join("pair").exists());
    assert!(
        !std::fs::read_to_string(saved_agent.join("config"))
            .unwrap()
            .contains("companion-app")
    );
    let staged = ssh::checked(
        Command::new("ssh")
            .args(["-G", "-F"])
            .arg(saved_agent.join("pair.probe/config"))
            .arg("companion-app"),
    )
    .unwrap();
    assert_eq!(setting(&staged, "hostname"), "127.0.0.1");
    assert_eq!(
        setting(&staged, "identityfile"),
        runtime.agent.join("pair/identity").to_str().unwrap()
    );
    // Before the record, only the root-only grant directory holds the key.
    let key = std::fs::read(runtime.key_directory("pair").join("identity")).unwrap();
    assert_eq!(files_containing(&snapshot, &key), [saved_grant.join("identity")]);
    assert_eq!(mode(&saved_grant), 0o700);
    assert_eq!(mode(&saved_grant.join("identity")) & 0o077, 0);
    assert_eq!(mode(&snapshot.join("run/companions")), 0o700);

    // The worker stops after the probe-config write and before connection.json:
    // during the probe, or after the config of the grant replaced the old one.
    for (saved, live) in [(snapshot.join("run"), &runtime.live), (saved_agent, &runtime.agent)] {
        std::fs::remove_dir_all(live).unwrap();
        std::fs::rename(saved, live).unwrap();
    }
    files::write(&runtime.key_directory("pair").join("config"), config.as_bytes()).unwrap();
    runtime.publish_catalog_locked(b"{}").unwrap();
    assert!(
        !runtime.agent.join("pair").exists(),
        "no key copy for an unfinished Connect"
    );
    assert!(
        !runtime.staged_probe("pair").exists(),
        "reconciliation removes the staged copy"
    );
    let published = std::fs::read_to_string(runtime.agent.join("config")).unwrap();
    assert!(!published.contains("companion-app") && published.contains("companion-service"));
    assert_eq!(
        setting(&resolve(&runtime, root.path(), "companion-app"), "hostname"),
        "companion-app",
        "the alias does not resolve for agents"
    );
    assert!(runtime.agent.join("other/identity").exists());
    assert!(runtime.agent.join("catalog.json").exists());

    // A record for another alias does not publish this configuration either.
    files::write(
        &runtime.key_directory("pair").join("connection.json"),
        &serde_json::to_vec(&connected("pair", "renamed")).unwrap(),
    )
    .unwrap();
    runtime.publish_agent_access().unwrap();
    assert!(!runtime.agent.join("pair").exists());

    // Revocation still removes everything of the grant.
    runtime.apply(&Request::Disconnect { grant: "pair".into() }).unwrap();
    runtime.apply(&Request::Forget { grant: "pair".into() }).unwrap();
    assert!(!runtime.key_directory("pair").exists());
    assert!(!runtime.agent.join("pair").exists());
    assert!(runtime.agent.join("other/identity").exists());
}

#[test]
fn a_refresh_keeps_the_published_alias_and_restores_it_when_the_refresh_fails() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = isolated(root.path());
    connect(&runtime, "pair", "app", "127.0.0.1");
    // The probe saves how the agent resolves the alias while the candidate is probed.
    let system = system_config(&runtime, root.path());
    let during = root.path().join("during");
    let fail_probe = root.path().join("fail-probe");
    let fail_resolve = root.path().join("fail-resolve");
    runtime.workspace_launcher = Some(launcher(
        root.path(),
        &format!(
            "shift\nif [ \"$3\" != -F ]; then [ -e '{fail_resolve}' ] && exit 1; exec ssh -G -F '{system}' \"$3\"; fi\n\
             ssh -G -F '{system}' \"$5\" > '{during}' || exit 1\n[ -e '{fail_probe}' ] && exit 1\nexec \"$@\"",
            fail_resolve = fail_resolve.display(),
            system = system.display(),
            during = during.display(),
            fail_probe = fail_probe.display(),
        ),
    ));
    let refresh = |host: &str| {
        let config = prepare(&runtime, "pair", "app", host);
        retry_busy(|| {
            runtime.commit_connection(
                "pair",
                &config,
                &connected("pair", "app"),
                std::time::Instant::now() + Duration::from_secs(30),
            )
        })
    };
    let hostname = |resolved: &str| setting(resolved, "hostname").to_owned();
    let published = || {
        let directory = runtime.key_directory("pair");
        (
            std::fs::read(directory.join("config")).unwrap(),
            std::fs::read(directory.join("connection.json")).unwrap(),
            std::fs::read(runtime.agent.join("pair/identity")).unwrap(),
        )
    };

    // A refresh keeps the alias usable for agents during the probe.
    refresh("127.0.0.3").unwrap();
    assert_eq!(hostname(&std::fs::read_to_string(&during).unwrap()), "127.0.0.1");
    assert_eq!(hostname(&resolve(&runtime, root.path(), "companion-app")), "127.0.0.3");
    let before = published();

    // A failed probe on a refresh keeps the old copy and record.
    std::fs::write(&fail_probe, "").unwrap();
    assert!(refresh("127.0.0.4").is_err());
    assert_eq!(hostname(&std::fs::read_to_string(&during).unwrap()), "127.0.0.3");
    assert_eq!(published(), before);
    assert_eq!(hostname(&resolve(&runtime, root.path(), "companion-app")), "127.0.0.3");
    assert!(!runtime.staged_probe("pair").exists());

    // A failure after the new record was written restores the previous connection.
    std::fs::remove_file(&fail_probe).unwrap();
    std::fs::write(&fail_resolve, "").unwrap();
    assert!(refresh("127.0.0.4").is_err());
    assert_eq!(published(), before);
    assert_eq!(hostname(&resolve(&runtime, root.path(), "companion-app")), "127.0.0.3");

    // A first Connect that fails publishes nothing and keeps nothing.
    std::fs::remove_file(&fail_resolve).unwrap();
    std::fs::write(&fail_probe, "").unwrap();
    let config = prepare(&runtime, "fresh", "fresh", "127.0.0.5");
    assert!(
        retry_busy(|| {
            runtime.commit_connection(
                "fresh",
                &config,
                &connected("fresh", "fresh"),
                std::time::Instant::now() + Duration::from_secs(30),
            )
        })
        .is_err()
    );
    assert!(!runtime.key_directory("fresh").join("config").exists());
    assert!(!runtime.agent.join("fresh").exists() && !runtime.staged_probe("fresh").exists());
    assert_eq!(
        hostname(&resolve(&runtime, root.path(), "companion-fresh")),
        "companion-fresh"
    );
}

#[test]
fn the_catalog_path_is_inside_the_published_directory() {
    assert_eq!(
        Path::new(crate::companion_tools::CATALOG).parent(),
        Some(Path::new(PUBLISHED))
    );
}
