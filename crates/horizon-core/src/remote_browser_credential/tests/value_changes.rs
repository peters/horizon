//! The change list that a link to a server reads: one entry for each value of the file
//! that this computer stored or removed, with the reference and never the value.

use super::{CredentialValueChange, SecretsState, basic_profile, memory_opener, wait_until};
use crate::remote_browser_credential::{
    CredentialReference, CredentialState, CredentialStoreKind, CredentialWorkbench, KeyringImportPlan,
};

/// A link to a server holds the same credential values as this computer, and it
/// learns about each change from the workbench: the reference and whether the value
/// was stored or removed, never the value.
#[test]
fn a_credential_change_reaches_the_link_once_and_without_its_value() {
    let profile = basic_profile(CredentialStoreKind::File, CredentialStoreKind::Session);
    let mut workbench = CredentialWorkbench::with_opener(memory_opener());
    wait_until(&mut workbench, |w| w.secrets_state() == &SecretsState::Ready);
    let user = CredentialReference::from("user");
    assert!(workbench.take_value_changes().is_empty(), "nothing changed yet");

    workbench
        .store_value("grid", &profile, &user, b"alice")
        .expect("store request accepted");
    wait_until(&mut workbench, |w| {
        w.readiness(&profile)[0].state == CredentialState::Present
    });
    let changes = workbench.take_value_changes();
    assert_eq!(
        changes,
        vec![CredentialValueChange {
            reference: user.clone(),
            stored: true,
        }],
        "the link learns that the value of this reference changed"
    );
    assert!(!format!("{changes:?}").contains("alice"), "a change carries no value");
    assert!(workbench.take_value_changes().is_empty(), "one change is reported once");

    workbench
        .delete("grid", &profile, &user)
        .expect("delete request accepted");
    wait_until(&mut workbench, |w| {
        w.readiness(&profile)[0].state == CredentialState::Missing
    });
    assert_eq!(
        workbench.take_value_changes(),
        vec![CredentialValueChange {
            reference: user,
            stored: false,
        }],
        "a deleted value is one change too"
    );
}

/// A value that the import of the keyring wrote reaches the link to a server exactly
/// as a value that a person typed: the reference and that it was stored, never the
/// value.
#[test]
fn an_imported_value_reaches_the_link_like_a_typed_one() {
    let mut workbench = CredentialWorkbench::with_opener(memory_opener());
    wait_until(&mut workbench, |w| w.secrets_state() == &SecretsState::Ready);
    let user = CredentialReference::from("user");
    assert!(workbench.take_value_changes().is_empty(), "nothing changed yet");

    workbench.adopt_imported_values(vec![CredentialValueChange {
        reference: user.clone(),
        stored: true,
    }]);

    let changes = workbench.take_value_changes();
    assert_eq!(
        changes,
        vec![CredentialValueChange {
            reference: user.clone(),
            stored: true,
        }],
        "the link learns about the reference that the import wrote"
    );
    assert!(!format!("{changes:?}").contains("alice"), "a change carries no value");
    assert!(workbench.take_value_changes().is_empty(), "one change is reported once");
    workbench.adopt_imported_values(Vec::new());
    assert!(
        workbench.take_value_changes().is_empty(),
        "an import of nothing changes nothing"
    );
}

/// The Settings tab offers the import of the keyring while a reference of the
/// settings has no value in the secrets file, and it stops offering it when the
/// value is there.
#[test]
fn the_import_is_offered_while_a_reference_of_the_file_has_no_value() {
    let profile = basic_profile(CredentialStoreKind::File, CredentialStoreKind::File);
    let mut remote = crate::browser::remote::RemoteBrowserConfig::default();
    remote.providers.insert("grid".to_string(), profile.clone());
    let plan = KeyringImportPlan::of(&remote);
    assert!(!plan.is_empty(), "the settings name a reference of the file");

    let mut workbench = CredentialWorkbench::with_opener(memory_opener());
    wait_until(&mut workbench, |w| w.secrets_state() == &SecretsState::Ready);
    assert!(
        !workbench.any_reference_without_value(&plan),
        "a presence that is still being read offers no import"
    );
    wait_until(&mut workbench, |w| {
        w.readiness(&profile)
            .iter()
            .all(|entry| entry.state == CredentialState::Missing)
    });
    assert!(
        workbench.any_reference_without_value(&plan),
        "no value on this computer offers the import"
    );

    workbench
        .store_value("grid", &profile, &CredentialReference::from("user"), b"alice")
        .expect("a value for one of the two references");
    wait_until(&mut workbench, |w| {
        w.readiness(&profile)[0].state == CredentialState::Present
    });
    assert!(
        workbench.any_reference_without_value(&plan),
        "the other reference of the file still has no value"
    );

    workbench
        .store_value("grid", &profile, &CredentialReference::from("key"), b"token")
        .expect("a value for the second reference");
    wait_until(&mut workbench, |w| {
        w.readiness(&profile)
            .iter()
            .all(|entry| entry.state == CredentialState::Present)
    });
    assert!(
        !workbench.any_reference_without_value(&plan),
        "two values on this computer offer no import"
    );
}
