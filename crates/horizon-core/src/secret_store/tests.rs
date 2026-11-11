//! The secrets file of one Horizon home: its rows, its modes and the one connection
//! that this process holds for it.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
    thread,
};

use super::{SECRETS_FILE_NAME, SecretStore};
use crate::HorizonHome;
use crate::remote_browser_credential::{CredentialLocator, RemoteCredentialError, RemoteCredentialStore, Sealed};

/// A store of a temporary folder, and the folder.
fn store() -> (tempfile::TempDir, Arc<SecretStore>) {
    let root = tempfile::tempdir().expect("a directory");
    let store = SecretStore::open(root.path().join(SECRETS_FILE_NAME)).expect("the store");
    (root, store)
}

/// The path of a secrets file in a new folder of `root`.
fn home(root: &Path) -> PathBuf {
    root.join("horizon-home").join(SECRETS_FILE_NAME)
}

/// A locator of one credential reference of a provider.
fn credential(reference: &str) -> CredentialLocator {
    use horizon_browser::remote::{ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind};
    let profile = (
        ControlEndpoint::parse("https://grid.example.net/wd/hub").expect("the endpoint"),
        CredentialBinding {
            store: CredentialStoreKind::File,
            slot: Some(format!("remote-browser/{reference}")),
        },
    );
    let reference = CredentialReference::from(reference);
    CredentialLocator::new(&profile.0, &reference, &profile.1)
}

/// The value that a store holds for `locator`, read through the sealed trait.
fn value_of(store: &Arc<SecretStore>, locator: &CredentialLocator) -> Option<Vec<u8>> {
    let mut sink = ValueSink::default();
    store.with_secret(locator, &mut sink).ok()?;
    sink.value
}

#[derive(Default)]
struct ValueSink {
    value: Option<Vec<u8>>,
}

impl Sealed for ValueSink {}

impl crate::remote_browser_credential::SecretSink for ValueSink {
    fn accept(&mut self, bytes: &[u8]) -> Result<(), RemoteCredentialError> {
        self.value = Some(bytes.to_vec());
        Ok(())
    }
}

#[test]
fn a_value_survives_a_second_open_of_the_file() {
    let root = tempfile::tempdir().expect("a directory");
    let path = root.path().join(SECRETS_FILE_NAME);
    {
        let mut store = SecretStore::open(path.clone()).expect("the store");
        store
            .put(&credential("grid"), b"the-value")
            .expect("the value of a provider");
        store
            .put(&crate::named_secrets::locator(&name()), b"the-named-value")
            .expect("the value of a named secret");
    }
    let store = SecretStore::open(path).expect("the store again");
    assert_eq!(
        value_of(&store, &credential("grid")).as_deref(),
        Some(&b"the-value"[..])
    );
    assert_eq!(
        value_of(&store, &crate::named_secrets::locator(&name())).as_deref(),
        Some(&b"the-named-value"[..])
    );
    assert_eq!(
        store.ids().expect("the ids"),
        vec!["credential/grid".to_owned(), "named/TOKEN".to_owned()]
    );
}

fn name() -> crate::named_secrets::SecretName {
    crate::named_secrets::SecretName::parse("TOKEN").expect("a name")
}

#[test]
fn two_opens_of_one_path_answer_one_store() {
    let root = tempfile::tempdir().expect("a directory");
    let path = home(root.path());
    let first = SecretStore::open(path.clone()).expect("the store");
    let second = SecretStore::open(path.clone()).expect("the store again");
    assert!(Arc::ptr_eq(&first, &second), "one file has one store");
    assert_eq!(SecretStore::opens(&path), 1, "the file is opened once");
    // Two spellings of one path are one file, so they are one store too.
    let spelled = root.path().join("horizon-home").join(".").join(SECRETS_FILE_NAME);
    let third = SecretStore::open(spelled).expect("the store of the same path");
    assert!(Arc::ptr_eq(&first, &third), "a second spelling of a path is the file");
    assert_eq!(SecretStore::opens(&path), 1, "and it opens no second connection");
}

#[test]
fn a_store_ends_with_its_last_holder() {
    let root = tempfile::tempdir().expect("a directory");
    let path = home(root.path());
    let first = SecretStore::open(path.clone()).expect("the store");
    let second = SecretStore::open(path.clone()).expect("the same store");
    assert_eq!(Arc::strong_count(&first), 2, "the registry holds no strong reference");
    drop(second);
    drop(first);
    let opened = SecretStore::open(path.clone()).expect("the store of a file that no one holds");
    assert_eq!(
        SecretStore::opens(&path),
        2,
        "a file whose last holder went is opened again"
    );
    drop(opened);
}

