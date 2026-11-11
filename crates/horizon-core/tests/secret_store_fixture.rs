//! The secrets file that the smoke fixture writes, read by the store of this crate.
//!
//! `scripts/device-smoke/serve.py` seeds the private home of one isolated desktop
//! with Python's `sqlite3`, because that desktop has no other way to hold a value
//! before Horizon starts. Both sides run `crates/horizon-core/src/secret_store/schema.sql`,
//! and this test proves that a file of the fixture is a file that Horizon reads: a
//! schema that drifts refuses the file here.

use std::path::Path;

use horizon_core::secret_store::{SECRETS_FILE_NAME, SecretStore};

#[test]
fn a_file_that_the_smoke_fixture_writes_is_a_file_that_this_store_reads() {
    let home = tempfile::tempdir().expect("a directory");
    let path = home.path().join(SECRETS_FILE_NAME);
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/device-smoke");
    let script = format!(
        "import sys\nsys.path.insert(0, {fixture:?})\nimport serve\nserve.seed_secret_rows({path:?}, [('TOKEN', 'from-the-fixture')], [('credential/fixture', 'a-fixture-value')])\n"
    );
    let ran = std::process::Command::new("python3")
        .arg("-B")
        .arg("-c")
        .arg(&script)
        .output();
    let Ok(output) = ran else {
        eprintln!("python3 is not here, so the fixture file of this test cannot be made");
        return;
    };
    assert!(
        output.status.success(),
        "the fixture wrote its file: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let store = SecretStore::open(path).expect("the file of the fixture");
    assert_eq!(
        store
            .read("named/TOKEN")
            .expect("the row")
            .as_deref()
            .map(Vec::as_slice),
        Some(&b"from-the-fixture"[..]),
        "a named secret of the fixture is a value of this store"
    );
    assert_eq!(
        store
            .read("credential/fixture")
            .expect("the row")
            .as_deref()
            .map(Vec::as_slice),
        Some(&b"a-fixture-value"[..]),
        "a credential value of the fixture is a value of this store"
    );
    assert_eq!(
        store.ids().expect("the ids"),
        vec!["credential/fixture".to_owned(), "named/TOKEN".to_owned()]
    );
}
