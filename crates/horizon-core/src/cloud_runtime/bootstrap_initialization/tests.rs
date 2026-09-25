mod membership;
use super::*;
use crate::cloud_runtime::owner::tests::{create as create_owner, fixture, open};
use horizon_cloud::{Bootstrap, Capabilities, Profile, Storage};
use std::{fs, os::unix::fs::PermissionsExt as _};

fn request(root: &std::path::Path) -> Request {
    let key = root.join("identity");
    assert!(
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .status()
            .unwrap()
            .success()
    );
    let credential = tempfile::NamedTempFile::new_in(root).unwrap();
    fs::write(credential.path(), "synthetic-account").unwrap();
    let credential = credential.keep().unwrap().1;
    Request {
        worker: WorkerSpec {
            operation_id: "first-test".into(),
            image_digest: format!("test/worker@sha256:{}", "a".repeat(64)),
            profile: Profile {
                cpu: 2,
                memory_gb: 4,
                gpu: false,
                storage: Storage {
                    container_gb: 10,
                    volume_gb: 10,
                },
                provider: "runpod".into(),
                image: "test/worker".into(),
                build: None,
                bootstrap: Bootstrap::default(),
                capabilities: Capabilities::default(),
            },
            public_key: fs::read_to_string(key.with_extension("pub")).unwrap().trim().to_owned(),
            registry_auth_id: None,
            gpu_types: Vec::new(),
            cpu_flavors: vec!["cpu3c".into()],
            data_centers: vec!["EU-NL-1".into()],
            startup_metadata: None,
        },
        sharing: SharingMode::TrustedShared,
        credential_file: credential,
        identity_file: key,
        known_hosts: root.join("known-hosts"),
    }
}

#[test]
fn retained_creation_never_repeats_provider_io_and_changed_bindings_are_rejected() {
    let (temp, root, vault) = fixture();
    let mut owner = create_owner(&root, &vault);
    let request = request(temp.path());
    let cancel = Cancellation::default();
    let (account, _, identity) = bindings(&request, &runner(&cancel)).unwrap();
    let record = Record {
        version: 1,
        request: request.clone(),
        account,
        identity,
        spec: request.worker.clone(),
        volume_spec: volumes::Spec {
            operation_id: request.worker.operation_id.clone(),
            size: 10,
            data_center_id: "EU-NL-1".into(),
        },
        volume: volumes::State::Requested,
        worker: CreateState::Prepared,
        startup: None,
        phase: Phase::Creating,
        requested: false,
        cleanup_receipt: None,
        initialize: None,
        abandon: None,
    };
    record.save(&mut owner).unwrap();
    drop(owner);
    let mut owner = open(&root, &vault).unwrap();
    assert!(matches!(
        create(&mut owner, &request, &cancel, Duration::from_secs(1)),
        Err(Error::Unresolved)
    ));
    assert!(matches!(
        resume(&mut owner, &request, &cancel, Duration::from_secs(1)),
        Err(Error::Unresolved)
    ));
    fs::write(&request.credential_file, "changed-account").unwrap();
    assert!(matches!(
        cleanup(&mut owner, &request, &cancel, Duration::from_secs(1)),
        Err(Error::Invalid)
    ));
    assert_eq!(Record::load(&owner).unwrap().unwrap().volume, volumes::State::Requested);
}