#[test]
fn many_threads_open_one_new_path_and_make_its_schema_once() {
    let root = tempfile::tempdir().expect("a directory");
    let path = home(root.path());
    let threads = 16;
    let barrier = Arc::new(Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                SecretStore::open(path).expect("the store of the one file")
            })
        })
        .collect();
    let stores: Vec<Arc<SecretStore>> = handles
        .into_iter()
        .map(|handle| handle.join().expect("the thread"))
        .collect();
    let first = &stores[0];
    assert!(
        stores.iter().all(|store| Arc::ptr_eq(first, store)),
        "every thread holds the one store of the file"
    );
    assert_eq!(SecretStore::opens(&path), 1, "the file is opened once");
    assert_eq!(
        first.ids().expect("the ids"),
        Vec::<String>::new(),
        "the schema of a new file is one file of no row"
    );
}

#[test]
fn a_credential_reference_and_a_named_secret_are_two_rows() {
    let (_root, mut store) = store();
    store.put(&credential("key"), b"one").expect("the value");
    store
        .put(&crate::named_secrets::locator(&name()), b"two")
        .expect("the value");
    assert_eq!(value_of(&store, &credential("key")).as_deref(), Some(&b"one"[..]));
    assert_eq!(
        value_of(&store, &crate::named_secrets::locator(&name())).as_deref(),
        Some(&b"two"[..])
    );
    // One reference is one row, so every provider that binds it reads one value.
    let other_origin = credential("key");
    assert_eq!(other_origin.secret_id(), "credential/key");
}

#[test]
fn a_second_write_replaces_the_value_and_a_removal_forgets_it() {
    let (_root, mut store) = store();
    let locator = credential("key");
    assert!(!store.contains(&locator).expect("the row"), "a new file holds nothing");
    store.put(&locator, b"first").expect("the first value");
    store.put(&locator, b"second").expect("the second value");
    assert_eq!(value_of(&store, &locator).as_deref(), Some(&b"second"[..]));
    store.delete(&locator).expect("the removal");
    assert!(!store.contains(&locator).expect("the row"));
    assert_eq!(value_of(&store, &locator), None);
    store.delete(&locator).expect("a removal of a missing value");
}

#[test]
fn the_file_and_its_folder_are_private_from_the_first_byte() {
    let root = tempfile::tempdir().expect("a directory");
    let folder = root.path().join("horizon-home");
    let path = folder.join(SECRETS_FILE_NAME);
    let mut store = SecretStore::open(path.clone()).expect("the store");
    let mode = |path: &Path| {
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(path).expect("the file").permissions()) & 0o777
    };
    assert_eq!(mode(&folder), 0o700, "the folder of a new file is private");
    assert_eq!(mode(&path), 0o600, "the file is private before SQLite writes to it");
    store.put(&credential("key"), b"one").expect("the value");
    assert_eq!(mode(&path), 0o600, "the file of the values is private");
    for suffix in ["-wal", "-shm"] {
        let side = Path::new(&format!("{}{suffix}", path.display())).to_path_buf();
        if side.exists() {
            assert_eq!(mode(&side), 0o600, "the {suffix} file is private");
        }
    }
}

#[test]
fn a_file_of_another_schema_is_refused_with_its_path() {
    let root = tempfile::tempdir().expect("a directory");
    let path = root.path().join(SECRETS_FILE_NAME);
    let connection = rusqlite::Connection::open(&path).expect("the file");
    connection
        .execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
             INSERT INTO meta (key, value) VALUES ('schema_version', '99');",
        )
        .expect("a file of another build");
    drop(connection);

    let error = SecretStore::open(path.clone()).expect_err("a newer schema is refused");
    let text = error.to_string();
    assert!(
        text.contains(&path.display().to_string()),
        "the text names the file: {text}"
    );
    assert!(text.contains("99"), "the text names both versions: {text}");
}

#[test]
fn a_computer_home_names_the_file_of_its_own_values() {
    let home = HorizonHome::from_root("/home/person/.horizon".into());
    assert_eq!(
        home.secrets_path(),
        std::path::PathBuf::from("/home/person/.horizon/secrets.sqlite")
    );
}

#[test]
fn a_store_in_memory_holds_a_value_and_no_file() {
    let mut store = SecretStore::in_memory().expect("the store");
    store.put(&credential("key"), b"one").expect("the value");
    assert_eq!(value_of(&store, &credential("key")).as_deref(), Some(&b"one"[..]));
    assert!(!store.path().exists());
}
