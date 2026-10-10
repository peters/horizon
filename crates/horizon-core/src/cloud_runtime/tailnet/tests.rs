use super::*;
use crate::cloud_runtime::{Cancellation, worker_contract::WorkerContract};
use std::cell::Cell;

#[test]
fn resumed_selected_tailnet_rejects_legacy_contract_before_ssh_or_state_changes() {
    let temporary = tempfile::tempdir().unwrap();
    let selection = Selection {
        tailnet: Some("synthetic-tailnet".into()),
    };
    selection.commit(temporary.path()).unwrap();
    let before = std::fs::read(temporary.path().join("tailnet.json")).unwrap();
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let connection = Connection {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        identity: temporary.path().join("identity"),
        known_hosts: temporary.path().join("known-hosts"),
        host_key_alias: "synthetic-worker".into(),
    };
    let cancel = Cancellation::default();
    let events = Cell::new(0);
    let emit = |_| events.set(events.get() + 1);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    for markers in [
        "",
        "horizon-tailnet-contract=1\n",
        "horizon-tailnet-contract=1\nhorizon-tailnet-contract=2\n",
        "horizon-tailnet-contract=3\n",
    ] {
        let contract = WorkerContract::reported(markers);
        let result = configure(&connection, temporary.path(), &runner, &contract);
        assert!(
            matches!(result,Err(Error::Invalid(message))if message.contains("cannot enforce tagged tailnet enrollment"))
        );
        assert_eq!(events.get(), 0, "a rejected contract must not run a remote helper");
        assert_eq!(std::fs::read(temporary.path().join("tailnet.json")).unwrap(), before);
        assert!(!connection.known_hosts.exists());
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
            "an incompatible resumed worker must not receive an SSH connection"
        );
    }
}

#[test]
fn none_or_absent_tailnet_preserves_legacy_worker_reconnect_without_ssh() {
    let temporary = tempfile::tempdir().unwrap();
    let connection = Connection {
        host: "invalid.example".into(),
        port: 22,
        identity: temporary.path().join("identity"),
        known_hosts: temporary.path().join("known-hosts"),
        host_key_alias: "synthetic-worker".into(),
    };
    let cancel = Cancellation::default();
    let events = Cell::new(0);
    let emit = |_| events.set(events.get() + 1);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    configure(&connection, temporary.path(), &runner, &WorkerContract::default()).unwrap();
    Selection::default().commit(temporary.path()).unwrap();
    configure(&connection, temporary.path(), &runner, &WorkerContract::default()).unwrap();
    assert_eq!(events.get(), 0);
}

