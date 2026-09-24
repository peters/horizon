use super::*;
use horizon_cloud_protocol::{
    AllocationId, ControllerId, OperationId, SharingMode,
    signed::{ControllerBinding, Intent},
};
use ring::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

struct Fixture {
    root: tempfile::TempDir,
    runtime: Runtime,
    key: Ed25519KeyPair,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let key =
            Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap().as_ref()).unwrap();
        let runtime = Runtime {
            version: 1,
            source: Source::LegacyEnvironment,
            startup: Startup {
                version: 1,
                controller: ControllerBinding::new(
                    AllocationId::generate(),
                    ControllerId::generate(),
                    key.public_key().as_ref().try_into().unwrap(),
                ),
                token: OperationId::generate(),
                sharing: SharingMode::TrustedShared,
                worker_operation: "worker-operation".into(),
                volume_id: "volume-one".into(),
                data_center_id: "center-one".into(),
            },
            worker_id: "worker-one".into(),
            volume_id: "volume-one".into(),
            data_center_id: "center-one".into(),
            worker_operation: "worker-operation".into(),
        };
        let fixture = Self { root, runtime, key };
        fixture.write("allocation.lock", b"fixed allocation lock");
        fixture.write(
            BOOTSTRAP,
            &serde_json::to_vec(&Bootstrap {
                version: 1,
                startup: fixture.runtime.startup.clone(),
                worker_id: fixture.runtime.worker_id.clone(),
                phase: Phase::Initializing,
                recovery: None,
                initialization: None,
                host_key: None,
                key_hash: None,
                abandonment: None,
            })
            .unwrap(),
        );
        fixture
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        fs::write(self.root.path().join(name), bytes).unwrap();
        fs::set_permissions(self.root.path().join(name), fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn request(&self) -> RecoveryRequest {
        let payload = serde_json::to_string(&RecoveryPayload::Recover {
            token: self.runtime.startup.token,
        })
        .unwrap();
        self.signed(payload, OperationId::generate(), 0, Action::Bootstrap)
    }
    fn signed(&self, payload: String, operation: OperationId, revision: u64, action: Action) -> RecoveryRequest {
        let binding = &self.runtime.startup.controller;
        let intent = Intent::new(
            binding,
            operation,
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
    fn recover(&self, request: &RecoveryRequest) -> io::Result<RecoveryReceipt> {
        recover(&Store::open(self.root.path())?, &self.runtime, request, &mut |_| Ok(()))
    }
    fn bootstrap(&self) -> Bootstrap {
        decode(&fs::read(self.root.path().join(BOOTSTRAP)).unwrap()).unwrap()
    }
}

#[test]
fn v1_direct_recovery_remains_supported_without_granting_cold_start_authority() {
    let f = Fixture::new();
    let request = f.request();
    let original = fs::read(f.root.path().join(BOOTSTRAP)).unwrap();
    let mut startup = f.runtime.clone();
    startup.source = Source::StartupCapture;
    {
        let store = Store::open(f.root.path()).unwrap();
        assert!(f.bootstrap().validate(&store, &startup).is_err());
        assert!(recover(&store, &startup, &request, &mut |_| Ok(())).is_err());
    }
    assert_eq!(fs::read(f.root.path().join(BOOTSTRAP)).unwrap(), original);
    assert!(!f.root.path().join(MANIFEST).exists());
    assert!(!f.root.path().join(keys::HOST_KEY).exists());
    f.recover(&request).unwrap();
    assert_eq!(f.bootstrap().phase, Phase::Initialized);
}

#[test]
fn recovery_publishes_once_and_retries_return_the_same_receipt() {
    let f = Fixture::new();
    let request = f.request();
    let receipt = f.recover(&request).unwrap();
    assert_eq!(f.bootstrap().phase, Phase::Initialized);
    let manifest = fs::read(f.root.path().join(MANIFEST)).unwrap();
    assert_eq!(f.recover(&request).unwrap(), receipt);
    assert_eq!(fs::read(f.root.path().join(MANIFEST)).unwrap(), manifest);
    assert_eq!(decode::<Manifest>(&manifest).unwrap().revision, 0);
    assert!(decode::<Manifest>(&manifest).unwrap().members.is_empty());
    assert!(f.recover(&f.request()).is_err());
}

#[test]
fn every_durable_boundary_can_reopen_after_a_crash_or_lost_reply() {
    for boundary in [Boundary::Receipt, Boundary::Manifest, Boundary::Initialized] {
        let f = Fixture::new();
        let request = f.request();
        assert!(
            recover(&Store::open(f.root.path()).unwrap(), &f.runtime, &request, &mut |at| {
                if at == boundary {
                    Err(io::Error::other("simulated crash"))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        let receipt = f.recover(&request).unwrap();
        assert_eq!(f.bootstrap().phase, Phase::Initialized);
        assert_eq!(f.recover(&request).unwrap(), receipt);
    }
}

#[test]
fn missing_or_corrupt_initialized_state_never_resets_membership() {
    for file in [BOOTSTRAP, MANIFEST] {
        for corrupt in [false, true] {
            let f = Fixture::new();
            let request = f.request();
            f.recover(&request).unwrap();
            if corrupt {
                f.write(file, b"corrupt");
            } else {
                fs::remove_file(f.root.path().join(file)).unwrap();
            }
            assert!(f.recover(&request).is_err());
            if corrupt {
                assert_eq!(fs::read(f.root.path().join(file)).unwrap(), b"corrupt");
            } else {
                assert!(!f.root.path().join(file).exists());
            }
        }
    }
    let f = Fixture::new();
    fs::remove_file(f.root.path().join(BOOTSTRAP)).unwrap();
    assert!(f.recover(&f.request()).is_err());
    assert!(!f.root.path().join(MANIFEST).exists());
}

#[test]
fn conflicting_or_nonempty_manifest_blocks_before_binding_a_recovery_operation() {
    for field in ["members", "revision", "version", "worker_id", "unknown"] {
        let f = Fixture::new();
        let original = fs::read(f.root.path().join(BOOTSTRAP)).unwrap();
        let mut manifest = serde_json::json!({
            "version":1,"startup":f.runtime.startup,"worker_id":f.runtime.worker_id,"revision":0,"members":[]
        });
        manifest[field] = match field {
            "members" => serde_json::json!([{"project":"must survive"}]),
            "worker_id" => serde_json::json!("other-worker"),
            _ => serde_json::json!(2),
        };
        let bytes = serde_json::to_vec(&manifest).unwrap();
        f.write(MANIFEST, &bytes);
        assert!(f.recover(&f.request()).is_err());
        assert_eq!(fs::read(f.root.path().join(MANIFEST)).unwrap(), bytes);
        assert_eq!(fs::read(f.root.path().join(BOOTSTRAP)).unwrap(), original);
    }
}

#[test]
fn wrong_key_token_action_revision_and_changed_retry_are_rejected() {
    let f = Fixture::new();
    let request = f.request();
    assert!(f.recover(&Fixture::new().request()).is_err());
    let bad_token = serde_json::to_string(&RecoveryPayload::Recover {
        token: OperationId::generate(),
    })
    .unwrap();
    for (payload, revision, action) in [
        (bad_token, 0, Action::Bootstrap),
        (request.payload.clone(), 1, Action::Bootstrap),
        (request.payload.clone(), 0, Action::InspectAllocation),
        ("{\"action\":\"initialize\"}".into(), 0, Action::Bootstrap),
    ] {
        assert!(
            f.recover(&f.signed(payload, OperationId::generate(), revision, action))
                .is_err()
        );
    }
    assert!(!f.root.path().join(MANIFEST).exists());
    let receipt = f.recover(&request).unwrap();
    let changed = f.signed(format!("{} ", request.payload), receipt.operation, 0, Action::Bootstrap);
    assert!(f.recover(&changed).is_err());
    assert_eq!(f.recover(&request).unwrap(), receipt);
}

#[test]
fn runtime_identity_is_independent_of_the_signed_request() {
    for field in 0..6 {
        let f = Fixture::new();
        let mut runtime = f.runtime.clone();
        match field {
            0 => runtime.worker_id = "other".into(),
            1 => runtime.volume_id = "other".into(),
            2 => runtime.data_center_id = "other".into(),
            3 => runtime.worker_operation = "other".into(),
            4 => runtime.startup.sharing = SharingMode::Dedicated,
            _ => runtime.startup.token = OperationId::generate(),
        }
        assert!(
            recover(
                &Store::open(f.root.path()).unwrap(),
                &runtime,
                &f.request(),
                &mut |_| Ok(())
            )
            .is_err()
        );
        assert!(f.bootstrap().recovery.is_none());
    }
}

#[test]
fn changed_files_at_commit_boundaries_never_report_success() {
    for boundary in [Boundary::Receipt, Boundary::Manifest, Boundary::Initialized] {
        for name in [BOOTSTRAP, MANIFEST] {
            let f = Fixture::new();
            assert!(
                recover(
                    &Store::open(f.root.path()).unwrap(),
                    &f.runtime,
                    &f.request(),
                    &mut |at| {
                        if at == boundary {
                            f.write(name, b"changed");
                        }
                        Ok(())
                    }
                )
                .is_err()
            );
            assert_eq!(fs::read(f.root.path().join(name)).unwrap(), b"changed");
        }
    }
}

#[test]
fn lock_contention_missing_lock_and_substitution_fail_closed() {
    let f = Fixture::new();
    let held = Store::open(f.root.path()).unwrap();
    assert!(Store::open(f.root.path()).is_err());
    fs::rename(f.root.path().join("allocation.lock"), f.root.path().join("old-lock")).unwrap();
    f.write("allocation.lock", b"new lock");
    assert!(recover(&held, &f.runtime, &f.request(), &mut |_| Ok(())).is_err());
    drop(held);
    fs::remove_file(f.root.path().join("allocation.lock")).unwrap();
    assert!(f.recover(&f.request()).is_err());
    assert!(!f.root.path().join("allocation.lock").exists());
}

#[test]
fn unsafe_files_and_paths_are_never_adopted_or_modified() {
    for name in [BOOTSTRAP, MANIFEST, "allocation.lock", ".recovery.next"] {
        for kind in ["symlink", "directory", "public", "fifo", "hardlink"] {
            let f = Fixture::new();
            let path = f.root.path().join(name);
            let _ = fs::remove_file(&path);
            let outside = tempfile::NamedTempFile::new().unwrap();
            fs::write(outside.path(), b"sentinel").unwrap();
            match kind {
                "symlink" => symlink(outside.path(), &path).unwrap(),
                "directory" => fs::create_dir(&path).unwrap(),
                "public" => {
                    f.write(name, b"public");
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
                }
                "fifo" => rustix::fs::mknodat(
                    rustix::fs::CWD,
                    &path,
                    rustix::fs::FileType::Fifo,
                    rustix::fs::Mode::RUSR,
                    0,
                )
                .unwrap(),
                _ => fs::hard_link(outside.path(), &path).unwrap(),
            }
            assert!(f.recover(&f.request()).is_err(), "{name} {kind}");
            assert_eq!(fs::read(outside.path()).unwrap(), b"sentinel");
        }
    }
    let f = Fixture::new();
    let parent = tempfile::tempdir().unwrap();
    symlink(f.root.path(), parent.path().join("alias")).unwrap();
    assert!(Store::open(&parent.path().join("alias")).is_err());
    let store = Store::open(f.root.path()).unwrap();
    fs::rename(f.root.path(), parent.path().join("moved")).unwrap();
    fs::create_dir(f.root.path()).unwrap();
    assert!(store.verify().is_err());
}

#[test]
fn partial_unpublished_staging_is_discarded_only_under_the_existing_lock() {
    let f = Fixture::new();
    f.write(".recovery.next", b"partial");
    let request = f.request();
    f.recover(&request).unwrap();
    assert!(!f.root.path().join(".recovery.next").exists());
    assert_eq!(f.bootstrap().phase, Phase::Initialized);
}

#[test]
fn staged_renamed_and_durable_publications_recover_the_same_initialization() {
    use super::super::store::Publication;
    for at in [Publication::Staged, Publication::Renamed, Publication::Durable] {
        for phase in [Boundary::Receipt, Boundary::Manifest, Boundary::Initialized] {
            let f = Fixture::new();
            let request = f.request();
            let receipt = f.recover(&request).unwrap();
            let initialized = fs::read(f.root.path().join(BOOTSTRAP)).unwrap();
            let expected_manifest = fs::read(f.root.path().join(MANIFEST)).unwrap();
            let mut record = f.bootstrap();
            record.phase = Phase::Initializing;
            f.write(BOOTSTRAP, &serde_json::to_vec(&record).unwrap());
            if phase != Boundary::Initialized {
                fs::remove_file(f.root.path().join(MANIFEST)).unwrap();
            }
            let original = fs::read(f.root.path().join(BOOTSTRAP)).unwrap();
            let file = if phase == Boundary::Manifest {
                MANIFEST
            } else {
                BOOTSTRAP
            };
            let (expected, next) = if phase == Boundary::Receipt {
                // A receipt publication can be interrupted before or after rename.
                record.recovery = None;
                f.write(BOOTSTRAP, &serde_json::to_vec(&record).unwrap());
                (Some(fs::read(f.root.path().join(BOOTSTRAP)).unwrap()), original)
            } else if phase == Boundary::Initialized {
                (Some(original), initialized)
            } else {
                (None, expected_manifest)
            };
            let store = Store::open(f.root.path()).unwrap();
            assert!(
                store
                    .write_with(file, expected.as_deref(), &next, &mut |phase| {
                        if phase == at {
                            Err(io::Error::other("crashed publication"))
                        } else {
                            Ok(())
                        }
                    })
                    .is_err()
            );
            drop(store);
            assert_eq!(f.recover(&request).unwrap(), receipt);
        }
    }
}

#[test]
fn malformed_unsupported_and_oversized_records_never_create_membership() {
    for input in [b"{}".to_vec(), vec![b' '; usize::try_from(LIMIT).unwrap() + 1]] {
        let f = Fixture::new();
        f.write(BOOTSTRAP, &input);
        assert!(f.recover(&f.request()).is_err());
        assert!(!f.root.path().join(MANIFEST).exists());
    }
    for field in ["version", "startup"] {
        let f = Fixture::new();
        let mut record = serde_json::to_value(f.bootstrap()).unwrap();
        if field == "version" {
            record[field] = serde_json::json!(2);
        } else {
            record[field]["version"] = serde_json::json!(2);
        }
        f.write(BOOTSTRAP, &serde_json::to_vec(&record).unwrap());
        assert!(f.recover(&f.request()).is_err());
        assert!(!f.root.path().join(MANIFEST).exists());
    }
    let f = Fixture::new();
    for payload in [
        format!(
            "{{\"action\":\"recover\",\"token\":\"{}\",\"extra\":true}}",
            f.runtime.startup.token
        ),
        format!(
            "{{\"action\":\"recover\",\"token\":\"{}\",\"token\":\"{}\"}}",
            f.runtime.startup.token, f.runtime.startup.token
        ),
    ] {
        assert!(
            f.recover(&f.signed(payload, OperationId::generate(), 0, Action::Bootstrap))
                .is_err()
        );
    }
    assert!(f.bootstrap().recovery.is_none());
}

#[test]
fn command_input_is_bounded_strict_and_preserves_signed_bytes() {
    let f = Fixture::new();
    let request = f.request();
    let encoded = serde_json::to_vec(&request).unwrap();
    let parsed = read_request(encoded.as_slice()).unwrap();
    assert_eq!(parsed.message, request.message);
    assert_eq!(parsed.payload, request.payload);
    for bytes in [
        b"{\"message\":\"\",\"message\":\"\",\"payload\":\"\"}".to_vec(),
        b"{\"message\":\"\",\"payload\":\"\",\"fresh\":true}".to_vec(),
        vec![b' '; usize::try_from(LIMIT).unwrap() + 1],
    ] {
        assert!(read_request(bytes.as_slice()).is_err());
    }
    assert!(f.bootstrap().recovery.is_none());
}

#[test]
#[ignore = "Creates a private fixture for command testing in an isolated mount namespace"]
fn export_native_command_fixture() {
    let destination = std::path::PathBuf::from(std::env::var_os("HORIZON_BOOTSTRAP_FIXTURE").unwrap());
    assert!(destination.is_absolute());
    fs::create_dir(&destination).unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).unwrap();
    let f = Fixture::new();
    let root = destination.join("workspace/.horizon-allocation");
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    for name in [BOOTSTRAP, "allocation.lock"] {
        fs::copy(f.root.path().join(name), root.join(name)).unwrap();
    }
    fs::write(
        destination.join("request.json"),
        serde_json::to_vec(&f.request()).unwrap(),
    )
    .unwrap();
    fs::write(
        destination.join("environment.json"),
        serde_json::to_vec(&serde_json::json!({
            "HORIZON_WORKER_STARTUP":serde_json::to_string(&f.runtime.startup).unwrap(),
            "HORIZON_CLOUD_OPERATION":f.runtime.worker_operation,
            "RUNPOD_POD_ID":f.runtime.worker_id,
            "RUNPOD_VOLUME_ID":f.runtime.volume_id,
            "RUNPOD_DC_ID":f.runtime.data_center_id
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn failed_resynchronization_of_visible_retry_state_never_grants_success() {
    for (boundary, successful_syncs) in [
        (Boundary::Receipt, 0),
        (Boundary::Manifest, 1),
        (Boundary::Initialized, 1),
        (Boundary::Initialized, 2),
    ] {
        let f = Fixture::new();
        let request = f.request();
        assert!(
            recover(&Store::open(f.root.path()).unwrap(), &f.runtime, &request, &mut |at| {
                if at == boundary {
                    Err(io::Error::other("lost reply"))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        let before = fs::read(f.root.path().join(BOOTSTRAP)).unwrap();
        let store = Store::open(f.root.path()).unwrap();
        store.fail_sync_after(successful_syncs);
        assert!(recover(&store, &f.runtime, &request, &mut |_| Ok(())).is_err());
        assert_eq!(fs::read(f.root.path().join(BOOTSTRAP)).unwrap(), before);
        if boundary == Boundary::Receipt {
            assert!(!f.root.path().join(MANIFEST).exists());
        }
        if boundary != Boundary::Initialized {
            assert_eq!(f.bootstrap().phase, Phase::Initializing);
        }
        drop(store);
        f.recover(&request).unwrap();
    }
}

#[test]
fn missing_receipt_cannot_adopt_published_or_initialized_state() {
    for phase in [Phase::Initializing, Phase::Initialized] {
        let f = Fixture::new();
        f.recover(&f.request()).unwrap();
        let manifest = fs::read(f.root.path().join(MANIFEST)).unwrap();
        let mut record = f.bootstrap();
        record.phase = phase;
        record.recovery = None;
        let original = serde_json::to_vec(&record).unwrap();
        f.write(BOOTSTRAP, &original);
        assert!(f.recover(&f.request()).is_err());
        assert_eq!(fs::read(f.root.path().join(BOOTSTRAP)).unwrap(), original);
        assert_eq!(fs::read(f.root.path().join(MANIFEST)).unwrap(), manifest);
    }
}
