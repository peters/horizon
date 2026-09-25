mod membership;
use super::super::recovery::{MANIFEST, recover};
use super::*;
use horizon_cloud_protocol::{
    AllocationId, ControllerId, OperationId, SharingMode,
    bootstrap::{RecoveryPayload, Startup},
    signed::{ControllerBinding, Intent},
};
use ring::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

struct Fixture {
    directory: tempfile::TempDir,
    runtime: Runtime,
    controller: Ed25519KeyPair,
    key: zeroize::Zeroizing<Vec<u8>>,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(directory.path().join("workspace")).unwrap();
        let controller =
            Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap().as_ref()).unwrap();
        assert!(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(directory.path().join(keys::HOST_KEY))
                .status()
                .unwrap()
                .success()
        );
        let key = keys::current(directory.path()).unwrap();
        let runtime = Runtime {
            version: 1,
            source: Source::StartupCapture,
            startup: Startup {
                version: 1,
                controller: ControllerBinding::new(
                    AllocationId::generate(),
                    ControllerId::generate(),
                    controller.public_key().as_ref().try_into().unwrap(),
                ),
                token: OperationId::generate(),
                sharing: SharingMode::TrustedShared,
                worker_operation: "create-one".into(),
                volume_id: "volume-one".into(),
                data_center_id: "center-one".into(),
            },
            worker_id: "worker-one".into(),
            volume_id: "volume-one".into(),
            data_center_id: "center-one".into(),
            worker_operation: "create-one".into(),
        };
        Self {
            directory,
            runtime,
            controller,
            key,
        }
    }
    fn root(&self) -> std::path::PathBuf {
        self.directory.path().join("workspace/.horizon-allocation")
    }
    fn signed(&self, payload: impl serde::Serialize) -> RecoveryRequest {
        let payload = serde_json::to_string(&payload).unwrap();
        let binding = &self.runtime.startup.controller;
        let intent = Intent::new(
            binding,
            OperationId::generate(),
            0,
            Target::Allocation {},
            Action::Bootstrap,
            payload.as_bytes(),
        )
        .unwrap();
        RecoveryRequest {
            message: serde_json::to_string(&SignedIntent::sign(intent, binding, &self.controller).unwrap()).unwrap(),
            payload,
        }
    }
    fn init_request(&self) -> RecoveryRequest {
        self.signed(BootstrapPayload::Initialize {
            startup: self.runtime.startup.clone(),
            worker_id: self.runtime.worker_id.clone(),
            host_key: keys::public(&self.key).unwrap(),
        })
    }
    fn recover_request(&self) -> RecoveryRequest {
        self.signed(RecoveryPayload::Recover {
            token: self.runtime.startup.token,
        })
    }
    fn abandon_request(&self) -> RecoveryRequest {
        self.signed(BootstrapPayload::Abandon {
            startup: self.runtime.startup.clone(),
            worker_id: self.runtime.worker_id.clone(),
        })
    }
    fn init(&self, request: &RecoveryRequest) -> io::Result<BootstrapReceipt> {
        initialize(&self.root(), &self.runtime, request, &self.key, &mut |_| Ok(()))
    }
    fn recover(&self, request: &RecoveryRequest) -> io::Result<horizon_cloud_protocol::bootstrap::RecoveryReceipt> {
        recover(&Store::open(&self.root())?, &self.runtime, request, &mut |_| Ok(()))
    }
    fn abandon(&self, request: &RecoveryRequest) -> io::Result<BootstrapReceipt> {
        abandon_with(&Store::open(&self.root())?, &self.runtime, request, &mut |_| Ok(()))
    }
}

#[test]
fn initialize_retains_exact_key_and_only_recover_publishes_membership() {
    let f = Fixture::new();
    let init = f.init_request();
    let receipt = f.init(&init).unwrap();
    assert!(!f.root().join(MANIFEST).exists());
    assert_eq!(&*Store::open(&f.root()).unwrap().host_key().unwrap(), &*f.key);
    assert_eq!(f.init(&init).unwrap(), receipt);
    assert!(f.init(&f.init_request()).is_err());
    let request = f.recover_request();
    let recovered = f.recover(&request).unwrap();
    assert_ne!(receipt.fingerprint, recovered.fingerprint);
    assert_eq!(f.recover(&request).unwrap(), recovered);
    assert_eq!(f.init(&init).unwrap(), receipt);
}

#[test]
fn partial_root_and_key_crashes_remain_fenced_but_durable_marker_can_recover() {
    for boundary in [Boundary::Root, Boundary::Key, Boundary::Marker] {
        let f = Fixture::new();
        let request = f.init_request();
        assert!(
            initialize(&f.root(), &f.runtime, &request, &f.key, &mut |at| if at == boundary {
                Err(invalid())
            } else {
                Ok(())
            })
            .is_err()
        );
        if boundary == Boundary::Marker {
            f.recover(&f.recover_request()).unwrap();
        } else {
            assert!(f.init(&request).is_err());
            assert!(f.recover(&f.recover_request()).is_err());
        }
    }
}

