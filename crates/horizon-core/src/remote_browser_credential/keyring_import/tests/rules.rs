//! The rules of one import: which row it writes, which row it leaves alone, and
//! what its line says.

use horizon_browser::remote::{CredentialReference, RemoteBrowserConfig};

use super::super::run::import;
use super::super::{KeyringImportError, KeyringImportPlan, KeyringImportReport};
use super::fixtures::{account, entry, read, settings, store, write};
use crate::remote_browser_credential::CredentialValueChange;

#[test]
fn an_entry_writes_the_row_of_a_reference_of_the_settings() {
    let remote = settings("https://grid.example.net/wd/hub", &["key"]);
    let plan = KeyringImportPlan::of(&remote);
    let mut store = store();

    let outcome = import(
        vec![entry(&account("https://grid.example.net/wd/hub"), "synthetic-token")],
        &plan,
        &mut store,
    )
    .expect("one import");

    assert_eq!(
        outcome.report,
        KeyringImportReport {
            imported: 1,
            already_here: 0,
            unused: 0
        }
    );
    assert_eq!(read(&store, "key").as_deref(), Some("synthetic-token"));
    assert_eq!(
        outcome.changes,
        vec![CredentialValueChange {
            reference: CredentialReference::from("key"),
            stored: true
        }],
        "the link to a server learns the value like a value that a person typed"
    );
}

#[test]
fn a_row_that_is_there_is_not_written_over() {
    let remote = settings("https://grid.example.net/wd/hub", &["key"]);
    let plan = KeyringImportPlan::of(&remote);
    let mut store = store();
    write(&store, "key", "the-value-that-the-person-typed");

    let outcome = import(
        vec![entry(&account("https://grid.example.net/wd/hub"), "the-old-value")],
        &plan,
        &mut store,
    )
    .expect("one import");

    assert_eq!(outcome.report.imported, 0);
    assert_eq!(outcome.report.already_here, 1);
    assert!(
        outcome.changes.is_empty(),
        "nothing changed, so the link learns nothing"
    );
    assert_eq!(
        read(&store, "key").as_deref(),
        Some("the-value-that-the-person-typed"),
        "an import never replaces the value of this computer"
    );
}

#[test]
fn an_entry_that_names_no_reference_is_counted_as_not_used() {
    let remote = settings("https://grid.example.net/wd/hub", &["key"]);
    let plan = KeyringImportPlan::of(&remote);
    let mut store = store();

    let outcome = import(
        vec![
            entry("https://elsewhere.example.net|remote-browser/other/key", "a-value"),
            entry(&account("https://grid.example.net/wd/hub"), "synthetic-token"),
        ],
        &plan,
        &mut store,
    )
    .expect("one import");

    assert_eq!(outcome.report.imported, 1);
    assert_eq!(outcome.report.unused, 1);
    assert_eq!(read(&store, "key").as_deref(), Some("synthetic-token"));
}

#[test]
fn two_references_that_share_a_slot_take_the_value() {
    let remote = settings("https://grid.example.net/wd/hub", &["key", "token"]);
    let plan = KeyringImportPlan::of(&remote);
    let mut store = store();

    let outcome = import(
        vec![entry(&account("https://grid.example.net/wd/hub"), "synthetic-token")],
        &plan,
        &mut store,
    )
    .expect("one import");

    assert_eq!(
        outcome.report.imported, 2,
        "one entry names the one item of both references"
    );
    assert_eq!(read(&store, "key").as_deref(), Some("synthetic-token"));
}

#[test]
fn a_reference_that_breaks_the_rule_is_refused_and_no_row_is_written() {
    let remote = settings("https://grid.example.net/wd/hub", &["key", "bad reference!"]);
    let plan = KeyringImportPlan::of(&remote);
    let mut store = store();

    let error = import(
        vec![entry(&account("https://grid.example.net/wd/hub"), "synthetic-token")],
        &plan,
        &mut store,
    )
    .expect_err("a refusal");

    assert_eq!(
        error,
        KeyringImportError::RefusedReference {
            reference: "bad reference!".to_string()
        }
    );
    assert!(
        read(&store, "key").is_none(),
        "a refusal writes no row, not even the row of the reference beside it"
    );
}

#[test]
fn an_empty_or_oversized_value_is_refused_by_the_rule_of_the_file() {
    let remote = settings("https://grid.example.net/wd/hub", &["key"]);
    let plan = KeyringImportPlan::of(&remote);
    let mut store = store();

    assert_eq!(
        import(
            vec![entry(&account("https://grid.example.net/wd/hub"), "")],
            &plan,
            &mut store
        )
        .err(),
        Some(KeyringImportError::RefusedValue)
    );
    let oversized = "a".repeat(crate::remote_browser_credential::MAX_SECRET_BYTES + 1);
    assert_eq!(
        import(
            vec![entry(&account("https://grid.example.net/wd/hub"), &oversized)],
            &plan,
            &mut store
        )
        .err(),
        Some(KeyringImportError::RefusedValue)
    );
    assert!(read(&store, "key").is_none());
}

#[test]
fn the_line_counts_the_values_that_were_imported() {
    assert_eq!(
        KeyringImportReport {
            imported: 2,
            already_here: 1,
            unused: 3
        }
        .to_string(),
        "Imported 2 values. 1 was already here. 3 entries of the keyring are not used by this version."
    );
    assert_eq!(
        KeyringImportReport {
            imported: 1,
            already_here: 0,
            unused: 0
        }
        .to_string(),
        "Imported 1 value."
    );
    assert_eq!(
        KeyringImportReport {
            imported: 0,
            already_here: 2,
            unused: 1
        }
        .to_string(),
        "Imported 0 values. 2 were already here. 1 entry of the keyring is not used by this version."
    );
}

#[test]
fn no_value_reaches_a_debug_or_a_display_of_a_result_or_an_error() {
    let value = "the-secret-value-of-4f1c";
    let remote = settings("https://grid.example.net/wd/hub", &["key"]);
    let plan = KeyringImportPlan::of(&remote);
    let mut store = store();
    let entry = entry(&account("https://grid.example.net/wd/hub"), value);
    assert!(!format!("{entry:?}").contains(value), "an entry redacts its value");

    let outcome = import(vec![entry], &plan, &mut store).expect("one import");
    assert!(!format!("{outcome:?}").contains(value));
    assert!(
        !format!("{} {:?}", outcome.report, outcome.report).contains(value),
        "the line of the import holds counts and no value"
    );
    assert!(!format!("{:?}", outcome.changes).contains(value));

    let errors = [
        KeyringImportError::Unreadable,
        KeyringImportError::RefusedValue,
        KeyringImportError::Store(crate::remote_browser_credential::RemoteCredentialError::Missing),
        KeyringImportError::ProgramFailed {
            program: "secret-tool",
            code: 1,
        },
    ];
    for error in errors {
        let said = format!("{error} {error:?}");
        assert!(!said.contains(value), "an error holds no value: {said}");
    }
}

#[test]
fn the_plan_names_the_account_of_the_older_horizon() {
    let plan = KeyringImportPlan::of(&settings("https://grid.example.net/wd/hub", &["key"]));

    assert_eq!(
        plan.users(),
        vec!["https://grid.example.net|remote-browser/grid/key"],
        "the account is the origin and the slot, as the older version built it"
    );
    assert_eq!(
        plan.locators()
            .map(|locator| locator.reference.as_str())
            .collect::<Vec<_>>(),
        vec!["key"]
    );
    assert!(!plan.is_empty());
    assert!(KeyringImportPlan::of(&RemoteBrowserConfig::default()).is_empty());
}
