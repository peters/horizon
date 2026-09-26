//! Inspect only initialized, empty allocations while retaining their lock.
use super::{
    recovery::{BOOTSTRAP, Bootstrap, Phase, ROOT, decode, read_request},
    runtime::Runtime,
    store::{LIMIT, Store, invalid},
};
use horizon_cloud::Capabilities;
use horizon_cloud_protocol::{
    bootstrap::RecoveryRequest,
    inspection::{Receipt, Request},
    signed::{Action, SignedIntent, Target},
};
use std::{
    fs::File,
    io::{self, Read, Seek, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn run() -> io::Result<()> {
    if std::env::args().len() != 2 {
        return Err(invalid());
    }
    let request = read_request(io::stdin().lock())?;
    let store = Store::open(Path::new(ROOT))?;
    let runtime = Runtime::captured()?;
    let receipt = inspect(&store, &runtime, &request, probe)?;
    serde_json::to_writer(io::stdout().lock(), &receipt)?;
    io::stdout().lock().write_all(b"\n")
}

fn inspect(
    store: &Store,
    runtime: &Runtime,
    request: &RecoveryRequest,
    probe: impl FnOnce(&Capabilities) -> io::Result<()>,
) -> io::Result<Receipt> {
    let encoded = store.read(BOOTSTRAP)?.ok_or_else(invalid)?;
    let bootstrap: Bootstrap = decode(&encoded)?;
    bootstrap.validate(store, runtime)?;
    if bootstrap.version != 2 || bootstrap.phase != Phase::Initialized {
        return Err(invalid());
    }
    bootstrap.empty(store)?;
    let signed = SignedIntent::parse(request.message.as_bytes()).map_err(|_| invalid())?;
    let intent = signed
        .verify(&bootstrap.startup.controller, request.payload.as_bytes())
        .map_err(|_| invalid())?;
    if intent.action() != Action::InspectAllocation
        || intent.expected_revision() != 0
        || *intent.target() != (Target::Allocation {})
    {
        return Err(invalid());
    }
    let selected: Request = decode(request.payload.as_bytes())?;
    probe(&selected.capabilities)?;
    bootstrap.validate(store, runtime)?;
    bootstrap.empty(store)?;
    if store.read(BOOTSTRAP)?.as_deref() != Some(&encoded) {
        return Err(invalid());
    }
    Ok(Receipt {
        version: 1,
        startup: bootstrap.startup,
        worker_id: bootstrap.worker_id,
        operation: intent.operation(),
        fingerprint: intent.fingerprint().map_err(|_| invalid())?,
        revision: 0,
        capabilities: selected.capabilities,
    })
}

pub(super) fn probe(capabilities: &Capabilities) -> io::Result<()> {
    probe_with_timeout(capabilities, Duration::from_secs(60))
}

pub(super) fn probe_with_timeout(capabilities: &Capabilities, timeout: Duration) -> io::Result<()> {
    // Version probes may create configuration. Keep it away from retained
    // project homes, credentials, global runtime files and the worker workspace.
    let home = tempfile::tempdir()?;
    let output = execute(
        Command::new("/usr/local/bin/horizon-worker-check")
            .args(["--capabilities-json", &serde_json::to_string(capabilities)?])
            .env_clear()
            .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("XDG_CACHE_HOME", home.path())
            .env("XDG_DATA_HOME", home.path())
            .current_dir(home.path()),
        timeout.min(Duration::from_secs(60)),
    )?;
    let output = std::str::from_utf8(&output).map_err(|_| invalid())?;
    for marker in [
        "horizon-worker-contract=1",
        "horizon-source-contract=1",
        "horizon-capabilities-contract=1",
        "horizon-session-restart-contract=1",
    ] {
        if !output.lines().any(|line| line == marker) {
            return Err(invalid());
        }
    }
    if capabilities.browserstack.is_some() && !output.lines().any(|line| line == "horizon-browserstack-contract=1") {
        return Err(invalid());
    }
    Ok(())
}

struct ProbeProcess(Child);
impl Drop for ProbeProcess {
    fn drop(&mut self) {
        if let Some(pid) = rustix::process::Pid::from_raw(self.0.id().cast_signed()) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.0.wait();
    }
}

fn execute(command: &mut Command, timeout: Duration) -> io::Result<Vec<u8>> {
    execute_input(command, timeout, Stdio::null())
}
pub(super) fn execute_leased(command: &mut Command, timeout: Duration, lease: File) -> io::Result<Vec<u8>> {
    execute_input(command, timeout, Stdio::from(lease))
}
fn execute_input(command: &mut Command, timeout: Duration, input: Stdio) -> io::Result<Vec<u8>> {
    let mut output = File::from(rustix::fs::memfd_create(
        "allocation-inspection",
        rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
    )?);
    // A fast or faulty checker cannot allocate unbounded output between polls.
    // The shared file offset records written bytes in this fixed-size buffer.
    output.set_len(LIMIT + 1)?;
    rustix::fs::fcntl_add_seals(&output, rustix::fs::SealFlags::GROW | rustix::fs::SealFlags::SHRINK)?;
    let mut child = ProbeProcess(
        command
            .process_group(0)
            .stdin(input)
            .stdout(output.try_clone()?)
            .stderr(Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + timeout;
    loop {
        if output.stream_position()? > LIMIT || Instant::now() >= deadline {
            return Err(invalid());
        }
        if let Some(status) = child.0.try_wait()? {
            if !status.success() {
                return Err(invalid());
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(child);
    let length = output.stream_position()?;
    if length > LIMIT {
        return Err(invalid());
    }
    output.rewind()?;
    let mut bytes = Vec::new();
    output.take(length).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::{keys, recovery::MANIFEST, runtime::Source};
    use horizon_cloud_protocol::{
        AllocationId, ControllerId, OperationId, SharingMode,
        bootstrap::{BootstrapOutcome, BootstrapReceipt, RecoveryReceipt, Startup},
        signed::{ControllerBinding, Intent},
    };
    use ring::{
        rand::SystemRandom,
        signature::{Ed25519KeyPair, KeyPair},
    };
    use std::{fs, os::unix::fs::PermissionsExt};

    struct Fixture {
        _directory: tempfile::TempDir,
        store: Store,
        runtime: Runtime,
        key: Ed25519KeyPair,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let workspace = directory.path().join("workspace");
            fs::create_dir(&workspace).unwrap();
            fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700)).unwrap();
            let key =
                Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap().as_ref())
                    .unwrap();
            let startup = Startup {
                version: 1,
                controller: ControllerBinding::new(
                    AllocationId::generate(),
                    ControllerId::generate(),
                    key.public_key().as_ref().try_into().unwrap(),
                ),
                token: OperationId::generate(),
                sharing: SharingMode::TrustedShared,
                worker_operation: "operation-one".into(),
                volume_id: "volume-one".into(),
                data_center_id: "center-one".into(),
            };
            let runtime = Runtime {
                version: 1,
                source: Source::StartupCapture,
                worker_id: "worker-one".into(),
                volume_id: startup.volume_id.clone(),
                data_center_id: startup.data_center_id.clone(),
                worker_operation: startup.worker_operation.clone(),
                startup,
            };
            let key_path = directory.path().join("ssh-key");
            assert!(
                Command::new("ssh-keygen")
                    .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                    .arg(&key_path)
                    .status()
                    .unwrap()
                    .success()
            );
            let host_key = fs::read(key_path).unwrap();
            let store = Store::create(&workspace.join(".horizon-allocation")).unwrap();
            store.create_host_key(&host_key).unwrap();
            let record = Bootstrap {
                version: 2,
                startup: runtime.startup.clone(),
                worker_id: runtime.worker_id.clone(),
                phase: Phase::Initialized,
                recovery: Some(RecoveryReceipt {
                    version: 1,
                    startup: runtime.startup.clone(),
                    worker_id: runtime.worker_id.clone(),
                    operation: OperationId::generate(),
                    fingerprint: [1; 32],
                }),
                initialization: Some(BootstrapReceipt {
                    version: 1,
                    startup: runtime.startup.clone(),
                    worker_id: runtime.worker_id.clone(),
                    operation: OperationId::generate(),
                    fingerprint: [2; 32],
                    outcome: BootstrapOutcome::Initializing,
                }),
                host_key: Some(keys::public(&host_key).unwrap()),
                key_hash: Some(keys::hash(&host_key)),
                abandonment: None,
            };
            store
                .write(BOOTSTRAP, None, &serde_json::to_vec(&record).unwrap())
                .unwrap();
            store.write(MANIFEST, None, &serde_json::to_vec(&serde_json::json!({"version":1,"startup":runtime.startup,"worker_id":runtime.worker_id,"revision":0,"members":[]})).unwrap()).unwrap();
            Self {
                _directory: directory,
                store,
                runtime,
                key,
            }
        }
        fn request(&self, action: Action, revision: u64) -> RecoveryRequest {
            let payload = serde_json::to_string(&Request {
                capabilities: serde_json::from_str("{}").unwrap(),
            })
            .unwrap();
            let binding = &self.runtime.startup.controller;
            let intent = Intent::new(
                binding,
                OperationId::generate(),
                revision,
                Target::Allocation {},
                action,
                payload.as_bytes(),
            )
            .unwrap();
            RecoveryRequest {
                message: serde_json::to_string(&SignedIntent::sign(intent, binding, &self.key).unwrap()).unwrap(),
                payload,
            }
        }
    }

    #[test]
    fn inspection_is_read_only_and_requires_current_signed_empty_state() {
        let f = Fixture::new();
        let original = f.store.read(BOOTSTRAP).unwrap().unwrap();
        let manifest = f.store.read(MANIFEST).unwrap().unwrap();
        let request = f.request(Action::InspectAllocation, 0);
        let receipt = inspect(&f.store, &f.runtime, &request, |_| Ok(())).unwrap();
        assert_eq!(receipt, inspect(&f.store, &f.runtime, &request, |_| Ok(())).unwrap());
        assert_eq!(receipt.revision, 0);
        assert_eq!(f.store.read(BOOTSTRAP).unwrap().unwrap(), original);
        assert_eq!(f.store.read(MANIFEST).unwrap().unwrap(), manifest);
        for variant in 0..8 {
            let mut request = f.request(Action::InspectAllocation, 0);
            let mut runtime = f.runtime.clone();
            let mut record: Bootstrap = decode(&original).unwrap();
            match variant {
                0 => request = f.request(Action::Bootstrap, 0),
                1 => request = f.request(Action::InspectAllocation, 1),
                2 => request.payload.push(' '),
                3 => request = Fixture::new().request(Action::InspectAllocation, 0),
                4 => record.phase = Phase::Initializing,
                5 => record.phase = Phase::Abandoned,
                6 => runtime.worker_id = "foreign-worker".into(),
                _ => record.version = 1,
            }
            f.store
                .write(BOOTSTRAP, Some(&original), &serde_json::to_vec(&record).unwrap())
                .unwrap();
            assert!(inspect(&f.store, &runtime, &request, |_| panic!("unauthorized probe")).is_err());
            let current = f.store.read(BOOTSTRAP).unwrap().unwrap();
            f.store.write(BOOTSTRAP, Some(&current), &original).unwrap();
        }
        assert!(inspect(&f.store, &f.runtime, &request, |_| Err(invalid())).is_err());
        assert!(
            inspect(&f.store, &f.runtime, &request, |_| {
                let mut value: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
                value["members"] = serde_json::json!([{"project":"preserve"}]);
                f.store
                    .write(MANIFEST, Some(&manifest), &serde_json::to_vec(&value).unwrap())
            })
            .is_err()
        );
        assert!(inspect(&f.store, &f.runtime, &request, |_| panic!("nonempty allocation")).is_err());
    }

    #[test]
    fn ownership_changes_during_probe_invalidate_the_observation() {
        for name in [BOOTSTRAP, "ssh-host-key"] {
            let f = Fixture::new();
            let request = f.request(Action::InspectAllocation, 0);
            let original = f.store.read(name).unwrap().unwrap();
            assert!(
                inspect(&f.store, &f.runtime, &request, |_| {
                    f.store.write(name, Some(&original), b"changed during probe")
                })
                .is_err()
            );
        }
    }

    #[test]
    fn checker_descendants_stop_on_timeout_and_success() {
        for wait in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("child-pid");
            let script = if wait {
                "sleep 30 & child=$!; printf '%s' \"$child\" > \"$1\"; wait"
            } else {
                "sleep 30 & child=$!; printf '%s' \"$child\" > \"$1\""
            };
            let result = execute(
                Command::new("/bin/sh").args(["-c", script, "probe"]).arg(&path),
                Duration::from_millis(250),
            );
            assert_eq!(result.is_err(), wait);
            let pid: u32 = fs::read_to_string(path).unwrap().parse().unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let state = fs::read_to_string(format!("/proc/{pid}/stat"));
                if match state {
                    Err(_) => true,
                    Ok(value) => value
                        .rsplit_once(") ")
                        .is_some_and(|(_, fields)| fields.starts_with('Z')),
                } {
                    break;
                }
                assert!(Instant::now() < deadline, "checker child remains running");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn checker_execution_bounds_time_output_and_failure() {
        assert_eq!(
            execute(
                Command::new("/bin/sh").args(["-c", "printf verified"]),
                Duration::from_secs(2)
            )
            .unwrap(),
            b"verified"
        );
        assert!(execute(Command::new("/bin/sh").args(["-c", "exit 1"]), Duration::from_secs(2)).is_err());
        let start = Instant::now();
        assert!(
            execute(
                Command::new("/bin/sh").args(["-c", "sleep 30 & wait"]),
                Duration::from_millis(50)
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(execute(Command::new("/bin/sh").args(["-c", "yes"]), Duration::from_secs(2)).is_err());
    }
}
