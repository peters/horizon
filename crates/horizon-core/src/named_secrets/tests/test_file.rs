//! The store of one private file that the smoke fixture uses, and the window-side
//! worker that reads the store of a person.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use super::super::{
    NamedSecretError, NamedSecretState, NamedSecretVault, NamedSecretWorkbench, SecretName, TEST_FILE_ENV,
    TestFileNamedSecretStore, locator,
};
use super::exclusive;
use crate::remote_browser_credential::{FakeCredentialStore, RemoteCredentialStore, SharedCredentialStore};

const MARKER: &str = "MARKER-2b8a-file-store-value";

fn name() -> SecretName {
    SecretName::parse("HETZNER_TOKEN").expect("a name")
}

fn held(store: &TestFileNamedSecretStore) -> Option<String> {
    let mut sink = TextSink::default();
    store.with_secret(&locator(&name()), &mut sink).ok()?;
    sink.value.take().map(|value| value.to_string())
}

#[derive(Default)]
struct TextSink {
    value: Option<Zeroizing<String>>,
}

impl crate::remote_browser_credential::Sealed for TextSink {}

impl crate::remote_browser_credential::SecretSink for TextSink {
    fn accept(&mut self, bytes: &[u8]) -> Result<(), crate::remote_browser_credential::RemoteCredentialError> {
        self.value = Some(Zeroizing::new(String::from_utf8_lossy(bytes).into_owned()));
        Ok(())
    }
}

