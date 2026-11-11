//! The change list that a link to a server reads: one entry for each OS-store value
//! that this computer stored or removed, with the reference and never the value.

use super::{CredentialValueChange, KeychainState, basic_profile, fake_opener, wait_until};
use crate::remote_browser_credential::{
    CredentialReference, CredentialState, CredentialStoreKind, CredentialWorkbench,
};

/// A link to a server holds the same credential values as this computer, and it
/// learns about each change from the workbench: the reference and whether the value
/// was stored or removed, never the value.
#[test]
fn a_credential_change_reaches_the_link_once_and_without_its_value() {
    let profile = basic_profile(CredentialStoreKind::OsKeychain, CredentialStoreKind::Session);
    let mut workbench = CredentialWorkbench::with_opener(fake_opener(false));
    wait_until(&mut workbench, |w| w.keychain_state() == &KeychainState::Available);
    let user = CredentialReference::from("user");
    assert!(workbench.take_value_changes().is_empty(), "nothing changed yet");

    workbench
        .store_in_keychain("grid", &profile, &user, b"alice")
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
