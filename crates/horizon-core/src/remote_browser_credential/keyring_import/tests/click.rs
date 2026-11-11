//! The state of one click: the line that the import answers, and the worker that
//! runs it.

use std::sync::Arc;

use horizon_browser::remote::{CredentialReference, RemoteBrowserConfig};

use super::fixtures::{read, settings, store};
use super::super::{Bound, KeyringImport, KeyringImportPlan, ProgramEnvironment};
use super::stand_in::StandIns;
use crate::remote_browser_credential::{CredentialValueChange, RemoteCredentialStore, SharedStore};

#[test]
fn an_import_that_reads_a_stand_in_reaches_the_line_of_the_button() {
    let stand_ins = StandIns::new();
    stand_ins.program(
        "secret-tool",
        "#!/bin/sh\n\
         printf '%s\\n' '[/org/freedesktop/secrets/collection/login/7]'\n\
         printf '%s\\n' 'label = keyring:a@horizon-remote-browser'\n\
         printf '%s\\n' 'secret = synthetic-token'\n\
         printf '%s\\n' 'created = '\n\
         printf '%s\\n' 'modified = '\n\
         printf '%s\\n' 'attribute.service = horizon-remote-browser' >&2\n\
         printf '%s\\n' 'attribute.username = https://grid.example.net|remote-browser/grid/key' >&2\n",
    );
    let remote = settings("https://grid.example.net/wd/hub", &["key"]);
    let plan = KeyringImportPlan::of(&remote);
    let values = store();
    let shared: SharedStore = Arc::new(std::sync::Mutex::new(
        Box::new(Arc::clone(&values)) as Box<dyn RemoteCredentialStore + Send>
    ));
    let mut import = KeyringImport::with_programs(
        stand_ins.environment(),
        Bound::WholeImport(std::time::Duration::from_secs(10)),
    );

    assert!(import.line().is_none(), "no line before a click");
    import.start(plan, shared);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while import.is_busy() && std::time::Instant::now() < deadline {
        import.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let line = import.line().expect("the answer of the click").clone();
    assert_eq!(line.text, "Imported 1 value.");
    assert!(!line.error);
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
    let stand_ins = StandIns::new();
    let remote = settings("https://grid.example.net/wd/hub", &["key"]);
    let plan = KeyringImportPlan::of(&remote);
    let values = store();
    let shared: SharedStore = Arc::new(std::sync::Mutex::new(
        Box::new(Arc::clone(&values)) as Box<dyn RemoteCredentialStore + Send>
    ));
    let mut import = KeyringImport::with_programs(
        stand_ins.environment(),
        Bound::WholeImport(std::time::Duration::from_secs(10)),
    );

    import.start(plan, shared);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while import.is_busy() && std::time::Instant::now() < deadline {
        import.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let line = import.line().expect("the answer of the click").clone();
    assert!(line.error, "a missing program is a refusal");
    assert!(line.text.contains("libsecret-tools"), "{line:?}");
    assert!(import.take_changes().is_empty());
    assert!(read(&values, "key").is_none(), "nothing is written");
}

#[test]
fn an_import_of_settings_that_name_no_reference_does_nothing() {
    let mut import = KeyringImport::with_programs(ProgramEnvironment::with_path(""), Bound::WholeImport(
        std::time::Duration::from_millis(200),
    ));
    let values = store();
    let shared: SharedStore = Arc::new(std::sync::Mutex::new(
        Box::new(Arc::clone(&values)) as Box<dyn RemoteCredentialStore + Send>
    ));

    import.start(KeyringImportPlan::of(&RemoteBrowserConfig::default()), shared);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while import.is_busy() && std::time::Instant::now() < deadline {
        import.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let line = import.line().expect("the answer of the click").clone();
    assert!(line.error && line.text.contains("name no reference"), "{line:?}");
}
