use super::*;
use crate::cloud_runtime::local_network::{endpoint::tests::prepared, session::Transport};
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

fn connection() -> Connection {
    Connection {
        host: "192.0.2.1".into(),
        port: 22,
        identity: "/private/key".into(),
        known_hosts: "/private/hosts".into(),
        host_key_alias: "worker-one".into(),
        proxy_command: None,
    }
}

#[test]
fn ordinary_tcp_gateway_cannot_become_a_private_ssh_attachment() {
    let gateway = Gateway::start(prepared("100.64.1.2:8123".parse().unwrap())).unwrap();
    assert!(gateway.proxy.private_path().is_none());
    assert!(matches!(gateway.attach(connection()), Err(Error::UnsupportedSource)));
}

#[test]
fn original_pinned_ssh_uses_private_target_and_unchanged_worker_command() {
    let gateway = Arc::new(Gateway::start_private(prepared("100.64.1.2:8123".parse().unwrap())).unwrap());
    let transport = PrivateSsh {
        connection: connection(),
        socket: gateway.proxy.private_path().unwrap().to_owned(),
        gateway,
    };
    let nonce = Nonce::random();
    let subnet = Subnet::containing("100.64.1.2".parse().unwrap(), 30).unwrap();
    let command = transport.hold(&nonce, subnet, 12345);
    let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(args.last().unwrap(), &nonce.hold_command(subnet));
    let forward = args.windows(2).find(|a| a[0] == "-R").unwrap();
    assert_eq!(
        forward[1],
        format!("{}:{}", nonce.bridge_socket(), transport.socket.display())
    );
    assert!(!forward[1].contains("127.0.0.1:"));
    assert!(args.iter().any(|a| a == "StrictHostKeyChecking=yes"));
    assert!(transport.bounded_input());
    assert!(transport.current());
    transport.gateway.revoke(&transport.gateway.status().receipt).unwrap();
    assert!(!transport.current());
}

struct SyntheticWorker;
impl Transport for SyntheticWorker {
    fn prepare(&self) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'horizon-local-network=1\n'"]).env_clear();
        command
    }
    fn hold(&self, _: &Nonce, _: Subnet, _: u16) -> Command {
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "printf '{\"proxy\":\"127.0.0.1:31001\"}\n'; while IFS= read -r line; do :; done",
            ])
            .env_clear();
        command
    }
    fn bounded_input(&self) -> bool {
        true
    }
}

pub(in crate::cloud_runtime::local_network::endpoint) fn synthetic_attachment(
    prepared: super::super::Prepared,
) -> Attached {
    let gateway = Arc::new(Gateway::start_private(prepared).unwrap());
    Attached::start(
        gateway,
        SyntheticWorker,
        Subnet::containing("100.64.1.2".parse().unwrap(), 30).unwrap(),
    )
    .unwrap()
}

#[test]
fn synthetic_worker_reports_existing_ready_and_exact_close_ends_source() {
    let attached = synthetic_attachment(prepared("100.64.1.2:8123".parse().unwrap()));
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !matches!(attached.status().worker, local_network::State::Active { .. }) {
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let receipt = attached.status().gateway.receipt;
    attached.close(&receipt).unwrap();
    assert!(matches!(attached.status().gateway.state, State::Revoked { .. }));
}

struct DiagnosticWorker {
    preparation_fails: bool,
}
impl Transport for DiagnosticWorker {
    fn prepare(&self) -> Command {
        let mut command = Command::new("sh");
        let script = if self.preparation_fails {
            "printf 'private-worker-note\\n'"
        } else {
            "printf 'horizon-local-network=1\\n'"
        };
        command.args(["-c", script]).env_clear();
        command
    }
    fn hold(&self, _: &Nonce, _: Subnet, _: u16) -> Command {
        assert!(!self.preparation_fails);
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "printf 'private-key-path private-known-hosts-path\\n' >&2; exit 1",
            ])
            .env_clear();
        command
    }
}

