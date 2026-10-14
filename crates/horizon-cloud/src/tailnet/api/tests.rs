use super::*;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    path::PathBuf,
};

const TOKEN: &str = "tskey-api-synthetic-only";
const AUTH: &str = "tskey-auth-synthetic-only";

#[derive(Default)]
pub(crate) struct Keys {
    values: RefCell<BTreeMap<String, Zeroizing<Vec<u8>>>>,
    fail_read: Cell<bool>,
    fail_put: Cell<bool>,
    fail_delete: Cell<bool>,
    block_metadata: RefCell<Option<PathBuf>>,
}
impl Keys {
    pub(crate) fn count(&self) -> usize {
        self.values.borrow().len()
    }
}
impl Credentials for Keys {
    fn read(&self, slot: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        if self.fail_read.get() {
            return Err(Error::Keychain);
        }
        Ok(self.values.borrow().get(slot).cloned())
    }
    fn put(&self, slot: &str, bytes: &[u8]) -> Result<()> {
        self.values
            .borrow_mut()
            .insert(slot.into(), Zeroizing::new(bytes.to_vec()));
        if let Some(path) = self.block_metadata.borrow_mut().take() {
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
    std::fs::write(
        root.path().join("tailnets.json"),
        br#"{"tailnets":[{"id":"work","name":"Work"},{"id":"other","name":"Other"}]}"#,
    )
    .unwrap();
    (root, store, Keys::default())
}

fn draft(kind: ApiCredentialKind) -> ApiCredential {
    match kind {
        ApiCredentialKind::PersonalToken => ApiCredential::personal_token(TOKEN).unwrap(),
        ApiCredentialKind::OAuth => ApiCredential::oauth("client-id", "synthetic-client-secret").unwrap(),
    }
}

fn stage(keys: &Keys, id: &str) {
    keys.put(id, &draft(ApiCredentialKind::PersonalToken).encode().unwrap())
        .unwrap();
}

#[test]
fn staged_personal_token_is_consumed_without_metadata_migration_or_secret_output() {
    let (root, store, keys) = fixture();
    stage(&keys, "work");
    assert_eq!(
        status(&store, "work", &keys).unwrap(),
        ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)
    );
    assert!(!root.path().join(METADATA).exists());
    assert_eq!(keys.values.borrow().len(), 1);
    let encoded = serde_json::to_string(&status(&store, "work", &keys).unwrap()).unwrap();
    assert!(!encoded.contains(TOKEN));
    assert_eq!(status(&store, "other", &keys).unwrap(), ApiCredentialStatus::Missing);
    assert!(matches!(status(&store, "unknown", &keys), Err(Error::Missing)));
}

#[test]
fn every_source_replacement_retires_only_the_old_generation_and_staged_account() {
    for first in [ApiCredentialKind::PersonalToken, ApiCredentialKind::OAuth] {
        for second in [ApiCredentialKind::PersonalToken, ApiCredentialKind::OAuth] {
            let (_root, store, keys) = fixture();
            stage(&keys, "work");
            stage(&keys, "other");
            save(&store, "work", &draft(first), &keys).unwrap();
            assert!(!keys.values.borrow().contains_key("work"));
            let before = records(&store).unwrap().slots();
            save(&store, "work", &draft(second), &keys).unwrap();
            assert_eq!(
                status(&store, "work", &keys).unwrap(),
                ApiCredentialStatus::Stored(second)
            );
            assert!(before.iter().all(|slot| !keys.values.borrow().contains_key(slot)));
            assert_eq!(keys.values.borrow().len(), 2);
            assert!(keys.values.borrow().contains_key("other"));
        }
    }
}

#[test]
fn remove_disables_staged_fallback_even_when_cleanup_is_interrupted() {
    let (root, store, keys) = fixture();
    stage(&keys, "work");
    keys.fail_delete.set(true);
    assert!(matches!(remove(&store, "work", &keys), Err(Error::Keychain)));
    assert!(matches!(records(&store).unwrap().bindings["work"], Binding::Disabled));
    assert!(root.path().join(JOURNAL).exists());
    // Failure remains explicit; the old token never becomes a ready credential again.
    assert!(matches!(status(&store, "work", &keys), Err(Error::Keychain)));
    keys.fail_delete.set(false);
    recover(&store, &keys).unwrap();
    assert!(keys.values.borrow().is_empty());
    stage(&keys, "work");
    assert_eq!(status(&store, "work", &keys).unwrap(), ApiCredentialStatus::Missing);
    remove(&store, "work", &keys).unwrap();
    assert!(
        keys.values.borrow().is_empty(),
        "repeated removal also clears a newly staged account"
    );
    save(&store, "work", &draft(ApiCredentialKind::OAuth), &keys).unwrap();
    assert!(!keys.values.borrow().contains_key("work"));
}

#[test]
fn interruption_recovery_preserves_the_published_source_at_every_boundary() {
    for managed in [false, true] {
        for boundary in 0..4 {
            let (root, store, keys) = fixture();
            stage(&keys, "work");
            if managed {
                save(&store, "work", &draft(ApiCredentialKind::PersonalToken), &keys).unwrap();
            }
            let before = records(&store).unwrap();
            let mut after = before.clone();
            after.bindings.insert(
                "work".into(),
                Binding::Active {
                    slot: "new-slot".into(),
                    kind: ApiCredentialKind::OAuth,
                },
            );
            let journal = pending(before.clone(), after.clone(), "work", Some("new-slot".into()));
            write(&root.path().join(JOURNAL), &journal).unwrap();
            if boundary >= 1 {
                keys.put("new-slot", &draft(ApiCredentialKind::OAuth).encode().unwrap())
                    .unwrap();
            }
            if boundary >= 2 {
                write(&root.path().join(METADATA), &after).unwrap();
            }
            if boundary >= 3 {
                for slot in &journal.retired {
                    keys.delete(slot).unwrap();
                }
            }
            recover(&store, &keys).unwrap();
            recover(&store, &keys).unwrap();
            assert!(records(&store).unwrap() == if boundary < 2 { before } else { after });
            let expected = if boundary < 2 {
                ApiCredentialKind::PersonalToken
            } else {
                ApiCredentialKind::OAuth
            };
            assert_eq!(
                status(&store, "work", &keys).unwrap(),
                ApiCredentialStatus::Stored(expected)
            );
            assert_eq!(keys.values.borrow().len(), 1);
            assert!(!root.path().join(JOURNAL).exists());
        }
    }
}

#[test]
fn failed_secret_write_or_metadata_commit_recovers_without_losing_staged_token() {
    for failure in [false, true] {
        let (root, store, keys) = fixture();
        stage(&keys, "work");
        if failure {
            keys.fail_put.set(true);
        } else {
            *keys.block_metadata.borrow_mut() = Some(root.path().join(METADATA));
        }
        assert!(save(&store, "work", &draft(ApiCredentialKind::OAuth), &keys).is_err());
        assert!(keys.values.borrow().contains_key("work"));
        if !failure {
            std::fs::remove_dir(root.path().join(METADATA)).unwrap();
        }
        recover(&store, &keys).unwrap();
        assert_eq!(
            status(&store, "work", &keys).unwrap(),
            ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)
        );
        assert_eq!(keys.values.borrow().len(), 1);
    }
}

