use super::*;
use std::cell::{Cell, RefCell};

const KEY: &str = "tskey-auth-synthetic12345678901234567890";
#[derive(Default)]
struct Keys {
    values: RefCell<BTreeMap<String, String>>,
    fail_put: Cell<bool>,
    fail_delete: Cell<bool>,
    block_catalog: RefCell<Option<PathBuf>>,
}
impl Credentials for Keys {
    fn put(&self, slot: &str, key: &str) -> Result<()> {
        self.values.borrow_mut().insert(slot.into(), key.into());
        if let Some(path) = self.block_catalog.borrow_mut().take() {
            std::fs::create_dir(path).unwrap();
        }
        if self.fail_put.replace(false) {
            Err(Error::Keychain)
        } else {
            Ok(())
        }
    }
    fn delete(&self, slot: &str) -> Result<()> {
        if self.fail_delete.get() {
            return Err(Error::Keychain);
        }
        self.values.borrow_mut().remove(slot);
        Ok(())
    }
}
fn fixture() -> (tempfile::TempDir, Store, Keys) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into());
    (root, store, Keys::default())
}

#[test]
fn auth_and_api_rotations_are_independent_and_network_removal_cleans_both() {
    use super::super::api::{self, ApiCredential, ApiCredentialKind, ApiCredentialStatus};
    let (root, store, keys) = fixture();
    let api_keys = api::tests::Keys::default();
    store.save_with(Some("work"), "Work", KEY, &keys).unwrap();
    api::save(
        &store,
        "work",
        &ApiCredential::personal_token("tskey-api-synthetic-only").unwrap(),
        &api_keys,
    )
    .unwrap();
    let metadata = std::fs::read(root.path().join("tailnet-api-bindings.json")).unwrap();
    let original_auth = store.credential_slot("work").unwrap();
    store.save_with(Some("work"), "New name", KEY, &keys).unwrap();
    let rotated_auth = store.credential_slot("work").unwrap();
    assert_ne!(original_auth, rotated_auth);
    assert_eq!(
        std::fs::read(root.path().join("tailnet-api-bindings.json")).unwrap(),
        metadata
    );
    assert_eq!(
        api::status(&store, "work", &api_keys).unwrap(),
        ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)
    );
    api::save(
        &store,
        "work",
        &ApiCredential::oauth("client-id", "synthetic-client-secret").unwrap(),
        &api_keys,
    )
    .unwrap();
    assert_eq!(store.credential_slot("work").unwrap(), rotated_auth);
    assert_eq!(keys.values.borrow()[&rotated_auth], KEY);
    store.delete_with("work", &keys, &api_keys).unwrap();
    assert!(keys.values.borrow().is_empty());
    assert_eq!(api_keys.count(), 0);
    assert!(store.load().unwrap().tailnets.is_empty());
}

#[test]
fn corrupt_api_cleanup_never_blocks_auth_catalog_recovery_or_rotation() {
    use super::super::api;
    let (root, store, keys) = fixture();
    store.save_with(Some("work"), "Work", KEY, &keys).unwrap();
    let journal = br#"{"invalid":"synthetic-api-journal"}"#;
    std::fs::write(root.path().join("tailnet-api.pending.json"), journal).unwrap();
    store.recover_with(&keys).unwrap();
    assert_eq!(store.load().unwrap().tailnets[0].id, "work");
    let old = store.credential_slot("work").unwrap();
    store.save_with(Some("work"), "New name", KEY, &keys).unwrap();
    assert_ne!(store.credential_slot("work").unwrap(), old);
    assert_eq!(store.load().unwrap().tailnets[0].name, "New name");
    assert!(matches!(
        api::status(&store, "work", &api::tests::Keys::default()),
        Err(Error::Storage)
    ));
    assert_eq!(
        std::fs::read(root.path().join("tailnet-api.pending.json")).unwrap(),
        journal
    );
}

#[test]
fn metadata_reader_caps_bytes_before_deserializing_a_large_file() {
    let (root, store, _keys) = fixture();
    let file = File::create(root.path().join("oversized.json")).unwrap();
    file.set_len(64 * 1024 * 1024).unwrap();
    assert!(matches!(
        store.read::<serde_json::Value>("oversized.json", 16),
        Err(Error::Storage)
    ));
    std::fs::write(root.path().join("bounded.json"), b"null").unwrap();
    assert_eq!(
        store.read::<serde_json::Value>("bounded.json", 4).unwrap(),
        Some(serde_json::Value::Null)
    );
    assert!(store.read::<serde_json::Value>("missing.json", 4).unwrap().is_none());
}

