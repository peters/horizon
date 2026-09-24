use super::*;
use serde_json::json;
use std::{cell::RefCell, collections::HashMap, rc::Rc};
use zeroize::Zeroizing;

#[derive(Clone, Default)]
struct MemoryVault(Rc<RefCell<HashMap<String, Vec<u8>>>>);
impl Vault for MemoryVault {
    fn read(&self, slot: &str) -> Result<Zeroizing<Vec<u8>>> {
        self.0
            .borrow()
            .get(slot)
            .cloned()
            .map(Zeroizing::new)
            .ok_or(Error::MissingRegistration)
    }
    fn write(&self, slot: &str, value: &[u8]) -> Result<()> {
        self.0.borrow_mut().insert(slot.into(), value.to_vec());
        Ok(())
    }
}

fn machine(value: u128) -> MachineReader {
    Box::new(move || serde_json::from_value(json!(uuid::Uuid::from_u128(value))).map_err(|_| Error::Machine))
}

#[cfg(unix)]
fn create(root: &Path, vault: &MemoryVault) -> Owner {
    Owner::create_with(
        root,
        &root.parent().unwrap().join("locks"),
        json!({"state":"prepared"}),
        Box::new(vault.clone()),
        machine(1),
        &mut |_| Ok(()),
    )
    .unwrap()
}

#[test]
#[cfg(unix)]
fn exclusive_owner_reopens_the_exact_anchored_journal_and_signs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("allocation");
    let vault = MemoryVault::default();
    let mut owner = create(&root, &vault);
    assert!(matches!(
        Owner::open_with(&root, Box::new(vault.clone()), machine(1)),
        Err(Error::Busy)
    ));
    let binding = owner.binding().unwrap();
    let intent = Intent::new(
        &binding,
        horizon_cloud_protocol::OperationId::generate(),
        0,
        horizon_cloud_protocol::signed::Target::Allocation {},
        horizon_cloud_protocol::signed::Action::InspectAllocation,
        b"{}",
    )
    .unwrap();
    assert!(owner.sign(intent).unwrap().verify(&binding, b"{}").is_ok());
    owner.save(json!({"state":"stopped"})).unwrap();
    drop(owner);
    let owner = Owner::open_with(&root, Box::new(vault), machine(1)).unwrap();
    assert_eq!(owner.load().unwrap(), json!({"state":"stopped"}));
}

#[test]
#[cfg(unix)]
fn copies_missing_registrations_and_rolled_back_same_path_journals_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("allocation");
    let copied = temp.path().join("copied");
    let vault = MemoryVault::default();
    let mut owner = create(&root, &vault);
    let old = journal::read(&root, JOURNAL).unwrap();
    owner.save(json!({"state":"stopped"})).unwrap();
    owner.save(json!({"state":"resumed"})).unwrap();
    drop(owner);
    assert!(matches!(
        Owner::open_with(&root, Box::new(vault.clone()), machine(2)),
        Err(Error::Ownership)
    ));
    std::fs::create_dir(&copied).unwrap();
    for file in [MARKER, JOURNAL, CANDIDATE] {
        std::fs::copy(root.join(file), copied.join(file)).unwrap();
    }
    assert!(matches!(
        Owner::open_with(&copied, Box::new(vault.clone()), machine(1)),
        Err(Error::Ownership)
    ));
    assert!(matches!(
        Owner::open_with(&root, Box::<MemoryVault>::default(), machine(1)),
        Err(Error::MissingRegistration)
    ));
    journal::write(&root, JOURNAL, &old).unwrap();
    assert!(matches!(
        Owner::open_with(&root, Box::new(vault), machine(1)),
        Err(Error::Journal)
    ));
}

#[test]
#[cfg(unix)]
fn interrupted_updates_recover_only_the_registered_transition() {
    for boundary in [
        Boundary::Candidate,
        Boundary::Pending,
        Boundary::Published,
        Boundary::Committed,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("allocation");
        let vault = MemoryVault::default();
        let mut owner = create(&root, &vault);
        assert!(
            owner
                .save_with(json!({"state":"stopped"}), &mut |step| {
                    if step == boundary { Err(Error::Journal) } else { Ok(()) }
                })
                .is_err()
        );
        assert!(matches!(owner.load(), Err(Error::Registration)));
        assert!(matches!(owner.binding(), Err(Error::Registration)));
        drop(owner);
        let owner = Owner::open_with(&root, Box::new(vault), machine(1)).unwrap();
        let expected = if boundary == Boundary::Candidate {
            "prepared"
        } else {
            "stopped"
        };
        assert_eq!(owner.load().unwrap(), json!({"state":expected}));
    }
}