#[test]
fn abandon_is_a_terminal_fence_before_and_after_recovery_and_lost_reply() {
    for initialized in [false, true] {
        let f = Fixture::new();
        let init = f.init_request();
        f.init(&init).unwrap();
        let recovery = f.recover_request();
        if initialized {
            f.recover(&recovery).unwrap();
        }
        let abandon = f.abandon_request();
        assert!(
            abandon_with(&Store::open(&f.root()).unwrap(), &f.runtime, &abandon, &mut |_| Err(
                invalid()
            ))
            .is_err()
        );
        let receipt = f.abandon(&abandon).unwrap();
        assert_eq!(receipt.outcome, BootstrapOutcome::Abandoned);
        assert_eq!(f.abandon(&abandon).unwrap(), receipt);
        assert!(f.abandon(&f.abandon_request()).is_err());
        assert!(f.init(&init).is_err());
        assert!(f.recover(&recovery).is_err());
    }
}

#[test]
fn invalid_context_key_and_nonempty_workspace_do_not_create_allocation_root() {
    for variant in 0..5 {
        let f = Fixture::new();
        let mut runtime = f.runtime.clone();
        let mut request = f.init_request();
        match variant {
            0 => runtime.source = Source::LegacyEnvironment,
            1 => runtime.volume_id = "wrong-volume".into(),
            2 => request = Fixture::new().init_request(),
            3 => {
                request = f.signed(BootstrapPayload::Initialize {
                    startup: f.runtime.startup.clone(),
                    worker_id: f.runtime.worker_id.clone(),
                    host_key: "ssh-ed25519 wrong".into(),
                });
            }
            _ => fs::write(f.directory.path().join("workspace/sentinel"), b"preserve").unwrap(),
        }
        assert!(initialize(&f.root(), &runtime, &request, &f.key, &mut |_| Ok(())).is_err());
        assert!(!f.root().exists());
    }
}

#[test]
fn missing_or_changed_retained_key_and_initialized_manifest_never_regenerate() {
    for name in ["ssh-host-key", BOOTSTRAP, MANIFEST] {
        for corrupt in [false, true] {
            let f = Fixture::new();
            let init = f.init_request();
            f.init(&init).unwrap();
            let recovery = f.recover_request();
            f.recover(&recovery).unwrap();
            let path = f.root().join(name);
            if corrupt {
                fs::write(&path, b"invalid").unwrap();
            } else {
                fs::remove_file(&path).unwrap();
            }
            assert!(f.init(&init).is_err());
            assert!(f.recover(&recovery).is_err());
            assert!(f.abandon(&f.abandon_request()).is_err());
            assert_eq!(path.exists(), corrupt);
        }
    }
}

#[test]
fn nonempty_manifest_prevents_abandonment() {
    let f = Fixture::new();
    f.init(&f.init_request()).unwrap();
    f.recover(&f.recover_request()).unwrap();
    let path = f.root().join(MANIFEST);
    let mut manifest: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["members"] = serde_json::json!([{"project":"preserve"}]);
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(f.abandon(&f.abandon_request()).is_err());
}

#[test]
fn private_seed_corruption_with_unchanged_public_key_is_rejected() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    fn skip(bytes: &[u8], offset: &mut usize) -> usize {
        let length = u32::from_be_bytes(bytes[*offset..*offset + 4].try_into().unwrap()) as usize;
        let start = *offset + 4;
        *offset = start + length;
        start
    }
    let f = Fixture::new();
    let original_public = keys::public(&f.key).unwrap();
    keys::seal(f.directory.path()).unwrap();
    f.init(&f.init_request()).unwrap();
    let body = std::str::from_utf8(&f.key)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("---"))
        .collect::<String>();
    let mut binary = STANDARD.decode(body).unwrap();
    let mut offset = b"openssh-key-v1\0".len();
    for _ in 0..3 {
        skip(&binary, &mut offset);
    }
    offset += 4;
    skip(&binary, &mut offset);
    let private = skip(&binary, &mut offset);
    let mut offset = private + 8;
    skip(&binary, &mut offset);
    skip(&binary, &mut offset);
    let seed = skip(&binary, &mut offset);
    binary[seed] ^= 1;
    let corrupted = format!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
        STANDARD.encode(binary)
    );
    assert_eq!(keys::public(corrupted.as_bytes()).unwrap(), original_public);
    fs::write(f.directory.path().join(keys::HOST_KEY), &corrupted).unwrap();
    assert!(keys::captured(f.directory.path()).is_err());
    fs::write(f.root().join(keys::HOST_KEY), corrupted).unwrap();
    assert!(f.recover(&f.recover_request()).is_err());
    assert!(!f.root().join(MANIFEST).exists());
    assert!(f.abandon(&f.abandon_request()).is_err());
}

#[test]
fn recovery_receipt_corruption_blocks_startup_validation_and_abandonment() {
    for field in ["version", "startup", "worker_id"] {
        let f = Fixture::new();
        f.init(&f.init_request()).unwrap();
        f.recover(&f.recover_request()).unwrap();
        let path = f.root().join(BOOTSTRAP);
        let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match field {
            "version" => record["recovery"][field] = serde_json::json!(2),
            "worker_id" => record["recovery"][field] = serde_json::json!("different-worker"),
            _ => record["recovery"][field]["token"] = serde_json::json!(OperationId::generate()),
        }
        fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
        let store = Store::open(&f.root()).unwrap();
        let record: Bootstrap = decode(&store.read(BOOTSTRAP).unwrap().unwrap()).unwrap();
        assert!(record.validate(&store, &f.runtime).is_err());
        drop(store);
        assert!(f.abandon(&f.abandon_request()).is_err());
    }
}
