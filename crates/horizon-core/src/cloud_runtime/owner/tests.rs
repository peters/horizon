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

fn inspect_intent(binding: &ControllerBinding) -> Intent {
    Intent::new(
        binding,
        horizon_cloud_protocol::OperationId::generate(),
        0,
        horizon_cloud_protocol::signed::Target::Allocation {},
        horizon_cloud_protocol::signed::Action::InspectAllocation,
        b"{}",
    )
    .unwrap()
}

fn machine(value: u128) -> MachineReader {
    Box::new(move || serde_json::from_value(json!(uuid::Uuid::from_u128(value))).map_err(|_| Error::Machine))
}

fn open(root: &Path, vault: &MemoryVault) -> Result<Owner> {
    Owner::open_with(root, Box::new(vault.clone()), machine(1))
}

fn interrupt_at(step: Boundary, expected: Boundary) -> Result<()> {
    if step == expected { Err(Error::Journal) } else { Ok(()) }
}

fn fixture() -> (tempfile::TempDir, PathBuf, MemoryVault) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("allocation");
    (temp, root, MemoryVault::default())
}

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
fn exclusive_owner_reopens_the_exact_anchored_journal_and_signs() {
    let (_temp, root, vault) = fixture();
    let mut owner = create(&root, &vault);
    assert!(matches!(open(&root, &vault), Err(Error::Busy)));
    let binding = owner.binding().unwrap();
    let intent = inspect_intent(&binding);
    assert!(owner.sign(intent).unwrap().verify(&binding, b"{}").is_ok());
    owner.save(json!({"state":"stopped"})).unwrap();
    drop(owner);
    let owner = open(&root, &vault).unwrap();
    assert_eq!(owner.load().unwrap(), json!({"state":"stopped"}));
}

#[test]
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
    assert!(matches!(open(&copied, &vault), Err(Error::Ownership)));
    assert!(matches!(
        Owner::open_with(&root, Box::<MemoryVault>::default(), machine(1)),
        Err(Error::MissingRegistration)
    ));
    journal::write(&root, JOURNAL, &old).unwrap();
    assert!(matches!(open(&root, &vault), Err(Error::Journal)));
}

#[test]
fn interrupted_updates_recover_only_the_registered_transition() {
    for boundary in [
        Boundary::Candidate,
        Boundary::Pending,
        Boundary::Published,
        Boundary::Committed,
    ] {
        let (_temp, root, vault) = fixture();
        let mut owner = create(&root, &vault);
        assert!(
            owner
                .save_with(json!({"state":"stopped"}), &mut |step| interrupt_at(step, boundary))
                .is_err()
        );
        assert!(matches!(owner.load(), Err(Error::Registration)));
        assert!(matches!(owner.binding(), Err(Error::Registration)));
        let marker = owner.marker.clone();
        drop(owner);
        if boundary == Boundary::Published {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
            let blocked = open(&root, &vault);
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(blocked.is_err());
            assert!(Registration::read(&vault, &marker).unwrap().pending.is_some());
        }
        let owner = open(&root, &vault).unwrap();
        let expected = if boundary == Boundary::Candidate {
            "prepared"
        } else {
            "stopped"
        };
        assert_eq!(owner.load().unwrap(), json!({"state":expected}));
    }
}