#[test]
fn api_metadata_never_contains_secrets_or_changes_enrollment_bindings() {
    let (root, store, keys) = fixture();
    let auth_catalog = std::fs::read(root.path().join("tailnets.json")).unwrap();
    std::fs::write(root.path().join("auth-fixture"), AUTH).unwrap();
    keys.fail_delete.set(true);
    assert!(save(&store, "work", &draft(ApiCredentialKind::OAuth), &keys).is_err());
    for path in [METADATA, JOURNAL] {
        let bytes = std::fs::read(root.path().join(path)).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains(TOKEN) && !text.contains("synthetic-client-secret") && !text.contains("client-id"));
    }
    assert_eq!(std::fs::read(root.path().join("tailnets.json")).unwrap(), auth_catalog);
    assert_eq!(std::fs::read_to_string(root.path().join("auth-fixture")).unwrap(), AUTH);
}

#[test]
fn missing_locked_and_invalid_credential_have_distinct_value_free_results() {
    let (_root, store, keys) = fixture();
    assert_eq!(status(&store, "work", &keys).unwrap(), ApiCredentialStatus::Missing);
    keys.fail_read.set(true);
    assert!(matches!(status(&store, "work", &keys), Err(Error::Keychain)));
    keys.fail_read.set(false);
    for bytes in [
        b"private-malformed-secret".as_slice(),
        br#"{"kind":"personal_token","token":"tskey-auth-private"}"#,
        br#"{"kind":"personal_token","token":"tskey-api-private","client_secret":"private"}"#,
        br#"{"kind":"personal_token","token":"tskey-api-private","client_secret":null}"#,
        br#"{"kind":"oauth","client_id":"client-id"}"#,
        br#"{"kind":"oauth","client_id":"client-id","client_secret":"private","extra":"private"}"#,
    ] {
        keys.put("work", bytes).unwrap();
        let error = status(&store, "work", &keys).unwrap_err();
        assert!(matches!(error, Error::InvalidApiCredential));
        assert!(!format!("{error:?} {error}").contains("private"));
    }
}

