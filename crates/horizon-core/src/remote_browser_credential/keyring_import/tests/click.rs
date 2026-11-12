//! The state of one click: the line that the import answers, and the worker that
//! runs it.

use std::sync::Arc;

use horizon_browser::remote::{CredentialReference, RemoteBrowserConfig};

use super::super::{Bound, KeyringImport, KeyringImportPlan, Platform, ProgramEnvironment};
use super::fixtures::{account, read, settings, store};
use super::stand_in::{self, StandIns};
use crate::remote_browser_credential::{CredentialValueChange, RemoteCredentialStore, SharedStore};

/// The endpoint of the provider of these tests.
const ENDPOINT: &str = "https://grid.example.net/wd/hub";

fn bound() -> Bound {
    Bound::WholeImport(std::time::Duration::from_secs(10))
}

/// The worker of one import, until its answer is on the line of the button.
fn answered(import: &mut KeyringImport) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while import.is_busy() && std::time::Instant::now() < deadline {
        import.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn an_import_that_reads_a_stand_in_reaches_the_line_of_the_button() {
    for platform in [Platform::SecretService, Platform::Keychain] {
        an_import_that_reads_a_stand_in(platform);
    }
}

/// The click path of one keyring, so the reading of either system is tested on the
/// computer that runs the tests.
fn an_import_that_reads_a_stand_in(platform: Platform) {
    let stand_ins = StandIns::new();
    stand_in::keyring(&stand_ins, platform, &account(ENDPOINT), "synthetic-token");
    let remote = settings(ENDPOINT, &["key"]);
    let plan = KeyringImportPlan::of(&remote);
    let values = store();
    let shared: SharedStore = Arc::new(std::sync::Mutex::new(
        Box::new(Arc::clone(&values)) as Box<dyn RemoteCredentialStore + Send>
    ));
    let mut import = KeyringImport::with_programs(platform, stand_ins.environment(), bound());

    assert!(import.line().is_none(), "no line before a click");
    import.start(plan, shared, None);
    answered(&mut import);

    let line = import.line().expect("the answer of the click").clone();
    assert_eq!(line.text, "Imported 1 value.", "{platform:?}");
    assert!(!line.error, "{platform:?}");
    assert_eq!(
        import.take_changes(),
        vec![CredentialValueChange {
            reference: CredentialReference::from("key"),
            stored: true
        }]
    );
    assert_eq!(read(&values, "key").as_deref(), Some("synthetic-token"));
    assert!(!line.text.contains("synthetic-token"), "the line holds no value");
}

#[test]
fn an_import_without_a_program_leaves_a_refusal_and_no_value() {
    for platform in [Platform::SecretService, Platform::Keychain] {
        an_import_without_a_program(platform);
    }
}

/// The refusal of one keyring whose program is missing, in the words of that system:
/// macOS must not offer the package of Linux.
fn an_import_without_a_program(platform: Platform) {
    let stand_ins = StandIns::new();
    let remote = settings(ENDPOINT, &["key"]);
    let plan = KeyringImportPlan::of(&remote);
    let values = store();
    let shared: SharedStore = Arc::new(std::sync::Mutex::new(
        Box::new(Arc::clone(&values)) as Box<dyn RemoteCredentialStore + Send>
    ));
    let mut import = KeyringImport::with_programs(platform, stand_ins.environment(), bound());

    import.start(plan, shared, None);
    answered(&mut import);

    let line = import.line().expect("the answer of the click").clone();
    assert!(line.error, "a missing program is a refusal");
    assert_eq!(line.text, platform.missing_program(), "{platform:?}");
    assert!(import.take_changes().is_empty());
    assert!(read(&values, "key").is_none(), "nothing is written");
}

#[test]
fn an_import_of_settings_that_name_no_reference_does_nothing() {
    let mut import = KeyringImport::with_programs(
        Platform::of_this_system(),
        ProgramEnvironment::with_path(""),
        Bound::WholeImport(std::time::Duration::from_millis(200)),
    );
    let values = store();
    let shared: SharedStore = Arc::new(std::sync::Mutex::new(
        Box::new(Arc::clone(&values)) as Box<dyn RemoteCredentialStore + Send>
    ));

    import.start(KeyringImportPlan::of(&RemoteBrowserConfig::default()), shared, None);
    answered(&mut import);

    let line = import.line().expect("the answer of the click").clone();
    assert!(line.error && line.text.contains("name no reference"), "{line:?}");
}

/// The import that a start builds reads the keyring of the system that runs it.
#[test]
fn the_import_of_a_start_reads_the_keyring_of_this_system() {
    assert_eq!(KeyringImport::new().platform(), Platform::of_this_system());
}

/// The bound of one entry of either keyring stayed with that keyring when the choice
/// of the system became a value: the Keychain may ask the person for each entry.
#[test]
fn each_keyring_keeps_the_bound_that_was_made_for_it() {
    assert_eq!(
        Platform::SecretService.bound(),
        Bound::WholeImport(std::time::Duration::from_secs(20)),
        "the Secret Service answers every entry within one process"
    );
    assert_eq!(
        Platform::Keychain.bound(),
        Bound::EachEntry(std::time::Duration::from_secs(60)),
        "the Keychain may ask the person once for each entry"
    );
}
