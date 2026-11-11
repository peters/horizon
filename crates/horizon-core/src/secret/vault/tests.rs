//! The vault of one computer: a machine key as three files, a credential value in
//! the OS store behind the trait, and the copy that a lost value keeps.

use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};

use horizon_browser::remote::{
    ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
    RemoteAuthentication, RemoteBrowserConfig, RemoteProviderProfile, RemoteSessionLimits,
};

use super::*;
use crate::remote_browser_credential::{CredentialLocator, FakeCredentialStore, SharedCredentialStore};

/// One provider profile whose two references are both bound to the OS store.
fn profile(slot: &str) -> RemoteProviderProfile {
    let mut credential_bindings = BTreeMap::new();
    for (reference, name) in [("user", "username"), ("key", "password")] {
        credential_bindings.insert(
            CredentialReference::from(reference),
            CredentialBinding {
                store: CredentialStoreKind::OsKeychain,
                slot: Some(format!("remote-browser/{slot}/{name}")),
            },
        );
    }
    RemoteProviderProfile {
        adapter: RemoteAdapterKind::Webdriver,
        endpoint: ControlEndpoint::parse("https://grid.example.net/wd/hub").expect("the endpoint"),
        authentication: RemoteAuthentication::Basic {
            username_ref: CredentialReference::from("user"),
            password_ref: CredentialReference::from("key"),
        },
        credential_bindings,
        limits: RemoteSessionLimits::default(),
    }
}

fn config(slot: &str) -> RemoteBrowserConfig {
    let mut remote = RemoteBrowserConfig::default();
    remote.providers.insert("grid".to_owned(), profile(slot));
    remote
}

/// A vault of a temporary Horizon home, with an in-memory credential store that
/// the test can also read.
fn vault() -> (
    tempfile::TempDir,
    SecretVault,
    Arc<CredentialVault>,
    Arc<Mutex<FakeCredentialStore>>,
) {
    let home = tempfile::tempdir().expect("a directory");
    let keys = crate::MachineKeys::new(home.path().join("machine-keys"));
    let fake = Arc::new(Mutex::new(FakeCredentialStore::new()));
    let shared = SharedCredentialStore::new(Arc::clone(&fake));
    let credentials = Arc::new(CredentialVault::new(Box::new(shared)));
    credentials.register(&config("a"));
    let vault = SecretVault::new(keys, Some(Arc::clone(&credentials)));
    (home, vault, credentials, fake)
}

fn files(private_key: &str) -> MachineKeyFiles {
    MachineKeyFiles {
        private_key: private_key.to_owned(),
        public_key: format!("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI{} test", private_key.len()),
        known_hosts: "203.0.113.10 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHOST".to_owned(),
    }
}

fn body(files: &MachineKeyFiles) -> Vec<u8> {
    serde_json::to_vec(files).expect("a body")
}

fn key(id: &str) -> SecretId {
    SecretId::machine_key(id).expect("a key name")
}

#[test]
fn a_machine_key_is_three_private_files_and_a_read_returns_the_same_value() {
    let (home, vault, _, _) = vault();
    let id = key("build-1");
    assert!(vault.read(&id).expect("a read").is_none());

    let value = files("PRIVATE-ONE");
    vault.write(&id, &body(&value)).expect("the value of the server");
    let directory = home.path().join("machine-keys");
    assert_eq!(
        std::fs::metadata(&directory)
            .expect("the directory")
            .permissions()
            .mode()
            & 0o777,
        0o700,
        "the key folder is private"
    );
    for name in ["build-1", "build-1.pub", "build-1.known_hosts"] {
        let path = directory.join(name);
        assert!(path.is_file(), "{name} is written");
        assert_eq!(
            std::fs::metadata(&path).expect(name).permissions().mode() & 0o777,
            0o600,
            "{name} is private"
        );
    }
    assert_eq!(
        std::fs::read_to_string(directory.join("build-1.known_hosts")).expect("known hosts"),
        value.known_hosts
    );
    let read = vault.read(&id).expect("a read").expect("the value");
    assert_eq!(read.to_vec(), body(&value), "the value travels as it came");

    // The key folder lists the pair, and a conflict copy is not a key.
    assert_eq!(
        vault.local_ids(SecretKind::MachineKey),
        vec!["machine-key/build-1".to_string()]
    );

    // A removal takes all three files.
    vault.remove(&id).expect("a removal");
    assert!(vault.read(&id).expect("a read").is_none());
    assert!(vault.remove(&id).is_ok(), "a value that is not there is already gone");
}