#[test]
#[cfg(unix)]
fn interrupted_initial_registration_never_adopts_an_existing_directory() {
    for boundary in [
        Boundary::Candidate,
        Boundary::Pending,
        Boundary::Published,
        Boundary::Committed,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("allocation");
        let vault = MemoryVault::default();
        assert!(
            Owner::create_with(
                &root,
                &temp.path().join("locks"),
                json!({}),
                Box::new(vault.clone()),
                machine(1),
                &mut |step| { if step == boundary { Err(Error::Journal) } else { Ok(()) } }
            )
            .is_err()
        );
        assert!(
            Owner::create_with(
                &root,
                &temp.path().join("locks"),
                json!({}),
                Box::new(vault.clone()),
                machine(1),
                &mut |_| Ok(())
            )
            .is_err()
        );
        let restored = Owner::open_with(&root, Box::new(vault), machine(1));
        if boundary == Boundary::Candidate {
            assert!(matches!(restored, Err(Error::MissingRegistration)));
        } else {
            assert_eq!(restored.unwrap().load().unwrap(), json!({}));
        }
    }
}

#[test]
#[cfg(unix)]
fn missing_or_conflicting_recovery_files_remain_fenced() {
    for file in [CANDIDATE, JOURNAL] {
        for corrupt in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("allocation");
            let vault = MemoryVault::default();
            let mut owner = create(&root, &vault);
            assert!(
                owner
                    .save_with(json!({"next":true}), &mut |step| {
                        if step == Boundary::Pending {
                            Err(Error::Journal)
                        } else {
                            Ok(())
                        }
                    })
                    .is_err()
            );
            drop(owner);
            if corrupt {
                std::fs::write(root.join(file), b"corrupt").unwrap();
            } else {
                std::fs::remove_file(root.join(file)).unwrap();
            }
            assert!(Owner::open_with(&root, Box::new(vault), machine(1)).is_err());
        }
    }
}

#[test]
#[cfg(unix)]
fn live_handle_rechecks_registration_machine_and_journal_before_signing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("allocation");
    let vault = MemoryVault::default();
    let mut owner = create(&root, &vault);
    let binding = owner.binding().unwrap();
    let intent = Intent::new(
        &binding,
        horizon_cloud_protocol::OperationId::generate(),
        0,
        horizon_cloud_protocol::signed::Target::Allocation {},
        horizon_cloud_protocol::signed::Action::InspectAllocation,
        b"{}",
    )
    .unwrap();
    owner.machine = machine(2);
    assert!(matches!(owner.sign(intent.clone()), Err(Error::Ownership)));
    owner.machine = machine(1);
    let original = vault.0.borrow().clone();
    vault.0.borrow_mut().clear();
    assert!(matches!(owner.sign(intent.clone()), Err(Error::MissingRegistration)));
    *vault.0.borrow_mut() = original;
    journal::write(&root, JOURNAL, b"corrupt").unwrap();
    assert!(matches!(owner.sign(intent), Err(Error::Journal)));
}

#[test]
fn restoring_a_current_copy_at_the_same_path_cannot_replace_the_live_lock() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("allocation");
    let moved = temp.path().join("moved");
    let vault = MemoryVault::default();
    let owner = create(&root, &vault);
    std::fs::rename(&root, &moved).unwrap();
    std::fs::create_dir(&root).unwrap();
    for file in [MARKER, JOURNAL, CANDIDATE] {
        std::fs::copy(moved.join(file), root.join(file)).unwrap();
    }
    assert!(matches!(
        Owner::open_with(&root, Box::new(vault.clone()), machine(1)),
        Err(Error::Busy)
    ));
    drop(owner);
    assert!(Owner::open_with(&root, Box::new(vault), machine(1)).is_ok());
}

struct FailingVault {
    memory: MemoryVault,
    writes: std::cell::Cell<usize>,
    fail_at: usize,
    after_write: bool,
}
impl Vault for FailingVault {
    fn read(&self, slot: &str) -> Result<Zeroizing<Vec<u8>>> {
        self.memory.read(slot)
    }
    fn write(&self, slot: &str, value: &[u8]) -> Result<()> {
        let number = self.writes.get() + 1;
        self.writes.set(number);
        if number == self.fail_at && !self.after_write {
            return Err(Error::Store);
        }
        self.memory.write(slot, value)?;
        if number == self.fail_at {
            return Err(Error::Store);
        }
        Ok(())
    }
}