#[test]
fn attachment_status_does_not_publish_preparation_output_or_ssh_diagnostics() {
    for preparation_fails in [true, false] {
        let gateway = Arc::new(Gateway::start_private(prepared("100.64.1.2:8123".parse().unwrap())).unwrap());
        let mut attached = Attached::start(
            gateway,
            DiagnosticWorker { preparation_fails },
            Subnet::containing("100.64.1.2".parse().unwrap(), 30).unwrap(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let diagnostic = loop {
            let state = attached
                .shared
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let local_network::State::Reconnecting { error } = state {
                break error;
            }
            assert!(Instant::now() < deadline, "owned child must report its actual failure");
            thread::sleep(Duration::from_millis(10));
        };
        assert!(diagnostic.contains(if preparation_fails {
            "private-worker-note"
        } else {
            "private-key-path private-known-hosts-path"
        }));
        assert_eq!(
            attached.status().worker,
            local_network::State::Reconnecting {
                error: "The approved endpoint's worker connection is reconnecting".into(),
            }
        );
        attached.cancel.cancel();
        attached.supervisor.take().unwrap().join().unwrap();
        attached.shared.set(local_network::State::Failed { error: diagnostic });
        assert_eq!(
            attached.status().worker,
            local_network::State::Failed {
                error: "The approved endpoint's worker connection is unavailable".into(),
            }
        );
    }
}

struct PreparingWorker(PathBuf);
impl Transport for PreparingWorker {
    fn prepare(&self) -> Command {
        let mut command = Command::new("sh");
        command
            .args(["-c", "printf started > \"$1\"; exec sleep 10", "fixture"])
            .arg(&self.0)
            .env_clear();
        command
    }
    fn hold(&self, _: &Nonce, _: Subnet, _: u16) -> Command {
        panic!("a cancelled preparation must not start a worker hold")
    }
    fn bounded_input(&self) -> bool {
        true
    }
}

#[test]
fn pending_preparation_ignores_stale_close_and_exact_close_cancels_the_owned_child() {
    let directory = tempfile::tempdir().unwrap();
    let began = directory.path().join("prepared-child");
    let gateway = Arc::new(Gateway::start_private(prepared("100.64.1.2:8123".parse().unwrap())).unwrap());
    let attached = Attached::start(
        gateway,
        PreparingWorker(began.clone()),
        Subnet::containing("100.64.1.2".parse().unwrap(), 30).unwrap(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !began.exists() {
        assert!(Instant::now() < deadline, "actual preparation child must start");
        thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(attached.status().worker, local_network::State::Starting));
    let receipt = attached.status().gateway.receipt;
    let mut stale = receipt.clone();
    stale.generation = uuid::Uuid::new_v4();
    assert_eq!(attached.close(&stale), Err(Error::StaleReceipt));
    assert!(!attached.cancel.is_cancelled());
    assert_eq!(attached.status().gateway.state, State::Active);
    let before = Instant::now();
    attached.close(&receipt).unwrap();
    assert!(matches!(attached.status().gateway.state, State::Revoked { .. }));
    assert!(matches!(attached.status().worker, local_network::State::Failed { .. }));
    drop(attached);
    assert!(
        before.elapsed() < Duration::from_secs(2),
        "owned child must retire before its sleep ends"
    );
}

struct CountedWorker {
    starts: Arc<AtomicUsize>,
    original: PrivateSsh,
}
impl Transport for CountedWorker {
    fn prepare(&self) -> Command {
        SyntheticWorker.prepare()
    }
    fn hold(&self, nonce: &Nonce, subnet: Subnet, port: u16) -> Command {
        self.starts.fetch_add(1, Ordering::Relaxed);
        SyntheticWorker.hold(nonce, subnet, port)
    }
    fn current(&self) -> bool {
        self.original.current()
    }
    fn bounded_input(&self) -> bool {
        self.original.bounded_input()
    }
}

#[test]
fn source_revocation_retires_the_confirmed_supervisor_without_restarting_ssh() {
    let changed = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&changed);
    let address = "100.64.1.2:8123".parse().unwrap();
    let mut candidate = prepared(address);
    candidate.source = super::super::Source::fixture(move |_| {
        Ok(super::super::tests::binding(
            address,
            if seen.load(Ordering::Acquire) {
                "replacement"
            } else {
                "peer-fixture"
            },
        ))
    });
    let gateway = Arc::new(Gateway::start_private(candidate).unwrap());
    let starts = Arc::new(AtomicUsize::new(0));
    let attached = Attached::start(
        Arc::clone(&gateway),
        CountedWorker {
            starts: Arc::clone(&starts),
            original: PrivateSsh {
                connection: connection(),
                socket: gateway.proxy.private_path().unwrap().to_owned(),
                gateway,
            },
        },
        Subnet::containing("100.64.1.2".parse().unwrap(), 30).unwrap(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !matches!(attached.status().worker, local_network::State::Active { .. }) {
        assert!(Instant::now() < deadline, "synthetic worker must confirm Ready");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(starts.load(Ordering::Relaxed), 1);
    changed.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !attached.supervisor.as_ref().unwrap().is_finished() {
        assert!(
            Instant::now() < deadline,
            "source revocation must retire the supervisor"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(attached.status().gateway.state, State::Revoked { .. }));
    assert!(matches!(attached.status().worker, local_network::State::Failed { .. }));
    assert_eq!(starts.load(Ordering::Relaxed), 1, "revocation must not reconnect SSH");
}

#[cfg(target_os = "linux")]
mod real_ssh;