#[test]
fn a_machine_key_that_the_server_does_not_read_is_refused_and_keeps_the_local_value() {
    let (_home, vault, _, _) = vault();
    let id = key("build-1");
    let value = files("PRIVATE-LOCAL");
    vault.write(&id, &body(&value)).expect("a local key");

    for bad in [b"not json".as_slice(), b"{}".as_slice(), b"[]".as_slice()] {
        let refused = vault.write(&id, bad).expect_err("a body that is not a key");
        assert_eq!(refused.code(), "unreadable_secret");
    }
    assert_eq!(
        vault.read(&id).expect("a read").expect("the value").to_vec(),
        body(&value),
        "a refused value of the server leaves the local key alone"
    );

    // The value of the server differs, so it is taken over and the local one is
    // kept as one more pair of this home.
    let theirs = files("PRIVATE-THEIRS");
    let notice = vault
        .take_over(&id, &body(&theirs), true, 1_760_000_000_000)
        .expect("a take over")
        .expect("the local value was kept");
    assert!(notice.contains("build-1.conflict-1760000000000"), "{notice}");
    assert_eq!(
        vault.read(&id).expect("a read").expect("the value").to_vec(),
        body(&theirs)
    );
    let conflict = SecretId::machine_key("build-1.conflict-1760000000000");
    assert!(
        conflict.is_err(),
        "a conflict copy is not one key name, so no agent uses it"
    );

    // A value that this computer published is replaced without a copy.
    let newer = files("PRIVATE-NEWER");
    assert!(
        vault
            .take_over(&id, &body(&newer), false, 1_760_000_000_001)
            .expect("a take over")
            .is_none()
    );
}

#[test]
fn one_key_that_differs_only_in_its_known_hosts_is_no_conflict() {
    let (_home, vault, _, _) = vault();
    let id = key("build-1");
    let local = files("PRIVATE-SAME");
    vault.write(&id, &body(&local)).expect("a local key");

    // The same private key and public line, and a known-hosts file that this
    // computer grew by meeting the host by itself.
    let mut theirs = local.clone();
    theirs
        .known_hosts
        .push_str("\n203.0.113.10 ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNOYTItbmlzdHAyNTY");
    assert!(
        vault
            .take_over(&id, &body(&theirs), true, 1_760_000_000_000)
            .expect("a take over")
            .is_none(),
        "a key that both computers hold the same way keeps no copy and reads no notice"
    );
    assert_eq!(
        vault.read(&id).expect("a read").expect("the value").to_vec(),
        body(&theirs),
        "the value of the server is written as it came"
    );
    assert_eq!(
        vault.local_ids(SecretKind::MachineKey),
        vec!["machine-key/build-1".to_string()],
        "no conflict copy is one more key of this home"
    );
}

#[test]
fn a_credential_value_that_the_server_holds_the_same_way_is_no_notice() {
    let (_home, vault, _, _) = vault();
    let id = SecretId::credential("key").expect("a reference");
    vault.write(&id, b"same-value").expect("the value of this computer");
    assert!(
        vault
            .take_over(&id, b"same-value", true, 5)
            .expect("a take over")
            .is_none(),
        "a value that both computers hold the same way reads no notice"
    );
    assert_eq!(
        vault.read(&id).expect("a read").expect("the value").to_vec(),
        b"same-value".to_vec()
    );
}

#[test]
fn a_credential_value_lives_under_its_reference_in_the_os_store() {
    let (_home, vault, credentials, fake) = vault();
    let id = SecretId::credential("key").expect("a reference");
    assert_eq!(
        vault.local_ids(SecretKind::Credential),
        Vec::<String>::new(),
        "an empty store holds no value"
    );
    assert!(vault.read(&id).expect("a read").is_none());

    vault.write(&id, b"s3cret-value").expect("the value of the server");
    assert_eq!(
        vault.local_ids(SecretKind::Credential),
        vec!["credential/key".to_string()]
    );
    assert_eq!(
        fake.lock().expect("the store").len(),
        1,
        "the value is one item of the OS store"
    );

    // The existing read path of a provider finds it.
    let profile = profile("a");
    let reference = CredentialReference::from("key");
    let locator = CredentialLocator::new(&profile.endpoint, &reference, &profile.credential_bindings[&reference]);
    let store = fake.lock().expect("the store");
    assert_eq!(
        crate::secret::read_secret_value(&*store, &locator)
            .expect("the value")
            .to_vec(),
        b"s3cret-value".to_vec()
    );
    drop(store);

    // A reference that no provider binds has no place here.
    let elsewhere = SecretId::credential("other").expect("a reference");
    let refused = vault.write(&elsewhere, b"value").expect_err("no place for it");
    assert_eq!(refused.code(), "unsupported_secret");

    // A local value that loses is one line for the person: the OS store holds one
    // value per reference and the person decides which one is right.
    let notice = vault
        .take_over(&id, b"theirs", true, 5)
        .expect("a take over")
        .expect("the local value lost");
    assert!(notice.contains("credential key"), "{notice}");
    assert_eq!(
        vault.read(&id).expect("a read").expect("the value").to_vec(),
        b"theirs".to_vec()
    );

    vault.remove(&id).expect("a removal");
    assert!(vault.read(&id).expect("a read").is_none());
    let _ = credentials;
}

#[test]
fn the_addresses_of_a_reference_follow_the_providers_of_this_computer() {
    let mut remote = config("a");
    // A second provider that names the same reference with another endpoint.
    let mut second = profile("b");
    second.endpoint = ControlEndpoint::parse("https://second.example.net/wd/hub").expect("the endpoint");
    remote.providers.insert("second".to_owned(), second);
    let locators = crate::secret::os_locators(&remote);
    assert_eq!(locators.len(), 2, "two references");
    for reference in ["key", "user"] {
        assert_eq!(
            locators[reference].len(),
            2,
            "{reference} has one address in each provider"
        );
    }
}