#[test]
fn failed_and_lost_store_write_replies_fence_the_handle_and_reconcile_exactly() {
    for fail_at in [1, 2] {
        for after_write in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("allocation");
            let memory = MemoryVault::default();
            let mut owner = create(&root, &memory);
            owner.vault = Box::new(FailingVault {
                memory: memory.clone(),
                writes: std::cell::Cell::new(0),
                fail_at,
                after_write,
            });
            assert!(matches!(owner.save(json!({"state":"stopped"})), Err(Error::Store)));
            assert!(matches!(owner.binding(), Err(Error::Registration)));
            drop(owner);
            let restored = Owner::open_with(&root, Box::new(memory), machine(1)).unwrap();
            let state = if fail_at == 1 && !after_write {
                "prepared"
            } else {
                "stopped"
            };
            assert_eq!(restored.load().unwrap(), json!({"state":state}));
        }
    }
}

#[test]
fn missing_lock_changed_public_identity_and_store_secrets_are_never_adopted() {
    for mutation in ["lock", "key", "marker", "version"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("allocation");
        let vault = MemoryVault::default();
        let owner = create(&root, &vault);
        let mut registration = Registration::read(&vault, &owner.marker).unwrap();
        match mutation {
            "lock" => std::fs::remove_file(&registration.lock_path).unwrap(),
            "key" => {
                registration.key.fill(0);
                registration.write(&vault).unwrap();
            }
            "marker" => {
                let mut marker = owner.marker.clone();
                marker.controller = ControllerId::generate();
                journal::write(&root, MARKER, &serde_json::to_vec(&marker).unwrap()).unwrap();
                assert!(matches!(owner.load(), Err(Error::Ownership)));
            }
            "version" => {
                registration.version = 2;
                registration.write(&vault).unwrap();
            }
            _ => unreachable!(),
        }
        drop(owner);
        assert!(Owner::open_with(&root, Box::new(vault), machine(1)).is_err());
    }
}

#[test]
fn replacement_lock_cannot_authorize_a_second_owner_or_keep_the_old_handle_usable() {
    for symlink in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("allocation");
        let vault = MemoryVault::default();
        let mut owner = create(&root, &vault);
        let registration = Registration::read(&vault, &owner.marker).unwrap();
        let moved = registration.lock_path.with_extension("old");
        std::fs::rename(&registration.lock_path, &moved).unwrap();
        if symlink {
            std::os::unix::fs::symlink(&moved, &registration.lock_path).unwrap();
        } else {
            std::fs::write(&registration.lock_path, b"").unwrap();
        }
        assert!(matches!(
            Owner::open_with(&root, Box::new(vault.clone()), machine(1)),
            Err(Error::Ownership)
        ));
        assert!(matches!(owner.binding(), Err(Error::Ownership)));
        assert!(matches!(
            owner.save(json!({"unauthorized":true})),
            Err(Error::Ownership)
        ));
        drop(owner);
        assert!(matches!(
            Owner::open_with(&root, Box::new(vault), machine(1)),
            Err(Error::Ownership)
        ));
    }
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "requires scripts/cloud-smoke/controller-keyring.sh private credential-store fixture"]
fn native_store_fixture() {
    let root = PathBuf::from(std::env::var_os("HORIZON_OWNER_TEST_ROOT").expect("private fixture root"));
    assert!(
        root.is_absolute()
            && root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("horizon-owner-keyring.")
    );
    assert_eq!(
        std::env::var_os("XDG_DATA_HOME"),
        Some(root.join("data").into_os_string())
    );
    assert_eq!(
        std::env::var_os("XDG_RUNTIME_DIR"),
        Some(root.join("runtime").into_os_string())
    );
    assert_eq!(
        std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap(),
        std::env::var("HORIZON_OWNER_TEST_BUS").unwrap()
    );
    let journal_root = root.join("allocation");
    match std::env::var("HORIZON_OWNER_TEST_PHASE").unwrap().as_str() {
        "create" => {
            let mut owner = Owner::create_with(
                &journal_root,
                &root.join("locks"),
                json!({"state":"prepared"}),
                Box::new(NativeVault::open().unwrap()),
                Box::new(MachineId::read),
                &mut |_| Ok(()),
            )
            .unwrap();
            owner.save(json!({"state":"stopped"})).unwrap();
        }
        "reopen" => {
            let owner = Owner::open(&journal_root).unwrap();
            assert_eq!(owner.load().unwrap(), json!({"state":"stopped"}));
            let binding = owner.binding().unwrap();
            let intent = Intent::new(
                &binding,
                horizon_cloud_protocol::OperationId::generate(),
                0,
                horizon_cloud_protocol::signed::Target::Allocation {},
                horizon_cloud_protocol::signed::Action::InspectAllocation,
                b"{}",
            )
            .unwrap();
            assert!(owner.sign(intent).unwrap().verify(&binding, b"{}").is_ok());
        }
        _ => panic!("unsupported private fixture phase"),
    }
}
