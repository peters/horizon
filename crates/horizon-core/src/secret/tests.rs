//! The ids of the secrets of this Horizon, and the values behind them.

use crate::secret::{MAX_SECRET_ID_CHARS, SecretId, SecretKind, SecretTarget};

#[test]
fn an_id_names_a_machine_key_or_a_credential_and_nothing_else() {
    let key = SecretId::machine_key("build-1").expect("one key name");
    assert_eq!(key.as_str(), "machine-key/build-1");
    assert_eq!(key.name(), "build-1");
    assert_eq!(key.kind(), SecretKind::MachineKey);
    assert_eq!(key.to_string(), "machine-key/build-1");

    let credential = SecretId::credential("key").expect("one reference");
    assert_eq!(credential.as_str(), "credential/key");
    assert_eq!(credential.name(), "key");
    assert_eq!(credential.kind(), SecretKind::Credential);

    // A reference may hold a dot, an underscore and a dash, and an upper case
    // letter; a key name may not.
    assert!(SecretId::credential("Remote-Browser.access_key").is_ok());
    for bad in ["", "-build", "build-", "Build", "build/1", &"a".repeat(41)] {
        assert!(SecretId::machine_key(bad).is_err(), "{bad:?} is no key name");
    }
    for bad in ["", "a b", "a/b", "a:b", &"a".repeat(65)] {
        assert!(SecretId::credential(bad).is_err(), "{bad:?} is no reference");
    }

    // An id from a server is parsed, and an id that is not one of this Horizon is
    // never a path and never a longer name than a name.
    for text in [
        "machine-key/build-1",
        "credential/key",
        "credential/remote-browser.grid.key",
    ] {
        assert_eq!(SecretId::parse(text).expect(text).as_str(), text);
    }
    for text in [
        "",
        "/",
        "machine-key",
        "machine-key/",
        "machine-key/../build-1",
        "machine-key/build-1/x",
        "credential/a b",
        "secret/build-1",
        "machine-keys/build-1",
        &format!("credential/{}", "a".repeat(70)),
    ] {
        assert!(SecretId::parse(text).is_err(), "{text:?} is no id");
    }

    // A refused id quotes at most one id, not a frame.
    let long = "m".repeat(500);
    let message = SecretId::parse(&long).expect_err("a long id").to_string();
    assert!(message.chars().count() < MAX_SECRET_ID_CHARS + 200, "{message}");
}

#[test]
fn every_kind_has_one_code() {
    assert_eq!(SecretKind::MachineKey.code(), "machine-key");
    assert_eq!(SecretKind::Credential.code(), "credential");
}

/// The trait is what the link builds on: a target that holds nothing answers no
/// value and no local id.
#[derive(Debug, Default)]
struct Nothing;

impl SecretTarget for Nothing {
    fn read(&self, _: &SecretId) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, crate::SecretError> {
        Ok(None)
    }

    fn write(&self, _: &SecretId, _: &[u8]) -> Result<(), crate::SecretError> {
        Ok(())
    }

    fn remove(&self, _: &SecretId) -> Result<(), crate::SecretError> {
        Ok(())
    }

    fn keep_local(&self, _: &SecretId, _: &[u8], _: u64) -> Result<String, crate::SecretError> {
        Ok(String::new())
    }

    fn local_ids(&self, _: SecretKind) -> Vec<String> {
        Vec::new()
    }
}

#[test]
fn a_target_without_a_value_takes_one_and_keeps_nothing() {
    let target = Nothing;
    let id = SecretId::machine_key("build-1").expect("an id");
    assert!(target.read(&id).expect("a read").is_none());
    assert!(target.take_over(&id, b"body", true, 7).expect("a take over").is_none());
    assert!(target.local_ids(SecretKind::MachineKey).is_empty());
}