#[test]
fn interrupted_initial_registration_never_adopts_an_existing_directory() {
    for boundary in [
        Boundary::Candidate,
        Boundary::Pending,
        Boundary::Published,
        Boundary::Committed,
    ] {
        let (temp, root, vault) = fixture();
        assert!(
            Owner::create_with(
                &root,
                &temp.path().join("locks"),
                json!({}),
                Box::new(vault.clone()),
                machine(1),
                &mut |step| interrupt_at(step, boundary)
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
        let restored = open(&root, &vault);
        if boundary == Boundary::Candidate {
            assert!(matches!(restored, Err(Error::MissingRegistration)));
        } else {
            assert_eq!(restored.unwrap().load().unwrap(), json!({}));
        }
    }
}

#[test]
fn missing_or_conflicting_recovery_files_remain_fenced() {
    for file in [CANDIDATE, JOURNAL] {
        for corrupt in [false, true] {
            let (_temp, root, vault) = fixture();
            let mut owner = create(&root, &vault);
            assert!(
                owner
                    .save_with(json!({"next":true}), &mut |step| interrupt_at(step, Boundary::Pending))
                    .is_err()
            );
            drop(owner);
            if corrupt {
                std::fs::write(root.join(file), b"corrupt").unwrap();
            } else {
                std::fs::remove_file(root.join(file)).unwrap();
            }
            assert!(open(&root, &vault).is_err());
        }
    }
}

#[test]
fn live_handle_rechecks_registration_machine_and_journal_before_signing() {
    let (_temp, root, vault) = fixture();
    let mut owner = create(&root, &vault);
    let binding = owner.binding().unwrap();
    let intent = inspect_intent(&binding);
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
    assert!(matches!(open(&root, &vault), Err(Error::Busy)));
    drop(owner);
    assert!(open(&root, &vault).is_ok());
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
            let restored = open(&root, &memory).unwrap();
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
        let (_temp, root, vault) = fixture();
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
        assert!(open(&root, &vault).is_err());
    }
}

#[test]
fn replacement_lock_cannot_authorize_a_second_owner_or_keep_the_old_handle_usable() {
    for symlink in [false, true] {
        let (_temp, root, vault) = fixture();
        let mut owner = create(&root, &vault);
        let registration = Registration::read(&vault, &owner.marker).unwrap();
        let moved = registration.lock_path.with_extension("old");
        std::fs::rename(&registration.lock_path, &moved).unwrap();
        if symlink {
            std::os::unix::fs::symlink(&moved, &registration.lock_path).unwrap();
        } else {
            std::fs::write(&registration.lock_path, b"").unwrap();
        }
        assert!(matches!(open(&root, &vault), Err(Error::Ownership)));
        assert!(matches!(owner.binding(), Err(Error::Ownership)));
        assert!(matches!(
            owner.save(json!({"unauthorized":true})),
            Err(Error::Ownership)
        ));
        drop(owner);
        assert!(matches!(open(&root, &vault), Err(Error::Ownership)));
    }
}

#[test]
fn journal_artifacts_reject_links_and_special_files_before_reading_or_recovery() {
    for file in [MARKER, JOURNAL, CANDIDATE] {
        let (temp, root, vault) = fixture();
        let mut owner = create(&root, &vault);
        assert!(
            owner
                .save_with(json!({"next":true}), &mut |step| interrupt_at(step, Boundary::Pending))
                .is_err()
        );
        drop(owner);
        let external = temp.path().join("outside.json");
        std::fs::rename(root.join(file), &external).unwrap();
        std::os::unix::fs::symlink(&external, root.join(file)).unwrap();
        assert!(journal::read(&root, file).is_err());
        assert!(open(&root, &vault).is_err());
        std::fs::remove_file(root.join(file)).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(root.join(file)).unwrap();
        assert!(journal::read(&root, file).is_err());
        assert!(open(&root, &vault).is_err());
    }
}

#[test]
fn signing_backend_wipes_keys_and_rejects_a_foreign_signature() {
    fn requires_wipe<T: zeroize::ZeroizeOnDrop>() {}
    requires_wipe::<SigningKey>();
    let temp = tempfile::tempdir().unwrap();
    let owner = create(&temp.path().join("allocation"), &MemoryVault::default());
    let binding = owner.binding().unwrap();
    let intent = inspect_intent(&binding);
    let foreign = SigningKey::from_bytes(&[99; 32]);
    assert!(matches!(
        SignedIntent::sign_with(intent, &binding, |bytes| foreign.sign(bytes).to_bytes().to_vec()),
        Err(horizon_cloud_protocol::signed::Error::Signature)
    ));
}

#[test]
fn live_owner_refuses_directory_replacement_during_verification() {
    for (ancestor, symlink) in [(false, false), (true, false), (false, true), (true, true)] {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        let root = parent.join("allocation");
        let vault = MemoryVault::default();
        let mut owner = Owner::create_with(
            &root,
            &temp.path().join("locks/nested"),
            json!({}),
            Box::new(vault.clone()),
            machine(1),
            &mut |_| Ok(()),
        )
        .unwrap();
        let intent = inspect_intent(&owner.binding().unwrap());
        let before = vault.0.borrow().clone();
        let moved = temp.path().join("moved");
        let copied = temp.path().join("copied");
        let replaced = if ancestor { &parent } else { &root };
        assert!(
            owner
                .directory
                .verify_with(|| {
                    std::fs::rename(replaced, &moved).unwrap();
                    let source = if ancestor { moved.join("allocation") } else { moved };
                    let destination = if ancestor {
                        copied.join("allocation")
                    } else {
                        copied.clone()
                    };
                    std::fs::create_dir_all(&destination).unwrap();
                    for file in [MARKER, JOURNAL, CANDIDATE] {
                        std::fs::copy(source.join(file), destination.join(file)).unwrap();
                    }
                    if symlink {
                        std::os::unix::fs::symlink(&copied, replaced).unwrap();
                    } else {
                        std::fs::rename(&copied, replaced).unwrap();
                    }
                })
                .is_err()
        );
        assert!(owner.load().is_err());
        assert!(owner.binding().is_err());
        assert!(owner.sign(intent).is_err());
        assert!(owner.save(json!({"changed":true})).is_err());
        assert_eq!(*vault.0.borrow(), before);
    }
}

#[test]
fn lock_nonce_changes_fence_even_an_unchanged_inode() {
    let (_temp, root, vault) = fixture();
    let owner = create(&root, &vault);
    let registration = Registration::read(&vault, &owner.marker).unwrap();
    std::fs::write(&registration.lock_path, [0; 16]).unwrap();
    assert!(matches!(owner.binding(), Err(Error::Ownership)));
    drop(owner);
    assert!(matches!(open(&root, &vault), Err(Error::Ownership)));
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
            let intent = inspect_intent(&binding);
            assert!(owner.sign(intent).unwrap().verify(&binding, b"{}").is_ok());
        }
        _ => panic!("unsupported private fixture phase"),
    }
}

