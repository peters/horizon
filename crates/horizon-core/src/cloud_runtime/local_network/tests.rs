use super::*;
#[cfg(unix)]
use std::time::{Duration, Instant};
use std::{
    net::Ipv4Addr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn subnet() -> Subnet {
    "192.168.1.0/24".parse().unwrap()
}

fn home() -> scope::Host {
    scope::tests::host(&[("192.168.1.20", Some(24))], Some("192.168.1.20"))
}

fn network() -> scope::Network {
    home().current_network().unwrap()
}

/// A gate on the home network whose every destination routes out of the bridged interface.
fn gate(resolve: Resolve) -> Scope {
    Scope {
        network: network(),
        resolve,
        host: Box::new(|| Ok(home())),
        source: Box::new(|_| Some(Ipv4Addr::new(192, 168, 1, 20))),
    }
}

fn name(value: &str, port: u16) -> Destination {
    Destination::Name(value.into(), port)
}

fn addresses(values: &[&str]) -> Vec<SocketAddr> {
    values.iter().map(|value| value.parse().unwrap()).collect()
}

#[test]
fn a_name_is_resolved_once_and_only_its_checked_addresses_are_returned() {
    // A rebinding resolver answers in scope first and with loopback afterwards.
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let gate = gate(Box::new(move |_, port| {
        let first = counted.fetch_add(1, Ordering::SeqCst) == 0;
        let ip = if first { [192, 168, 1, 50] } else { [127, 0, 0, 1] };
        Ok(vec![SocketAddr::new(Ipv4Addr::from(ip).into(), port)])
    }));
    let destination = name("camera.example", 554);
    assert_eq!(Scope::admit(&gate, &destination), Ok(addresses(&["192.168.1.50:554"])));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(Scope::admit(&gate, &destination), Err(Reply::NotAllowed));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn names_resolving_outside_the_scope_are_refused_and_mixed_answers_are_filtered() {
    let gate = gate(Box::new(|name, port| {
        Ok(match name {
            "outside.example" => addresses(&["8.8.8.8:0", "127.0.0.1:0", "[::1]:0", "192.168.1.20:0"]),
            "mixed.example" => addresses(&["10.0.0.5:0", "[fe80::1]:0", "192.168.1.60:0", "[::ffff:192.168.1.61]:0"]),
            _ => return Err(Reply::HostUnreachable),
        }
        .into_iter()
        .map(|address| SocketAddr::new(address.ip(), port))
        .collect())
    }));
    assert_eq!(
        Scope::admit(&gate, &name("outside.example", 80)),
        Err(Reply::NotAllowed)
    );
    assert_eq!(
        Scope::admit(&gate, &name("mixed.example", 80)),
        Ok(addresses(&["192.168.1.60:80", "192.168.1.61:80"]))
    );
    assert_eq!(
        Scope::admit(&gate, &name("missing.example", 80)),
        Err(Reply::HostUnreachable)
    );
}

#[test]
fn addresses_are_checked_without_resolving_and_attempts_are_bounded() {
    let gate = gate(Box::new(|_, port| {
        Ok((50..60)
            .map(|host| SocketAddr::new(Ipv4Addr::new(192, 168, 1, host).into(), port))
            .collect())
    }));
    assert_eq!(
        Scope::admit(&gate, &name("many.example", 22)).unwrap().len(),
        MAX_ATTEMPTS
    );
    let direct = |value: &str| Scope::admit(&gate, &Destination::Address(value.parse().unwrap()));
    assert_eq!(direct("192.168.1.50:22"), Ok(addresses(&["192.168.1.50:22"])));
    assert_eq!(direct("[::ffff:192.168.1.50]:22"), Ok(addresses(&["192.168.1.50:22"])));
    for refused in [
        "192.168.1.20:22",
        "127.0.0.1:22",
        "[::1]:22",
        "192.168.2.1:22",
        "192.168.1.255:22",
    ] {
        assert_eq!(direct(refused), Err(Reply::NotAllowed), "{refused}");
    }
}

#[test]
fn a_changed_network_is_reported_instead_of_connecting() {
    let changed = Scope {
        host: Box::new(|| Ok(scope::tests::host(&[("10.1.0.9", Some(24))], Some("10.1.0.9")))),
        ..gate(Box::new(|_, _| Ok(Vec::new())))
    };
    // Every destination reports the change, and names are not even looked up.
    for destination in [
        Destination::Address("192.168.1.50:80".parse().unwrap()),
        Destination::Address("8.8.8.8:80".parse().unwrap()),
        Destination::Address("[fe80::1]:80".parse().unwrap()),
        name("missing.example", 80),
    ] {
        assert_eq!(
            changed.admit(&destination),
            Err(Reply::NetworkUnreachable),
            "{destination:?}"
        );
    }
    // Leaving the network while a name resolves refuses it too.
    let reads = Arc::new(AtomicUsize::new(0));
    let during = Scope {
        host: Box::new(move || {
            Ok(if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                home()
            } else {
                scope::tests::host(&[("10.1.0.9", Some(24))], Some("10.1.0.9"))
            })
        }),
        ..gate(Box::new(|_, port| {
            Ok(addresses(&["192.168.1.50:0"])
                .into_iter()
                .map(|a| SocketAddr::new(a.ip(), port))
                .collect())
        }))
    };
    assert_eq!(
        during.admit(&name("camera.example", 554)),
        Err(Reply::NetworkUnreachable)
    );
    let unreadable = Scope {
        host: Box::new(|| Err(io::Error::other("interfaces unavailable"))),
        ..gate(Box::new(|_, _| Ok(Vec::new())))
    };
    let routed_elsewhere = Scope {
        source: Box::new(|_| Some(Ipv4Addr::new(10, 8, 0, 2))),
        ..gate(Box::new(|_, _| Ok(Vec::new())))
    };
    assert_eq!(
        Scope::admit(
            &routed_elsewhere,
            &Destination::Address("192.168.1.50:80".parse().unwrap())
        ),
        Err(Reply::NotAllowed)
    );
    assert_eq!(
        Scope::admit(&unreadable, &Destination::Address("192.168.1.50:80".parse().unwrap())),
        Err(Reply::GeneralFailure)
    );
}

#[test]
fn the_current_scope_and_the_resolver_work_on_this_computer() {
    assert_eq!(gate(Box::new(|_, _| Ok(Vec::new()))).subnet(), subnet());
    // This computer's own network decides whether a real scope exists here.
    match Scope::current() {
        Ok(scope) => assert!(scope.subnet().prefix() >= 16),
        // No shareable network, or interfaces a sandbox does not let the test read.
        Err(StartError::Scope(_) | StartError::Io(_)) => {}
    }
    let lookups = Arc::new(AtomicUsize::new(0));
    assert!(
        resolve(&lookups, "localhost", 80).is_ok_and(|addresses| addresses.iter().all(|address| address.port() == 80))
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while lookups.load(Ordering::Acquire) != 0 {
        assert!(std::time::Instant::now() < deadline, "the lookup kept its share");
        std::thread::yield_now();
    }
    // Lookups still running, even abandoned ones, hold their share of the limit.
    lookups.store(MAX_LOOKUPS, Ordering::Release);
    assert_eq!(resolve(&lookups, "localhost", 80), Err(Reply::GeneralFailure));
    assert_eq!(lookups.load(Ordering::Acquire), MAX_LOOKUPS);
}

#[test]
fn the_proxy_reports_its_subnet_port_and_counters() {
    let proxy = Proxy::with_gate(subnet(), Arc::new(gate(Box::new(|_, _| Ok(Vec::new()))))).unwrap();
    assert_eq!(proxy.subnet(), subnet());
    assert_ne!(proxy.port(), 0);
    assert_eq!(proxy.counters(), Counters::default());
}

#[cfg(unix)]
fn wait_for_state(bridge: &Bridge, mut matches: impl FnMut(&State) -> bool) -> State {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = bridge.status().state;
        if matches(&state) {
            return state;
        }
        assert!(Instant::now() < deadline, "bridge stayed {state:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Shell stand-ins for the worker; they need a POSIX shell.
#[cfg(unix)]
mod supervision {
    use super::*;
    use std::{path::PathBuf, process::Command};

    struct Script {
        prepare: String,
        hold: String,
        log: PathBuf,
    }

    impl session::Transport for Script {
        fn prepare(&self) -> Command {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", &self.prepare]);
            command
        }

        fn hold(&self, nonce: &horizon_cloud_protocol::local_network::Nonce, subnet: Subnet, port: u16) -> Command {
            let mut command = Command::new("/bin/sh");
            command
                .args([
                    "-c",
                    &self.hold,
                    "hold",
                    nonce.as_str(),
                    &subnet.to_string(),
                    &port.to_string(),
                ])
                .env("LOG", &self.log);
            command
        }

        fn heartbeat(&self) -> Duration {
            Duration::from_millis(100)
        }
    }

    fn start(prepare: &str, hold: &str) -> (Bridge, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let script = Script {
            prepare: prepare.into(),
            hold: hold.into(),
            log: root.path().join("log"),
        };
        let proxy = Proxy::with_gate(subnet(), Arc::new(gate(Box::new(|_, _| Ok(Vec::new()))))).unwrap();
        let bridge = Bridge::with_parts(proxy, script).unwrap();
        (bridge, root)
    }

    const PREPARED: &str = "printf 'horizon-local-network=1\\n'";
    const HOLD: &str = r#"printf '%s %s %s\n' "$1" "$2" "$3" >> "$LOG"; printf '{"proxy":"127.0.0.1:41234"}\n'; while read -r _; do printf 'beat\n' >> "$LOG"; done"#;

    fn log(root: &tempfile::TempDir) -> String {
        std::fs::read_to_string(root.path().join("log")).unwrap_or_default()
    }

    #[test]
    fn a_confirmed_session_is_active_and_receives_heartbeats() {
        let (bridge, root) = start(PREPARED, HOLD);
        assert_eq!(
            wait_for_state(&bridge, |state| matches!(state, State::Active { .. })),
            State::Active {
                proxy: "127.0.0.1:41234".parse().unwrap()
            }
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while log(&root).matches("beat").count() < 2 {
            assert!(Instant::now() < deadline, "no heartbeats: {}", log(&root));
            std::thread::sleep(Duration::from_millis(20));
        }
        let first = log(&root);
        let arguments: Vec<_> = first.lines().next().unwrap().split(' ').collect();
        assert_eq!(arguments[0].len(), 32);
        assert_eq!(arguments[1], "192.168.1.0/24");
        assert_eq!(arguments[2].parse::<u16>().unwrap(), bridge.proxy.port());
        let status = bridge.status();
        assert_eq!((status.subnet, status.counters), (subnet(), Counters::default()));
    }

    #[test]
    fn an_image_without_the_helper_fails_without_retrying() {
        for old in [
            "printf '%s\\n' 'Cloud worker service: Usage: horizon-cloud-worker serve|connect'",
            "printf '%s\\n' 'bash: line 1: horizon-cloud-worker: command not found'",
        ] {
            let (bridge, root) = start(old, HOLD);
            assert_eq!(
                wait_for_state(&bridge, |state| matches!(state, State::Failed { .. })),
                State::Failed {
                    error: session::UNSUPPORTED.into()
                }
            );
            std::thread::sleep(Duration::from_millis(300));
            assert_eq!(log(&root), "");
        }
        // A supported helper reporting something else missing is retried.
        let (bridge, _root) = start("printf '%s\\n' 'Directory not found'", HOLD);
        assert!(matches!(
            wait_for_state(&bridge, |state| !matches!(state, State::Starting)),
            State::Reconnecting { .. }
        ));
    }

    #[test]
    fn a_lost_session_reports_why_and_starts_again_with_a_fresh_nonce() {
        let hold = r#"printf '%s\n' "$1" >> "$LOG"; printf '{"proxy":"127.0.0.1:41234"}\n'; printf 'Connection closed\033[2J by remote host\377\n' >&2; exit 255"#;
        let (bridge, root) = start(PREPARED, hold);
        assert_eq!(
            wait_for_state(&bridge, |state| matches!(state, State::Reconnecting { .. })),
            State::Reconnecting {
                error: "Connection closed[2J by remote host\u{fffd}".into()
            }
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while log(&root).lines().count() < 2 {
            assert!(Instant::now() < deadline, "no retry");
            std::thread::sleep(Duration::from_millis(50));
        }
        let nonces: Vec<_> = log(&root).lines().map(str::to_owned).collect();
        assert_ne!(nonces[0], nonces[1]);
    }

    #[test]
    fn an_unreachable_worker_and_a_silent_helper_are_retried() {
        let (bridge, _root) = start("exit 255", HOLD);
        assert_eq!(
            wait_for_state(&bridge, |state| matches!(state, State::Reconnecting { .. })),
            State::Reconnecting {
                error: "Cannot reach the worker over SSH".into()
            }
        );
        drop(bridge);
        let (bridge, _root) = start("printf '%s\\n' 'Read-only file system'", HOLD);
        assert_eq!(
            wait_for_state(&bridge, |state| matches!(state, State::Reconnecting { .. })),
            State::Reconnecting {
                error: "The worker could not prepare the bridge: Read-only file system".into()
            }
        );
        drop(bridge);
        let (bridge, _root) = start(PREPARED, "exec cat > /dev/null");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(bridge.status().state, State::Starting);
    }

    #[test]
    fn stopping_the_bridge_ends_the_session_process() {
        let hold = r#"printf '%s\n' "$$" >> "$LOG"; printf '{"proxy":"127.0.0.1:41234"}\n'; exec sleep 600"#;
        let (bridge, root) = start(PREPARED, hold);
        wait_for_state(&bridge, |state| matches!(state, State::Active { .. }));
        let pid = log(&root).trim().to_owned();
        let started = Instant::now();
        drop(bridge);
        assert!(started.elapsed() < Duration::from_secs(10));
        let alive = Command::new("kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "session process {pid} survived the bridge");
    }
}

/// A real `ssh -R` against a user-level OpenSSH server. CI runners do not provide `sshd`;
/// run with `HORIZON_TEST_SSHD=/path/to/sshd cargo test -p horizon-core local_network -- --ignored`.
#[cfg(unix)]
mod end_to_end;