#[test]
fn selected_tailnet_accepts_only_exact_current_privilege_and_sole_tag_markers() {
    let selection = Selection {
        tailnet: Some("synthetic-tailnet".into()),
    };
    let current = WorkerContract::reported("horizon-tailnet-contract=1\nhorizon-tailnet-contract=3\n");
    validate_worker_contract(&selection, &current).unwrap();
    for markers in [
        "horizon-tailnet-contract=1\nhorizon-tailnet-contract=3-extra\n",
        "horizon-tailnet-contract=1\n horizon-tailnet-contract=3\n",
    ] {
        assert!(validate_worker_contract(&selection, &WorkerContract::reported(markers)).is_err());
    }
}

    #[test]
    fn selection_actual_names_and_unavailable_legacy_snapshots() {
        let cancel = horizon_cloud::Cancellation::default();
        assert!(
            observe_device_name(false, "cloud1", true, &cancel, || panic!(
                "no network read without a tailnet"
            ))
            .unwrap()
            .is_none()
        );
        let actual = observe_device_name(true, "cloud1", false, &cancel, || {
            Ok(br#"{"devices":[{"name":"renamed-1.example.ts.net."}]}"#.to_vec())
        })
        .unwrap()
        .unwrap();
        assert_eq!(actual.name, "renamed-1.example.ts.net");
        assert!(actual.observed);
        for stable in [false, true] {
            let missing =
                observe_device_name(true, "cloud1", stable, &cancel, || Err(Error::PrivateTransport)).unwrap();
            assert_eq!(missing.is_some(), stable);
        }
        let canceled = observe_device_name(true, "cloud1", true, &cancel, || {
            cancel.cancel();
            Err(Error::PrivateTransport)
        });
        assert!(canceled.is_err(), "cancellation never supplies a success fallback");
    }

    #[test]
    #[cfg(unix)] // Synthetic shell output exercises the Unix process transport.
    fn the_snapshot_read_is_bounded_without_log_events() {
        let cancel = horizon_cloud::Cancellation::default();
        let events = std::sync::Mutex::new(Vec::new());
        let emit = |event| events.lock().unwrap().push(event);
        let runner = Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: vec![],
        };
        let identity = observe_device_name(true, "cloud1", true, &cancel, || {
            runner.private_exchange(
                std::process::Command::new("sh").args(["-c", "head -c 65537 /dev/zero"]),
                &[],
                Duration::from_secs(5),
            )
        })
        .unwrap()
        .unwrap();
        assert!(!identity.observed);
        assert_eq!(identity.name, "horizon-cloud-cloud1");
        assert!(
            events.lock().unwrap().is_empty(),
            "peer contents stay out of deployment logs"
        );
    }

    #[test]
    #[cfg(unix)] // The worker's Python reader runs against an isolated Unix snapshot fixture.
    fn a_fresh_snapshot_requires_two_publications_and_cancellation_never_falls_back() {
        const OBSERVE_READS: &str = r#"import os, sys
original = os.fstat
def observe(fd):
    result = original(fd)
    with open(sys.argv[3], "a") as output:
        output.write(str(result.st_ino) + "\n")
    return result
os.fstat = observe
exec(sys.stdin.read())
"#;
        let temp = tempfile::tempdir().unwrap();
        let snapshot = temp.path().join("devices.json");
        let reads = temp.path().join("reads");
        let cancel = horizon_cloud::Cancellation::default();
        for cancelled in [false, true] {
            let initial = publish_fixture(&snapshot, "old.example.ts.net");
            std::fs::write(&reads, "").unwrap();
            std::thread::scope(|scope| {
                let (send, receive) = std::sync::mpsc::channel();
                let fixture = &snapshot;
                let observations = &reads;
                let cancellation = &cancel;
                scope.spawn(move || {
                    let runner = Runner {
                        cancel: cancellation,
                        emit: &|_| panic!("snapshot reads never emit peer data"),
                        secrets: vec![],
                    };
                    let result = observe_device_name(true, "cloud1", true, cancellation, || {
                        runner.private_exchange(
                            std::process::Command::new("python3")
                                .args(["-c", OBSERVE_READS])
                                .arg(fixture)
                                .arg("12")
                                .arg(observations),
                            FRESH_SNAPSHOT.as_bytes(),
                            Duration::from_secs(15),
                        )
                    });
                    send.send(result).unwrap();
                });
                await_fixture_read(&reads, initial);
                // An in-flight status query may still publish its pre-configure name.
                let older = publish_fixture(&snapshot, "in-flight.example.ts.net");
                await_fixture_read(&reads, older);
                assert!(
                    receive.try_recv().is_err(),
                    "one publication cannot establish freshness"
                );
                if cancelled {
                    cancel.cancel();
                    assert!(receive.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
                } else {
                    publish_fixture(&snapshot, "renamed-worker-7.example.ts.net.");
                    let actual = receive.recv_timeout(Duration::from_secs(5)).unwrap().unwrap().unwrap();
                    assert_eq!(actual.name, "renamed-worker-7.example.ts.net");
                    assert!(actual.observed);
                }
            });
        }
    }

    #[test]
    #[cfg(unix)] // A stopped Unix publisher cannot make a cached name a fresh observation.
    fn an_unconfirmed_snapshot_returns_only_the_contract_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let snapshot = temp.path().join("devices.json");
        publish_fixture(&snapshot, "cached.example.ts.net");
        let cancel = horizon_cloud::Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| panic!("snapshot reads never emit peer data"),
            secrets: vec![],
        };
        for stable in [false, true] {
            let identity = observe_device_name(true, "cloud1", stable, &cancel, || {
                runner.private_exchange(
                    std::process::Command::new("python3").arg("-").arg(&snapshot).arg("12"),
                    FRESH_SNAPSHOT.as_bytes(),
                    Duration::from_secs(1),
                )
            })
            .unwrap();
            assert_eq!(identity.is_some(), stable);
            if let Some(identity) = identity {
                assert_eq!(identity.name, "horizon-cloud-cloud1");
                assert!(!identity.observed);
            }
        }
    }

    #[cfg(unix)]
    fn publish_fixture(path: &Path, name: &str) -> u64 {
        use std::os::unix::fs::MetadataExt;
        let pending = path.with_extension("pending");
        std::fs::write(&pending, serde_json::json!({"devices": [{"name": name}]}).to_string()).unwrap();
        std::fs::rename(pending, path).unwrap();
        std::fs::metadata(path).unwrap().ino()
    }

    #[cfg(unix)]
    fn await_fixture_read(path: &Path, inode: u64) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if std::fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .any(|line| line == inode.to_string())
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "reader did not observe the fixture generation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn snapshot_keeps_the_first_devices_actual_dns_name() {
        let snapshot = br#"{"devices":[{"name":"renamed-worker-1.example.ts.net.","online":false},{"name":"peer.example.ts.net."}]}"#;
        let identity = DeviceName::published(snapshot).unwrap();
        assert_eq!(identity.name, "renamed-worker-1.example.ts.net");
        assert!(identity.observed);
        assert!(identity.full_name());
        let bare = DeviceName::published(br#"{"devices":[{"name":"old-random-container"}]}"#).unwrap();
        assert!(!bare.full_name());
    }

    #[test]
    fn an_invalid_self_never_uses_a_peers_name() {
        for json in [
            br#"{"devices":[]}"#.as_slice(),
            br#"{"devices":[{"name":""},{"name":"peer.example.ts.net"}]}"#,
            br#"{"devices":[{"name":"bad/name"}]}"#,
            br#"{"devices":[{"name":"bad\nname"}]}"#,
            br#"{"devices":[{"name":"-name.example"}]}"#,
            br#"{"devices":[{"name":"name..example"}]}"#,
            br#"{"devices":[{"name":"name.example.."}]}"#,
            br#"{"devices":[{"name":"name.example..."}]}"#,
            b"invalid",
        ] {
            assert!(DeviceName::published(json).is_none());
        }
    }

    #[test]
    fn only_v2_derives_a_name_and_matches_the_worker_helper() {
        assert!(DeviceName::derived("cloud-123", false).is_none());
        assert!(DeviceName::derived("invalid/cloud", true).is_none());
        let direct = DeviceName::derived("cloud-123", true).unwrap();
        assert_eq!(direct.name, "horizon-cloud-cloud-123");
        assert!(!direct.observed);
        assert!(!direct.full_name());
        for id in ["Mixed_ID", "name-", "a-0123456789abcdef0123", &"a".repeat(100)] {
            let name = DeviceName::derived(id, true).unwrap().name;
            assert!(valid_dns_name(&name));
            assert!(name.len() <= 63);
        }
        for (id, expected) in [
            ("Mixed_ID", "horizon-cloud-mixed-id-12af5122d9932e2b2577"),
            ("name-", "horizon-cloud-name-f0a4e7e61161383a47dc"),
            (
                "a-0123456789abcdef0123",
                "horizon-cloud-a-0123456789abcdef0123-ab1695dda518dd53fce3",
            ),
            (
                &"a".repeat(100),
                "horizon-cloud-aaaaaaaaaaaaaaaaaaaaaaaaaaaa-2816597888e4a0d3a36b",
            ),
        ] {
            assert_eq!(DeviceName::derived(id, true).unwrap().name, expected);
        }
        assert_ne!(
            DeviceName::derived("Mixed_ID", true),
            DeviceName::derived("mixed-id", true)
        );
    }