#[test]
fn conflicting_metadata_or_forged_journal_cannot_retire_another_network() {
    let (root, store, keys) = fixture();
    stage(&keys, "other");
    let mut after = Records::default();
    after.bindings.insert(
        "work".into(),
        Binding::Active {
            slot: "new-slot".into(),
            kind: ApiCredentialKind::OAuth,
        },
    );
    let mut journal = pending(Records::default(), after, "work", Some("new-slot".into()));
    journal.retired.insert("other".into());
    write(&root.path().join(JOURNAL), &journal).unwrap();
    assert!(matches!(recover(&store, &keys), Err(Error::Storage)));
    assert!(keys.values.borrow().contains_key("other"));
    assert!(root.path().join(JOURNAL).exists());
    std::fs::remove_file(root.path().join(JOURNAL)).unwrap();
    let mut records = Records::default();
    records.bindings.insert(
        "work".into(),
        Binding::Active {
            slot: "other".into(),
            kind: ApiCredentialKind::PersonalToken,
        },
    );
    write(&root.path().join(METADATA), &records).unwrap();
    assert!(matches!(status(&store, "work", &keys), Err(Error::Storage)));
}

#[test]
fn source_kind_must_match_the_published_binding() {
    let (_root, store, keys) = fixture();
    save(&store, "work", &draft(ApiCredentialKind::OAuth), &keys).unwrap();
    let slot = records(&store).unwrap().slots().pop_first().unwrap();
    stage(&keys, &slot);
    assert!(matches!(
        status(&store, "work", &keys),
        Err(Error::InvalidApiCredential)
    ));
}

#[test]
fn drafts_and_stored_payloads_are_bounded_and_accept_only_the_named_source_fields() {
    for token in [
        "",
        "tskey-api-",
        "tskey-api-  ",
        "tskey-auth-wrong-kind",
        "tskey-api-internal whitespace",
    ] {
        assert!(matches!(
            ApiCredential::personal_token(token),
            Err(Error::InvalidApiCredential)
        ));
    }
    assert!(
        ApiCredential::personal_token("tskey-api-a").is_ok(),
        "stored shape is distinct from server validation"
    );
    assert!(ApiCredential::personal_token(&format!("tskey-api-{}", "a".repeat(4096))).is_err());
    assert!(ApiCredential::oauth("", "secret").is_err());
    assert!(ApiCredential::oauth("id", "internal\ncontrol").is_err());
    assert!(ApiCredential::decode(&vec![b'a'; 8193]).is_err());
    let staged = br#"{"kind":"personal_token","token":"tskey-api-synthetic-only"}"#;
    let credential = ApiCredential::decode(staged).unwrap();
    assert_eq!(&*credential.encode().unwrap(), staged);
    let escaped = br#"{"kind":"oauth","client_id":"client-id","client_secret":"synthetic\u002dsecret"}"#;
    assert_eq!(ApiCredential::decode(escaped).unwrap().kind(), ApiCredentialKind::OAuth);
}

#[test]
fn oversized_sidecar_is_rejected_before_secret_mutation() {
    let (root, store, keys) = fixture();
    stage(&keys, "work");
    let mut huge = Records::default();
    for index in 0..2000 {
        huge.bindings.insert(format!("network-{index}"), Binding::Disabled);
    }
    assert!(matches!(huge.validate(&store), Err(Error::Storage)));
    write(&root.path().join(METADATA), &huge).unwrap();
    assert!(matches!(
        save(&store, "work", &draft(ApiCredentialKind::OAuth), &keys),
        Err(Error::Storage)
    ));
    assert_eq!(keys.values.borrow().len(), 1);
    assert!(keys.values.borrow().contains_key("work"));
    assert!(!root.path().join(JOURNAL).exists());
}

#[test]
fn missing_managed_secret_never_falls_back_to_a_staged_old_token() {
    let (_root, store, keys) = fixture();
    save(&store, "work", &draft(ApiCredentialKind::OAuth), &keys).unwrap();
    let slot = records(&store).unwrap().slots().pop_first().unwrap();
    keys.delete(&slot).unwrap();
    stage(&keys, "work");
    assert_eq!(status(&store, "work", &keys).unwrap(), ApiCredentialStatus::Missing);
}

#[test]
fn maximum_valid_drafts_remain_readable_after_json_escaping_and_storage() {
    let (_root, store, keys) = fixture();
    let token = format!("tskey-api-{}", "\\".repeat(4096 - "tskey-api-".len()));
    let personal = ApiCredential::personal_token(&token).unwrap();
    save(&store, "work", &personal, &keys).unwrap();
    assert_eq!(
        status(&store, "work", &keys).unwrap(),
        ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)
    );
    let oauth = ApiCredential::oauth(&"\\".repeat(256), &"\"".repeat(4096)).unwrap();
    save(&store, "work", &oauth, &keys).unwrap();
    assert_eq!(
        status(&store, "work", &keys).unwrap(),
        ApiCredentialStatus::Stored(ApiCredentialKind::OAuth)
    );
    assert!(ApiCredential::decode(&vec![b' '; 16 * 1024 + 1]).is_err());
}