#[test]
fn a_file_store_keeps_replaces_and_removes_the_values_of_its_file() {
    let _guard = exclusive();
    let root = tempfile::tempdir().expect("a directory");
    let path = root.path().join("named-secrets.json");
    std::fs::write(&path, format!(r#"{{"named/HETZNER_TOKEN":"{MARKER}"}}"#)).expect("the seeded file");
    let mut store = TestFileNamedSecretStore::at(path.clone()).expect("the store");
    assert_eq!(held(&store).as_deref(), Some(MARKER), "a value of the fixture is read");

    store.put(&locator(&name()), b"second").expect("the replacement");
    assert_eq!(held(&store).as_deref(), Some("second"));
    assert_eq!(store.path(), path.as_path());
    // A write of the store is a private file, as the values are in clear text.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).expect("the file").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the file of the values is private");
    }

    store.delete(&locator(&name())).expect("the removal");
    assert!(held(&store).is_none());
    store.delete(&locator(&name())).expect("a second removal");

    // A store of a file that does not exist yet holds no value.
    let empty = TestFileNamedSecretStore::at(root.path().join("absent.json")).expect("an empty store");
    assert!(held(&empty).is_none());
}

#[test]
fn the_variable_of_the_smoke_fixture_names_the_file_of_the_store() {
    assert!(
        TestFileNamedSecretStore::from_environment()
            .expect("the environment")
            .is_none(),
        "a Horizon that starts without the variable of the fixture uses the OS credential store"
    );
    let root = tempfile::tempdir().expect("a directory");
    let path = root.path().join("named-secrets.json");
    std::fs::write(&path, "{}").expect("a file of the fixture");
    let output = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", "named_secrets::tests::test_file::store_child", "--nocapture"])
        .env(TEST_FILE_ENV, &path)
        .output()
        .expect("the child test");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

/// The child of [`the_variable_of_the_smoke_fixture_names_the_file_of_the_store`]:
/// this process names the file of the fixture, so it reads that file.
#[test]
fn store_child() {
    let Some(path) = std::env::var_os(TEST_FILE_ENV) else {
        return;
    };
    let store = TestFileNamedSecretStore::from_environment()
        .expect("the environment")
        .expect("a store of the fixture");
    assert_eq!(store.path(), Path::new(&path));
    assert!(held(&store).is_none());
}

#[test]
fn the_window_reads_a_state_and_never_a_value() {
    let _guard = exclusive();
    let values = Arc::new(Mutex::new(FakeCredentialStore::new()));
    let vault = Arc::new(NamedSecretVault::new(Some(Box::new(SharedCredentialStore::new(
        Arc::clone(&values),
    )))));
    let token = name();
    vault.set_names(["HETZNER_TOKEN".to_owned()]);
    let mut bench = NamedSecretWorkbench::new(Some(Arc::clone(&vault)), std::slice::from_ref(&token));
    assert_eq!(bench.state(&token), NamedSecretState::Checking, "a read is due");
    assert_eq!(
        settle(&mut bench, &token, NamedSecretState::NoValue),
        NamedSecretState::NoValue
    );

    bench.store(token.clone(), Zeroizing::new(MARKER.as_bytes().to_vec()));
    assert_eq!(
        settle(&mut bench, &token, NamedSecretState::Present),
        NamedSecretState::Present
    );
    let changes = bench.take_changes();
    assert_eq!(changes.len(), 1);
    assert!(changes[0].stored);
    assert_eq!(changes[0].name, token);
    assert_eq!(super::super::published_names(), vec![token.clone()]);
    assert!(bench.failure().is_none());

    bench.delete(token.clone());
    assert_eq!(
        settle(&mut bench, &token, NamedSecretState::NoValue),
        NamedSecretState::NoValue
    );
    let changes = bench.take_changes();
    assert_eq!(changes.len(), 1);
    assert!(!changes[0].stored);
    assert!(bench.take_changes().is_empty(), "a change is queued once");
}

#[test]
fn a_locked_store_shows_its_state_and_never_a_value() {
    let _guard = exclusive();
    let store = FakeCredentialStore::new();
    let values = Arc::new(Mutex::new(store));
    values.lock().expect("the store").lock();
    let vault = Arc::new(NamedSecretVault::new(Some(Box::new(SharedCredentialStore::new(
        Arc::clone(&values),
    )))));
    let token = name();
    vault.set_names(["HETZNER_TOKEN".to_owned()]);
    let mut bench = NamedSecretWorkbench::new(Some(vault), std::slice::from_ref(&token));
    assert_eq!(
        settle(&mut bench, &token, NamedSecretState::Locked),
        NamedSecretState::Locked
    );
    assert!(matches!(bench.failure(), Some(NamedSecretError::Locked)));
    assert_eq!(bench.take_failure(), Some(NamedSecretError::Locked));
    assert!(bench.failure().is_none(), "the line is shown once");
}

#[test]
fn a_window_whose_vault_is_not_ready_yet_answers_checking_and_no_failure() {
    let _guard = exclusive();
    let token = name();
    let mut bench = NamedSecretWorkbench::new(None, std::slice::from_ref(&token));
    bench.poll();
    assert_eq!(bench.state(&token), NamedSecretState::Checking);
    assert!(
        bench.take_failure().is_none(),
        "a store that has not answered is no failure"
    );
}

#[test]
fn a_computer_without_a_credential_store_says_so_for_every_name() {
    let _guard = exclusive();
    let token = name();
    let vault = Arc::new(NamedSecretVault::new(None));
    vault.set_names(["HETZNER_TOKEN".to_owned()]);
    let mut bench = NamedSecretWorkbench::new(Some(vault), std::slice::from_ref(&token));
    assert_eq!(
        settle(&mut bench, &token, NamedSecretState::StoreUnavailable),
        NamedSecretState::StoreUnavailable
    );
    assert_eq!(bench.take_failure(), Some(NamedSecretError::StoreUnavailable));
    assert!(
        super::super::published_names().is_empty(),
        "no panel of this computer gets a value"
    );
}

/// Polls the workbench until `name` reaches `wanted`, and answers the state that it
/// reached.
fn settle(bench: &mut NamedSecretWorkbench, name: &SecretName, wanted: NamedSecretState) -> NamedSecretState {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        bench.poll();
        let state = bench.state(name);
        if state == wanted || Instant::now() >= deadline {
            return state;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