#[test]
fn native_commits_recheck_ownership_and_the_exact_predecessor() {
    for boundary in [Boundary::Candidate, Boundary::Pending, Boundary::Published] {
        for mutation in ["machine", "lock", "marker", "delete", "replace"] {
            let (_temp, root, vault) = fixture();
            let mut owner = create(&root, &vault);
            let lock = owner.lock_path.clone();
            let marker = owner.marker.clone();
            let slot = marker.registration.to_string();
            let machine_id = Rc::new(std::cell::Cell::new(1));
            let reader = machine_id.clone();
            owner.machine = Box::new(move || machine(reader.get())());
            let mut expected = HashMap::new();
            let result = owner.save_with(json!({"next":true}), &mut |step| {
                if step == boundary {
                    match mutation {
                        "machine" => machine_id.set(2),
                        "lock" => std::fs::write(&lock, [0; 16]).unwrap(),
                        "marker" => std::fs::write(root.join(MARKER), b"{}").unwrap(),
                        "delete" => {
                            vault.0.borrow_mut().remove(&slot);
                        }
                        "replace" => {
                            let mut changed = Registration::read(&vault, &marker).unwrap();
                            changed.committed.as_mut().unwrap().hash = [0; 32];
                            changed.write(&vault).unwrap();
                        }
                        _ => unreachable!(),
                    }
                    expected = vault.0.borrow().clone();
                }
                Ok(())
            });
            assert!(result.is_err(), "{boundary:?} {mutation}");
            assert!(!owner.ready);
            assert_eq!(*vault.0.borrow(), expected, "{boundary:?} {mutation}");
        }
    }
}

#[test]
fn recovery_never_recreates_or_overwrites_a_changed_native_predecessor() {
    for deleted in [false, true] {
        let (_temp, root, vault) = fixture();
        let mut owner = create(&root, &vault);
        assert!(
            owner
                .save_with(json!({"next":true}), &mut |step| interrupt_at(step, Boundary::Pending))
                .is_err()
        );
        let mut registration = Registration::read(&vault, &owner.marker).unwrap();
        let mut expected = HashMap::new();
        assert!(
            owner
                .recover(&mut registration, &mut |_| {
                    if deleted {
                        vault.0.borrow_mut().clear();
                    } else {
                        let mut changed = Registration::read(&vault, &owner.marker).unwrap();
                        changed.pending.as_mut().unwrap().next.hash = [0; 32];
                        changed.write(&vault).unwrap();
                    }
                    expected = vault.0.borrow().clone();
                    Ok(())
                })
                .is_err()
        );
        assert!(!owner.ready);
        assert_eq!(*vault.0.borrow(), expected);
    }
}

#[test]
fn relative_lock_roots_are_canonicalized_before_syncing_ancestors() {
    let temp = tempfile::tempdir_in(".").unwrap();
    let relative = PathBuf::from(temp.path().file_name().unwrap()).join("locks/nested");
    assert!(relative.is_relative());
    assert_eq!(
        journal::create_lock_root(&relative).unwrap(),
        relative.canonicalize().unwrap()
    );
}

#[test]
fn an_unavailable_store_is_reported_before_creating_journal_state() {
    struct Unavailable;
    impl Vault for Unavailable {
        fn read(&self, _: &str) -> Result<Zeroizing<Vec<u8>>> {
            Err(Error::Store)
        }
        fn write(&self, _: &str, _: &[u8]) -> Result<()> {
            panic!("unavailable store was written")
        }
    }
    let (_temp, root, _) = fixture();
    assert!(matches!(
        Owner::create_with(
            &root,
            &root.with_extension("locks"),
            json!({}),
            Box::new(Unavailable),
            machine(1),
            &mut |_| Ok(())
        ),
        Err(Error::Store)
    ));
    assert!(!root.exists());
}