fn native_fixture(directory: &std::path::Path) -> CoordinatorFixture {
    let (temp, root, vault) = fixture();
    let mut owner = create_owner(&root, &vault);
    let mut request = request(temp.path());
    request.identity_file = directory.join("id_ed25519");
    fs::read_to_string(directory.join("id_ed25519.pub"))
        .unwrap()
        .trim()
        .clone_into(&mut request.worker.public_key);
    request.known_hosts = directory.join("known_hosts");
    let cancel = Cancellation::default();
    let runner = runner(&cancel);
    let (account, _, identity) = bindings(&request, &runner).unwrap();
    let startup = Startup {
        version: 1,
        controller: owner.binding().unwrap(),
        token: OperationId::generate(),
        sharing: request.sharing,
        worker_operation: request.worker.operation_id.clone(),
        volume_id: "volume-one".into(),
        data_center_id: "EU-NL-1".into(),
    };
    let volume_spec = volumes::Spec {
        operation_id: request.worker.operation_id.clone(),
        size: 10,
        data_center_id: startup.data_center_id.clone(),
    };
    let volume = volumes::Volume {
        id: startup.volume_id.clone(),
        name: volume_spec.name(),
        size: 10,
        data_center_id: startup.data_center_id.clone(),
    };
    let mut record = Record {
        version: 1,
        request: request.clone(),
        account,
        identity,
        spec: request.worker.clone(),
        volume_spec,
        volume: volumes::State::Bound { volume, creation: None },
        worker: CreateState::Bound {
            worker_id: "worker-one".into(),
        },
        startup: Some(startup.clone()),
        phase: Phase::Creating,
        requested: false,
        cleanup_receipt: None,
        initialize: None,
        abandon: None,
    };
    record.spec.startup_metadata =
        Some(horizon_cloud::StartupMetadata::new(serde_json::to_string(&startup).unwrap()).unwrap());
    record.save(&mut owner).unwrap();
    fs::write(directory.join("runtime.json"), serde_json::to_vec(&serde_json::json!({
        "HORIZON_WORKER_STARTUP":serde_json::to_string(&startup).unwrap(), "HORIZON_CLOUD_OPERATION": startup.worker_operation,
        "RUNPOD_POD_ID":"worker-one", "RUNPOD_VOLUME_ID":startup.volume_id, "RUNPOD_DC_ID":startup.data_center_id
    })).unwrap()).unwrap();
    CoordinatorFixture {
        directory: temp,
        root,
        vault,
        owner,
        record,
    }
}

fn startup_deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