#[test]
fn recovery_preserves_the_published_generation_at_every_interruption_boundary() {
    for existing in [false, true] {
        for boundary in 0..4 {
            let (root, store, keys) = fixture();
            if existing {
                store.save_with(Some("work"), "Work", KEY, &keys).unwrap();
            }
            let before = store.records().unwrap();
            let old = before.slot("work").ok();
            let mut after = before.clone();
            if !existing {
                after.tailnets.push(Tailnet {
                    id: "work".into(),
                    name: "Work".into(),
                });
            }
            after.slots.insert("work".into(), "new-generation".into());
            let pending = Pending {
                before: before.clone(),
                after: after.clone(),
                created: Some("new-generation".into()),
                retired: old.clone(),
            };
            write(&root.path().join("tailnets.pending.json"), &pending).unwrap();
            if boundary >= 1 {
                keys.put("new-generation", KEY).unwrap();
            }
            if boundary >= 2 {
                write(&root.path().join("tailnets.json"), &after).unwrap();
            }
            if boundary >= 3
                && let Some(slot) = &old
            {
                keys.delete(slot).unwrap();
            }
            let restarted = Store::new(root.path().into());
            restarted.recover_with(&keys).unwrap();
            restarted.recover_with(&keys).unwrap();
            let expected = if boundary < 2 { before } else { after };
            assert!(restarted.records().unwrap() == expected);
            assert_eq!(
                keys.values.borrow().keys().cloned().collect::<BTreeSet<_>>(),
                expected.credentials()
            );
            assert!(!root.path().join("tailnets.pending.json").exists());
        }
    }
}
#[test]
fn deletion_is_recovered_before_or_after_catalog_publication() {
    for committed in [false, true] {
        let (root, store, keys) = fixture();
        store.save_with(Some("work"), "Work", KEY, &keys).unwrap();
        let before = store.records().unwrap();
        let pending = Pending {
            retired: Some(before.slot("work").unwrap()),
            before: before.clone(),
            after: Records::default(),
            created: None,
        };
        write(&root.path().join("tailnets.pending.json"), &pending).unwrap();
        if committed {
            write(&root.path().join("tailnets.json"), &pending.after).unwrap();
        }
        Store::new(root.path().into()).recover_with(&keys).unwrap();
        assert_eq!(store.load().unwrap().tailnets.len(), usize::from(!committed));
        assert_eq!(keys.values.borrow().len(), usize::from(!committed));
    }
}
#[test]
fn failed_key_write_and_failed_catalog_rename_leave_recoverable_cleanup() {
    for key_failure in [false, true] {
        let (root, store, keys) = fixture();
        if key_failure {
            keys.fail_put.set(true);
        } else {
            *keys.block_catalog.borrow_mut() = Some(root.path().join("tailnets.json"));
        }
        assert!(store.save_with(None, "Work", KEY, &keys).is_err());
        assert_eq!(keys.values.borrow().len(), 1);
        assert!(root.path().join("tailnets.pending.json").exists());
        if !key_failure {
            std::fs::remove_dir(root.path().join("tailnets.json")).unwrap();
        }
        Store::new(root.path().into()).recover_with(&keys).unwrap();
        assert!(keys.values.borrow().is_empty());
        assert!(store.load().unwrap().tailnets.is_empty());
    }
}
#[test]
fn cleanup_failure_keeps_both_generations_addressable_until_retry() {
    let (root, store, keys) = fixture();
    store.save_with(Some("work"), "Work", KEY, &keys).unwrap();
    let prior = store.credential_slot("work").unwrap();
    keys.fail_delete.set(true);
    assert!(store.save_with(Some("work"), "Renamed", KEY, &keys).is_err());
    let published = store.credential_slot("work").unwrap();
    assert_ne!(prior, published);
    assert_eq!(keys.values.borrow().len(), 2);
    assert!(root.path().join("tailnets.pending.json").exists());
    keys.fail_delete.set(false);
    store.recover_with(&keys).unwrap();
    assert_eq!(
        keys.values.borrow().keys().cloned().collect::<Vec<_>>(),
        vec![published]
    );
    assert_eq!(store.load().unwrap().tailnets[0].name, "Renamed");
    assert!(!root.path().join("tailnets.pending.json").exists());
}
#[test]
fn legacy_slots_migrate_without_changing_the_public_binding_id() {
    let (root, store, keys) = fixture();
    let legacy = Records {
        tailnets: vec![Tailnet {
            id: "work".into(),
            name: "Work".into(),
        }],
        ..Records::default()
    };
    write(&root.path().join("tailnets.json"), &legacy).unwrap();
    keys.put("work", KEY).unwrap();
    assert_eq!(store.credential_slot("work").unwrap(), "work");
    let updated = store.save_with(Some("work"), "Work", KEY, &keys).unwrap();
    assert_eq!(updated.tailnets[0].id, "work");
    assert_ne!(store.credential_slot("work").unwrap(), "work");
    assert!(!keys.values.borrow().contains_key("work"));
    assert!(!serde_json::to_string(&updated).unwrap().contains("slots"));
    store
        .delete_with("work", &keys, &super::super::api::tests::Keys::default())
        .unwrap();
    assert!(keys.values.borrow().is_empty());
}
#[test]
fn conflicting_catalog_or_corrupt_journal_never_deletes_credentials() {
    let (root, store, keys) = fixture();
    store.save_with(Some("work"), "Work", KEY, &keys).unwrap();
    let before = store.records().unwrap();
    let pending = Pending {
        before: Records::default(),
        after: Records::default(),
        created: Some("unrelated".into()),
        retired: None,
    };
    write(&root.path().join("tailnets.pending.json"), &pending).unwrap();
    assert!(store.recover_with(&keys).is_err());
    assert!(store.records().unwrap() == before);
    assert_eq!(keys.values.borrow().len(), 1);
}