fn native_inspection(
    owner: &Owner,
    target: &bootstrap_recovery::Target,
    runner: &Runner<'_>,
    directory: &std::path::Path,
) {
    let capabilities: Capabilities = serde_json::from_str("{}").unwrap();
    let observed = inspection::inspect_with(
        owner,
        target,
        &capabilities,
        startup_deadline(),
        &mut |connection, bytes, timeout| {
            Ok(runner.private_exchange(
                &mut connection.pinned_command("horizon-cloud-worker inspect-allocation"),
                bytes,
                timeout,
            )?)
        },
    )
    .unwrap();
    assert_eq!(observed.capabilities, capabilities);
    assert!(!directory.join("workspace/capabilities.json").exists());
    let unavailable: Capabilities = serde_json::from_str(r#"{"desktop":true}"#).unwrap();
    assert!(
        inspection::inspect_with(
            owner,
            target,
            &unavailable,
            startup_deadline(),
            &mut |connection, bytes, timeout| Ok(runner.private_exchange(
                &mut connection.pinned_command("horizon-cloud-worker inspect-allocation"),
                bytes,
                timeout,
            )?),
        )
        .is_err()
    );
}

#[test]
#[ignore = "requires scripts/cloud-initialization-smoke.py and an isolated mount namespace"]
fn native_ssh_worker_initialization() {
    let directory = PathBuf::from(std::env::var_os("HORIZON_INITIALIZATION_FIXTURE").unwrap());
    let port = u16::try_from(serde_json::from_slice::<serde_json::Value>(&fs::read(directory.join("fixture.json")).unwrap()).unwrap()["port"].as_u64().unwrap()).unwrap();
    let CoordinatorFixture {
        directory: _temporary,
        root,
        vault,
        mut owner,
        mut record,
    } = native_fixture(&directory);
    let cancel = Cancellation::default();
    let runner = runner(&cancel);
    let target = target(&record, ([127, 0, 0, 1], port).into()).unwrap();
    let deadline = startup_deadline();
    enroll(&record, &target, &runner, deadline).unwrap();
    initialize(&mut owner, &mut record, &target, &runner, deadline).unwrap();
    assert!(!directory.join("workspace/.horizon-allocation/membership.json").exists());
    // Lose the host handle after Requested: a fresh handle sends only Recover.
    drop(owner);
    let mut owner = open(&root, &vault).unwrap();
    let mut record = Record::load(&owner).unwrap().unwrap();
    vault.fail_after(Some(2));
    assert!(complete(&mut owner, &mut record, &target, &cancel, startup_deadline()).is_err());
    drop(owner);
    vault.fail_after(None);
    let mut owner = open(&root, &vault).unwrap();
    let mut record = Record::load(&owner).unwrap().unwrap();
    let receipt = complete(&mut owner, &mut record, &target, &cancel, startup_deadline()).unwrap();
    let pin = fs::read(&target.connection.known_hosts).unwrap();
    fs::write(directory.join("restart"), b"restart only fixture runtime").unwrap();
    assert_eq!(
        complete(&mut owner, &mut record, &target, &cancel, startup_deadline()).unwrap(),
        receipt
    );
    assert_eq!(fs::read(&target.connection.known_hosts).unwrap(), pin);
    native_inspection(&owner, &target, &runner, &directory);
    let payload = BootstrapPayload::Abandon {
        startup: target.startup.clone(),
        worker_id: target.worker_id.clone(),
    };
    record.abandon = Some(
        Signed::new(
            &owner,
            &target.startup,
            &target.worker_id,
            &payload,
            BootstrapOutcome::Abandoned,
        )
        .unwrap(),
    );
    record.phase = Phase::AbandonRequested;
    record.save(&mut owner).unwrap();
    let signed = record.abandon.as_ref().unwrap();
    let snapshot = Snapshot::capture(&target).unwrap();
    let request_bytes = signed.request(&target, &payload, BootstrapOutcome::Abandoned).unwrap();
    let response = runner
        .private_exchange(
            &mut snapshot
                .connection
                .pinned_command("horizon-cloud-worker abandon-bootstrap"),
            request_bytes,
            Duration::from_secs(30),
        )
        .unwrap();
    // Simulate loss of the fence reply followed by a cold runtime restart.
    fs::write(directory.join("restart"), b"restart after abandonment").unwrap();
    let repeated = runner
        .private_exchange(
            &mut snapshot
                .connection
                .pinned_command("horizon-cloud-worker abandon-bootstrap"),
            request_bytes,
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(response, repeated);
    signed.confirm(&repeated).unwrap();
    assert!(bootstrap_recovery::recover(&mut owner, &target, &cancel, Duration::from_secs(30)).is_err());
    let initial = record.initialize.as_ref().unwrap();
    // Exact delayed initialization bytes cannot revive the terminal marker.
    let request_bytes = serde_json::to_value(initial).unwrap()["request"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        runner
            .private_exchange(
                &mut snapshot
                    .connection
                    .pinned_command("horizon-cloud-worker initialize-allocation"),
                request_bytes.as_bytes(),
                Duration::from_secs(30)
            )
            .is_err()
    );
}

#[test]
fn pre_startup_cleanup_intent_and_completion_reopen_without_new_creation() {
    for state in [
        volumes::State::Requested,
        volumes::State::Deleting {
            volume: volumes::Volume {
                id: "volume-one".into(),
                name: "horizon-volume-first-test".into(),
                size: 10,
                data_center_id: "EU-NL-1".into(),
            },
            creation: None,
        },
        volumes::State::Deleted,
    ] {
        let (temp, root, vault) = fixture();
        let mut owner = create_owner(&root, &vault);
        let request = request(temp.path());
        let cancel = Cancellation::default();
        let (account, _, identity) = bindings(&request, &runner(&cancel)).unwrap();
        let record = Record {
            version: 1,
            request: request.clone(),
            account,
            identity,
            spec: request.worker.clone(),
            volume_spec: volumes::Spec {
                operation_id: request.worker.operation_id.clone(),
                size: 10,
                data_center_id: "EU-NL-1".into(),
            },
            volume: state,
            worker: CreateState::Prepared,
            startup: None,
            phase: Phase::DeleteConfirmed,
            requested: false,
            cleanup_receipt: None,
            initialize: None,
            abandon: None,
        };
        record.save(&mut owner).unwrap();
        drop(owner);
        let mut owner = open(&root, &vault).unwrap();
        let (account, _, identity) = bindings(&request, &runner(&cancel)).unwrap();
        let mut record = Record::load(&owner).unwrap().unwrap();
        record.verify(&owner, &request, &account, &identity).unwrap();
        assert!(matches!(
            create(&mut owner, &request, &cancel, Duration::from_secs(1)),
            Err(Error::Unresolved)
        ));
        if record.volume == volumes::State::Deleted {
            record.phase = Phase::Deleted;
            record.save(&mut owner).unwrap();
            cleanup(&mut owner, &request, &cancel, Duration::from_secs(1)).unwrap();
        }
    }
}

struct CoordinatorFixture {
    directory: tempfile::TempDir,
    root: PathBuf,
    vault: crate::cloud_runtime::owner::tests::MemoryVault,
    owner: Owner,
    record: Record,
}
impl CoordinatorFixture {
    fn new() -> Self {
        let (directory, root, vault) = fixture();
        let mut owner = create_owner(&root, &vault);
        let request = request(directory.path());
        let cancel = Cancellation::default();
        let (account, _, identity) = bindings(&request, &runner(&cancel)).unwrap();
        let startup = Startup {
            version: 1,
            controller: owner.binding().unwrap(),
            token: OperationId::generate(),
            sharing: request.sharing,
            worker_operation: request.worker.operation_id.clone(),
            volume_id: "volume-one".into(),
            data_center_id: "EU-NL-1".into(),
        };
        let volume_spec = volumes::Spec {
            operation_id: request.worker.operation_id.clone(),
            size: 10,
            data_center_id: startup.data_center_id.clone(),
        };
        let volume = volumes::Volume {
            id: startup.volume_id.clone(),
            name: volume_spec.name(),
            size: 10,
            data_center_id: startup.data_center_id.clone(),
        };
        let host_key = request
            .worker
            .public_key
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        fs::write(&request.known_hosts, format!("horizon-cloud-worker-one {host_key}\n")).unwrap();
        fs::set_permissions(&request.known_hosts, fs::Permissions::from_mode(0o600)).unwrap();
        let mut record = Record {
            version: 1,
            request: request.clone(),
            account,
            identity,
            spec: request.worker.clone(),
            volume_spec,
            volume: volumes::State::Bound { volume, creation: None },
            worker: CreateState::Bound {
                worker_id: "worker-one".into(),
            },
            startup: Some(startup.clone()),
            phase: Phase::Requested,
            requested: true,
            cleanup_receipt: None,
            initialize: Some(
                Signed::new(
                    &owner,
                    &startup,
                    "worker-one",
                    &BootstrapPayload::Initialize {
                        startup: startup.clone(),
                        worker_id: "worker-one".into(),
                        host_key,
                    },
                    BootstrapOutcome::Initializing,
                )
                .unwrap(),
            ),
            abandon: None,
        };
        record.spec.startup_metadata =
            Some(horizon_cloud::StartupMetadata::new(serde_json::to_string(&startup).unwrap()).unwrap());
        record.save(&mut owner).unwrap();
        bootstrap_recovery::anchor(&mut owner, &target(&record, ([127, 0, 0, 1], 22).into()).unwrap()).unwrap();
        Self {
            directory,
            root,
            vault,
            owner,
            record,
        }
    }
    fn reopen(self) -> Self {
        let Self {
            directory,
            root,
            vault,
            owner,
            ..
        } = self;
        drop(owner);
        vault.fail_after(None);
        let owner = open(&root, &vault).unwrap();
        let record = Record::load(&owner).unwrap().unwrap();
        Self {
            directory,
            root,
            vault,
            owner,
            record,
        }
    }
}
fn resolve(record: &Record) -> Result<bootstrap_recovery::Target> {
    target(record, ([127, 0, 0, 1], 22).into())
}
fn fence_reply(bytes: &[u8]) -> Vec<u8> {
    use horizon_cloud_protocol::{
        bootstrap::{BootstrapReceipt, RecoveryRequest},
        signed::SignedIntent,
    };
    let request: RecoveryRequest = serde_json::from_slice(bytes).unwrap();
    let BootstrapPayload::Abandon { startup, worker_id } = serde_json::from_str(&request.payload).unwrap() else {
        panic!("unexpected command")
    };
    let signed = SignedIntent::parse(request.message.as_bytes()).unwrap();
    let intent = signed.verify(&startup.controller, request.payload.as_bytes()).unwrap();
    serde_json::to_vec(&BootstrapReceipt {
        version: 1,
        startup,
        worker_id,
        operation: intent.operation(),
        fingerprint: intent.fingerprint().unwrap(),
        outcome: BootstrapOutcome::Abandoned,
    })
    .unwrap()
}

#[test]
fn cleanup_lost_fence_reply_reuses_request_and_confirmed_delete_needs_no_worker() {
    let mut f = CoordinatorFixture::new();
    let mut request = Vec::new();
    assert!(
        cleanup_with(
            &mut f.owner,
            &mut f.record,
            &mut resolve,
            &mut |_, bytes| {
                request = bytes.to_vec();
                Err(Error::Unresolved)
            },
            &mut |_, _| panic!("delete before fence")
        )
        .is_err()
    );
    f = f.reopen();
    assert!(
        cleanup_with(
            &mut f.owner,
            &mut f.record,
            &mut resolve,
            &mut |_, bytes| {
                assert_eq!(bytes, request);
                Ok(fence_reply(bytes))
            },
            &mut |owner, record| {
                assert!(Record::load(owner).unwrap().unwrap().phase == Phase::DeleteConfirmed);
                assert!(record.cleanup_receipt.is_some());
                Err(Error::Unresolved)
            }
        )
        .is_err()
    );
    f = f.reopen();
    cleanup_with(
        &mut f.owner,
        &mut f.record,
        &mut |_| panic!("worker already deleted"),
        &mut |_, _| panic!("no more SSH"),
        &mut |_, _| Ok(()),
    )
    .unwrap();
    assert!(Record::load(&f.owner).unwrap().unwrap().phase == Phase::Deleted);
}

#[test]
fn cleanup_failed_intent_and_receipt_saves_never_delete() {
    for fail_after_receipt in [false, true] {
        let mut f = CoordinatorFixture::new();
        if !fail_after_receipt {
            f.vault.fail_after(Some(0));
        }
        assert!(
            cleanup_with(
                &mut f.owner,
                &mut f.record,
                &mut resolve,
                &mut |_, bytes| {
                    assert!(fail_after_receipt);
                    f.vault.fail_after(Some(0));
                    Ok(fence_reply(bytes))
                },
                &mut |_, _| panic!("unanchored deletion")
            )
            .is_err()
        );
        f = f.reopen();
        assert!(f.record.phase != Phase::DeleteConfirmed);
        cleanup_with(
            &mut f.owner,
            &mut f.record,
            &mut resolve,
            &mut |_, bytes| Ok(fence_reply(bytes)),
            &mut |_, _| Ok(()),
        )
        .unwrap();
    }
}

#[test]
fn recovery_only_owner_cannot_create_and_missing_recovery_cannot_mint_a_new_operation() {
    let mut f = CoordinatorFixture::new();
    let request = f.record.request.clone();
    let cancel = Cancellation::default();
    let mut payload = f.owner.load().unwrap();
    let saved = payload
        .as_object_mut()
        .unwrap()
        .remove("bootstrap_initialization")
        .unwrap();
    f.owner.save(payload).unwrap();
    assert!(matches!(
        create(&mut f.owner, &request, &cancel, Duration::from_secs(1)),
        Err(Error::Unresolved)
    ));
    let mut payload = f.owner.load().unwrap();
    payload.as_object_mut().unwrap().remove("bootstrap_recovery");
    payload
        .as_object_mut()
        .unwrap()
        .insert("bootstrap_initialization".into(), saved);
    f.owner.save(payload).unwrap();
    assert!(matches!(
        resume(&mut f.owner, &request, &cancel, Duration::from_secs(1)),
        Err(Error::Recovery(_))
    ));
    assert!(f.owner.load().unwrap().get("bootstrap_recovery").is_none());
}

#[test]
fn failed_prepared_and_requested_anchors_prevent_initialize_transport() {
    for writes in [0, 2] {
        let mut f = CoordinatorFixture::new();
        f.record.phase = Phase::Creating;
        f.record.requested = false;
        f.record.initialize = None;
        f.record.save(&mut f.owner).unwrap();
        f.vault.fail_after(Some(writes));
        let target = resolve(&f.record).unwrap();
        assert!(
            initialize_with(
                &mut f.owner,
                &mut f.record,
                &target,
                &mut || Ok(Duration::from_secs(30)),
                &mut |_, _, _| panic!("unanchored Initialize")
            )
            .is_err()
        );
        f = f.reopen();
        assert!(!f.record.requested);
    }
}

#[test]
fn expired_startup_budget_never_sends_initialize_or_discards_consumed_permission() {
    assert!(remaining(Instant::now()).is_err());
    for allowed in 0..3 {
        let mut f = CoordinatorFixture::new();
        f.record.phase = Phase::Creating;
        f.record.requested = false;
        f.record.initialize = None;
        f.record.save(&mut f.owner).unwrap();
        let target = resolve(&f.record).unwrap();
        let mut checks = 0;
        assert!(
            initialize_with(
                &mut f.owner,
                &mut f.record,
                &target,
                &mut || {
                    checks += 1;
                    if checks > allowed {
                        Err(Error::Unresolved)
                    } else {
                        Ok(Duration::from_secs(1))
                    }
                },
                &mut |_, _, _| panic!("expired Initialize")
            )
            .is_err()
        );
        f = f.reopen();
        assert_eq!(f.record.requested, allowed == 2);
        if allowed == 2 {
            assert!(
                complete(
                    &mut f.owner,
                    &mut f.record,
                    &target,
                    &Cancellation::default(),
                    Instant::now()
                )
                .is_err()
            );
            assert!(f.record.requested);
        }
    }
}

#[test]
fn enrollment_pin_substitution_and_growth_fail_promptly() {
    for kind in ["symlink", "fifo", "oversized"] {
        let root = tempfile::tempdir().unwrap();
        let file = tempfile::NamedTempFile::new_in(root.path()).unwrap();
        assert!(pin_bytes(file.path(), file.as_file()).unwrap().is_empty());
        match kind {
            "symlink" => {
                fs::remove_file(file.path()).unwrap();
                std::os::unix::fs::symlink("/dev/zero", file.path()).unwrap();
            }
            "fifo" => {
                fs::remove_file(file.path()).unwrap();
                assert!(
                    Command::new("mkfifo")
                        .args(["-m", "600"])
                        .arg(file.path())
                        .status()
                        .unwrap()
                        .success()
                );
            }
            _ => file.as_file().set_len(65 * 1024).unwrap(),
        }
        let started = Instant::now();
        assert!(pin_bytes(file.path(), file.as_file()).is_err());
        assert!(pin_directory(file.path()).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}

#[test]
fn replaced_pin_parent_cannot_satisfy_directory_durability() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("parent");
    fs::create_dir(&path).unwrap();
    let retained = pin_directory(&path).unwrap();
    verify_pin_parent(&path, &retained).unwrap();
    fs::rename(&path, root.path().join("old-parent")).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(verify_pin_parent(&path, &retained).is_err());
}

#[test]
fn corrupted_private_seed_never_anchors_and_corrected_key_can_retry() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    fn skip(bytes: &[u8], offset: &mut usize) -> usize {
        let length = usize::try_from(u32::from_be_bytes(bytes[*offset..*offset + 4].try_into().unwrap())).unwrap();
        let start = *offset + 4;
        *offset = start + length;
        start
    }
    if std::env::var_os("HORIZON_IDENTITY_AGENT_FIXTURE").is_none() {
        struct Agent(std::process::Child);
        impl Drop for Agent {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("agent.sock");
        let _agent = Agent(
            Command::new("ssh-agent")
                .arg("-D")
                .arg("-a")
                .arg(&socket)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(socket.exists());
        assert!(Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "cloud_runtime::bootstrap_initialization::tests::corrupted_private_seed_never_anchors_and_corrected_key_can_retry", "--nocapture"])
            .env("SSH_AUTH_SOCK", &socket).env("HORIZON_IDENTITY_AGENT_FIXTURE", "1")
            .status().unwrap().success());
        return;
    }
    let mut f = CoordinatorFixture::new();
    let target = resolve(&f.record).unwrap();
    assert!(
        Command::new("ssh-add")
            .arg(&target.connection.identity)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    let mut payload = f.owner.load().unwrap();
    payload.as_object_mut().unwrap().remove("bootstrap_recovery");
    f.owner.save(payload).unwrap();
    let original = fs::read(&target.connection.identity).unwrap();
    let body = std::str::from_utf8(&original)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("---"))
        .collect::<String>();
    let mut bytes = STANDARD.decode(body).unwrap();
    let mut offset = b"openssh-key-v1\0".len();
    for _ in 0..3 {
        skip(&bytes, &mut offset);
    }
    offset += 4;
    skip(&bytes, &mut offset);
    let private = skip(&bytes, &mut offset);
    let mut offset = private + 8;
    skip(&bytes, &mut offset);
    skip(&bytes, &mut offset);
    let seed = skip(&bytes, &mut offset);
    bytes[seed] ^= 1;
    fs::write(
        &target.connection.identity,
        format!(
            "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
            STANDARD.encode(bytes)
        ),
    )
    .unwrap();
    let parsed = Command::new("ssh-keygen")
        .args(["-y", "-P", "", "-f"])
        .arg(&target.connection.identity)
        .output()
        .unwrap();
    assert!(
        parsed.status.success(),
        "public extraction alone misses this corruption"
    );
    assert!(bootstrap_recovery::anchor(&mut f.owner, &target).is_err());
    assert!(f.owner.load().unwrap().get("bootstrap_recovery").is_none());
    fs::write(&target.connection.identity, original).unwrap();
    bootstrap_recovery::anchor(&mut f.owner, &target).unwrap();
}

#[test]
fn inspection_rejects_wrong_receipts_and_expired_or_changed_bindings() {
    use horizon_cloud_protocol::{
        bootstrap::RecoveryRequest,
        inspection::Receipt,
        signed::{Action, SignedIntent},
    };
    let f = CoordinatorFixture::new();
    let target = resolve(&f.record).unwrap();
    let capabilities: Capabilities = serde_json::from_str("{}").unwrap();
    let original = serde_json::to_vec(&Record::load(&f.owner).unwrap()).unwrap();
    let marker = fs::read(f.root.join("owner.json")).unwrap();
    for variant in 0..9 {
        let result = inspection::inspect_with(
            &f.owner,
            &target,
            &capabilities,
            startup_deadline(),
            &mut |_, bytes, _| {
                let request: RecoveryRequest = serde_json::from_slice(bytes).unwrap();
                let signed = SignedIntent::parse(request.message.as_bytes()).unwrap();
                let intent = signed
                    .verify(&target.startup.controller, request.payload.as_bytes())
                    .unwrap();
                assert_eq!(intent.action(), Action::InspectAllocation);
                let mut receipt = Receipt {
                    version: 1,
                    startup: target.startup.clone(),
                    worker_id: target.worker_id.clone(),
                    operation: intent.operation(),
                    fingerprint: intent.fingerprint().unwrap(),
                    revision: 0,
                    capabilities: capabilities.clone(),
                };
                match variant {
                    0 => {}
                    1 => receipt.version = 2,
                    2 => receipt.startup.token = OperationId::generate(),
                    3 => receipt.worker_id = "foreign-worker".into(),
                    4 => receipt.operation = OperationId::generate(),
                    5 => receipt.fingerprint = [0; 32],
                    6 => receipt.revision = 1,
                    7 => receipt.capabilities.desktop = true,
                    _ => fs::write(f.root.join("owner.json"), b"changed during exchange").unwrap(),
                }
                Ok(serde_json::to_vec(&receipt).unwrap())
            },
        );
        assert_eq!(result.is_ok(), variant == 0);
        fs::write(f.root.join("owner.json"), &marker).unwrap();
    }
    assert_eq!(serde_json::to_vec(&Record::load(&f.owner).unwrap()).unwrap(), original);
    assert!(
        inspection::inspect_with(&f.owner, &target, &capabilities, Instant::now(), &mut |_, _, _| panic!(
            "expired send"
        ))
        .is_err()
    );
    fs::write(&target.connection.known_hosts, b"changed pin").unwrap();
    assert!(
        inspection::inspect_with(
            &f.owner,
            &target,
            &capabilities,
            startup_deadline(),
            &mut |_, _, _| panic!("changed pins")
        )
        .is_err()
    );
}

#[test]
fn inspection_checks_completed_state_and_qualified_image_before_provider_reads() {
    let mut f = CoordinatorFixture::new();
    let cancel = Cancellation::default();
    let capabilities: Capabilities = serde_json::from_str("{}").unwrap();
    for completed in [false, true] {
        if completed {
            f.record.phase = Phase::Completed;
            f.record.save(&mut f.owner).unwrap();
        }
        assert!(matches!(
            inspect(
                &f.owner,
                &f.record.request,
                "different-image",
                &capabilities,
                &cancel,
                Duration::from_secs(5)
            ),
            Err(Error::Invalid)
        ));
    }
    assert!(matches!(
        inspect(
            &f.owner,
            &f.record.request,
            &f.record.spec.image_digest,
            &capabilities,
            &cancel,
            Duration::ZERO
        ),
        Err(Error::Unresolved)
    ));
}
