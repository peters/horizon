//! The server store, judged on a temporary file: idempotence, the index, the
//! version rule, the limits and the time of the server.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use horizon_core::Machine;
use horizon_sync::{
    Changes, Intent, KIND_MACHINE, KIND_PROBE, KIND_SETTING, MAX_PROBES, MAX_SETTINGS, Op, Outcome, Refusal, Store,
    StoreError, tip,
};
use serde_json::{Value, json};

/// A store in a temporary directory, with its directory kept alive.
fn store() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let store = Store::open(&temp.path().join("server.sqlite")).expect("a new store");
    (temp, store)
}

fn machine(name: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: "203.0.113.10".to_owned(),
        user: Some("root".to_owned()),
        port: Some(2222),
        session: Some("work".to_owned()),
        provider: Some("Hetzner, hel1".to_owned()),
        repository: Some("example-org/shop".to_owned()),
        added_at_millis: 7,
    }
}

fn put_machine(device: &str, id: u64, name: &str) -> Intent {
    Intent {
        device: device.to_owned(),
        id,
        seen: None,
        op: Op::MachinePut { record: machine(name) },
    }
}

fn remove_machine(device: &str, id: u64, name: &str) -> Intent {
    Intent {
        device: device.to_owned(),
        id,
        seen: None,
        op: Op::MachineRemove { name: name.to_owned() },
    }
}

fn put_probe(device: &str, id: u64, seen: Option<u64>, probe: &str, body: Value) -> Intent {
    Intent {
        device: device.to_owned(),
        id,
        seen,
        op: Op::ObjectPut {
            kind: KIND_PROBE.to_owned(),
            id: probe.to_owned(),
            body,
        },
    }
}

fn put_setting(device: &str, id: u64, seen: Option<u64>, section: &str, body: Value) -> Intent {
    Intent {
        device: device.to_owned(),
        id,
        seen,
        op: Op::SettingPut {
            id: section.to_owned(),
            body,
        },
    }
}

/// The body of one section of the settings, as one computer sends it.
fn section_body(theme: &str) -> Value {
    json!({ "format": 1, "config_version": 12, "section": { "theme": theme } })
}

/// The index of the one applied result of a push of one intent.
fn applied(store: &Store, device: &str, intent: Intent) -> u64 {
    match store.push(device, &[intent]).expect("the push").results[0]
        .outcome
        .clone()
    {
        Outcome::Applied { index } => index,
        refused @ Outcome::Refused(_) => panic!("the intent was not applied: {refused:?}"),
    }
}

/// The refusal of the one result of a push of one intent.
fn refused(store: &Store, device: &str, intent: Intent) -> Refusal {
    match store.push(device, &[intent]).expect("the push").results[0]
        .outcome
        .clone()
    {
        Outcome::Refused(refusal) => refusal,
        applied @ Outcome::Applied { .. } => panic!("the intent was applied: {applied:?}"),
    }
}

#[test]
fn a_resend_changes_nothing_and_returns_the_first_result() {
    let (_temp, store) = store();
    let first = store
        .push("mbp-7c1", &[put_machine("mbp-7c1", 1, "build-1")])
        .expect("the first push");
    let again = store
        .push("mbp-7c1", &[put_machine("mbp-7c1", 1, "build-1")])
        .expect("the resend");
    assert_eq!(again.results, first.results, "a resend answers the first result");
    assert_eq!(store.index(), 1, "a resend takes no index");
    let changes = store.all().expect("the objects");
    assert_eq!(changes.rows.len(), 1, "a resend writes no second object");
    assert_eq!(again.next_expected, 2, "the next counter of this computer");
}

#[test]
fn the_first_intent_on_a_version_wins_and_a_later_one_is_refused() {
    let (_temp, store) = store();
    let created = applied(
        &store,
        "mbp-7c1",
        put_probe("mbp-7c1", 1, None, "p1", json!({ "n": 1 })),
    );

    let second = refused(
        &store,
        "other-2a",
        put_probe("other-2a", 1, None, "p1", json!({ "n": 2 })),
    );
    assert_eq!(
        second,
        Refusal::AlreadyAnswered { version: created },
        "a create on an object that is there is already answered"
    );

    let stale = refused(
        &store,
        "other-2a",
        put_probe("other-2a", 2, Some(created - 1), "p1", json!({ "n": 3 })),
    );
    assert_eq!(
        stale,
        Refusal::ChangedSinceSeen {
            seen: created - 1,
            version: created
        },
        "a version that is not the one the object has is refused with both numbers"
    );

    let updated = applied(
        &store,
        "other-2a",
        put_probe("other-2a", 3, Some(created), "p1", json!({ "n": 4 })),
    );
    assert!(updated > created);
    let late = refused(
        &store,
        "mbp-7c1",
        put_probe("mbp-7c1", 2, Some(created), "p1", json!({ "n": 5 })),
    );
    assert_eq!(
        late,
        Refusal::ChangedSinceSeen {
            seen: created,
            version: updated
        },
        "the second intent on one version loses to the first"
    );
    let missing = refused(
        &store,
        "mbp-7c1",
        put_probe("mbp-7c1", 3, Some(1), "p2", json!({ "n": 1 })),
    );
    assert_eq!(
        missing,
        Refusal::ChangedSinceSeen { seen: 1, version: 0 },
        "an object that is not there has version 0"
    );
}

#[test]
fn every_decided_intent_takes_the_next_index_so_the_log_has_no_gap() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let path = temp.path().join("server.sqlite");
    let store = Store::open(&path).expect("the store");
    let mut broken = machine("build-2");
    broken.host = "-oProxyCommand=id".to_owned();
    let push = store
        .push(
            "mbp-7c1",
            &[
                Intent {
                    device: "mbp-7c1".to_owned(),
                    id: 1,
                    seen: None,
                    op: Op::MachinePut { record: broken },
                },
                put_machine("mbp-7c1", 2, "build-1"),
                remove_machine("mbp-7c1", 3, "build-9"),
            ],
        )
        .expect("the push");
    assert!(
        matches!(push.results[0].outcome, Outcome::Refused(_)),
        "a record that breaks a rule is refused"
    );
    assert!(matches!(push.results[1].outcome, Outcome::Applied { index: 2 }));
    assert!(matches!(push.results[2].outcome, Outcome::Applied { index: 3 }));
    assert_eq!(store.index(), 3, "a refusal takes an index too, and none is skipped");

    store.close();
    let connection = rusqlite::Connection::open(&path).expect("the file");
    let indexes: Vec<i64> = connection
        .prepare("SELECT idx FROM log ORDER BY idx")
        .expect("the log")
        .query_map([], |row| row.get(0))
        .expect("the rows")
        .collect::<Result<_, _>>()
        .expect("the indexes");
    assert_eq!(indexes, vec![1, 2, 3], "the log is one run of indexes from 1");
    let decided: i64 = connection
        .query_row("SELECT COUNT(*) FROM applied", [], |row| row.get(0))
        .expect("the applied identifiers");
    assert_eq!(decided, 3, "every decided intent is remembered");
}

#[test]
fn the_time_of_a_machine_is_the_time_of_the_server_and_it_stays() {
    let (_temp, store) = store();
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock")
        .as_millis();
    // A computer that sends 1 as the time of its record: the clock of a computer
    // never reaches the list of another computer.
    let mut sent = machine("build-1");
    sent.added_at_millis = 1;
    applied(
        &store,
        "mbp-7c1",
        Intent {
            device: "mbp-7c1".to_owned(),
            id: 1,
            seen: None,
            op: Op::MachinePut { record: sent.clone() },
        },
    );
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock")
        .as_millis();
    let stored = store
        .all()
        .expect("the objects")
        .rows
        .into_iter()
        .find(|row| row.kind == KIND_MACHINE && row.id == "build-1")
        .expect("the row of the machine");
    let stamped = stored
        .body
        .get("added_at_millis")
        .and_then(Value::as_u64)
        .expect("a time");
    assert!(
        stamped >= u64::try_from(before).expect("one time"),
        "the server stamps the record with its own clock"
    );
    assert!(
        stamped <= u64::try_from(after).expect("one time"),
        "the stamp is not after the push"
    );

    // A later change of the same machine, from another computer and with another
    // time, keeps the time of the first store.
    let mut changed = sent;
    changed.provider = Some("Hetzner, hel1".to_owned());
    changed.added_at_millis = 99;
    applied(
        &store,
        "other-2a",
        Intent {
            device: "other-2a".to_owned(),
            id: 1,
            seen: None,
            op: Op::MachinePut { record: changed },
        },
    );
    let again = store
        .all()
        .expect("the objects")
        .rows
        .into_iter()
        .find(|row| row.kind == KIND_MACHINE && row.id == "build-1")
        .expect("the row of the machine");
    assert_eq!(
        again.body.get("added_at_millis").and_then(Value::as_u64),
        Some(stamped),
        "a change keeps the time that the server stamped"
    );
    assert_eq!(
        again.body.get("provider").and_then(Value::as_str),
        Some("Hetzner, hel1"),
        "the rest of the record is the change"
    );
}

#[test]
fn ten_computers_push_at_once_and_the_log_stays_one_run() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let path = temp.path().join("server.sqlite");
    let store = Store::open(&path).expect("the store");
    let started = Instant::now();
    let mut threads = Vec::new();
    for computer in 0..10 {
        let store = store.clone();
        threads.push(std::thread::spawn(move || {
            let device = format!("computer-{computer}");
            for round in 0..25 {
                let id = round + 1;
                let push = store
                    .push(&device, &[put_machine(&device, id, &format!("build-{computer}"))])
                    .expect("no push waits for a lock or fails");
                assert!(
                    matches!(push.results[0].outcome, Outcome::Applied { .. }),
                    "the machine of {} at round {round} was refused: {:?}",
                    device,
                    push.results[0].outcome
                );
            }
        }));
    }
    for thread in threads {
        thread.join().expect("the computer ended");
    }
    let elapsed = started.elapsed();
    assert_eq!(store.index(), 250, "ten computers write one run of indexes");
    assert_eq!(
        store.all().expect("the objects").rows.len(),
        10,
        "each computer kept one machine, and each write replaced it"
    );
    println!(
        "10 computers, 250 intents, {:.3} s, no push waited for a lock",
        elapsed.as_secs_f64()
    );
    store.close();

    let connection = rusqlite::Connection::open(&path).expect("the file");
    let indexes: Vec<i64> = connection
        .prepare("SELECT idx FROM log ORDER BY idx")
        .expect("the log")
        .query_map([], |row| row.get(0))
        .expect("the rows")
        .collect::<Result<_, _>>()
        .expect("the indexes");
    assert_eq!(indexes, (1..=250).collect::<Vec<i64>>(), "log.idx is one run from 1");
}

#[test]
fn the_server_stamps_its_own_time_and_the_server_file_names_itself() {
    let (_temp, store) = store();
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock")
        .as_millis();
    applied(&store, "mbp-7c1", put_machine("mbp-7c1", 1, "build-1"));
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock")
        .as_millis();
    let path = store.path().to_path_buf();
    let store_id = store.store_id().to_owned();
    store.close();

    let connection = rusqlite::Connection::open(&path).expect("the file");
    let (at_millis, device): (i64, String) = connection
        .query_row("SELECT at_millis, device FROM log WHERE idx = 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("the log row");
    assert!(
        at_millis >= i64::try_from(before).expect("one time"),
        "the stamp is not before the push"
    );
    assert!(
        at_millis <= i64::try_from(after).expect("one time"),
        "the stamp is not after the push"
    );
    assert_eq!(device, "mbp-7c1");
    let last_seen: i64 = connection
        .query_row("SELECT last_seen FROM devices WHERE name = 'mbp-7c1'", [], |row| {
            row.get(0)
        })
        .expect("the computer");
    assert_eq!(last_seen, at_millis, "the computer is stamped by the same clock");

    let reopened = Store::open(&path).expect("the same store");
    assert_eq!(reopened.store_id(), store_id, "the store id survives a restart");
    assert_eq!(reopened.index(), 1, "the log survives a restart");
    assert_eq!(
        tip(reopened.store_id(), reopened.index()),
        tip(&store_id, 1),
        "both sides name the same log"
    );
    reopened.close();

    let other = tempfile::tempdir().expect("another directory");
    let other = Store::open(&other.path().join("server.sqlite")).expect("another store");
    assert_ne!(other.store_id(), store_id, "two files are two stores");
    other.close();
}

#[test]
fn a_wait_answers_at_its_bound_and_wakes_on_a_change() {
    let (_temp, store) = store();
    let started = Instant::now();
    let empty = store
        .wait(0, Duration::from_millis(150))
        .expect("the wait at its bound");
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(140),
        "the wait held for its bound, {waited:?}"
    );
    assert!(waiting_is_empty(&empty));

    let other = store.clone();
    let waiting = std::thread::spawn(move || other.wait(0, Duration::from_secs(50)));
    std::thread::sleep(Duration::from_millis(50));
    applied(&store, "mbp-7c1", put_machine("mbp-7c1", 1, "build-1"));
    let woken = waiting.join().expect("the waiter ended").expect("the wait answered");
    assert_eq!(woken.rows.len(), 1, "the wait woke on the change of another computer");
    assert_eq!(woken.rows[0].id, "build-1");
    assert_eq!(woken.index, 1, "and it names the index it saw");
}

fn waiting_is_empty(changes: &Changes) -> bool {
    changes.rows.is_empty() && !changes.reset
}

#[test]
fn the_machine_list_has_its_limit_and_last_write_wins() {
    let (_temp, store) = store();
    for index in 0..horizon_core::MAX_MACHINES {
        let id = u64::try_from(index).expect("one counter") + 1;
        applied(&store, "mbp-7c1", put_machine("mbp-7c1", id, &format!("build-{index}")));
    }
    let refused = refused(&store, "mbp-7c1", put_machine("mbp-7c1", 100, "build-last"));
    assert_eq!(
        refused,
        Refusal::TooMany {
            kind: KIND_MACHINE.to_owned(),
            limit: 64
        }
    );
    let replaced = applied(&store, "other-2a", put_machine("other-2a", 1, "build-3"));
    assert!(replaced > 64, "a machine that is there is replaced, not counted");

    let stale = Intent {
        seen: Some(999),
        ..put_machine("other-2a", 2, "build-3")
    };
    assert!(
        matches!(
            store.push("other-2a", &[stale]).expect("the push").results[0].outcome,
            Outcome::Applied { .. }
        ),
        "the machine list is last write wins, so a seen version decides nothing"
    );
}

#[test]
fn a_removed_machine_leaves_a_row_that_says_it_is_gone() {
    let (_temp, store) = store();
    let put = applied(&store, "mbp-7c1", put_machine("mbp-7c1", 1, "build-1"));
    applied(&store, "mbp-7c1", remove_machine("mbp-7c1", 2, "build-1"));
    let changes = store.changes(put).expect("the changes");
    assert_eq!(changes.rows.len(), 1);
    assert!(
        changes.rows[0].deleted,
        "a computer that was away learns that the machine is gone"
    );
    assert_eq!(changes.rows[0].body, Value::Null);
    assert!(
        store.all().expect("the objects").rows[0].deleted,
        "and the state of the server holds it too"
    );
}

#[test]
fn a_kind_and_a_record_that_the_server_does_not_accept_are_refused() {
    let (_temp, store) = store();
    let unknown = refused(
        &store,
        "mbp-7c1",
        Intent {
            device: "mbp-7c1".to_owned(),
            id: 1,
            seen: None,
            op: Op::ObjectPut {
                kind: "card".to_owned(),
                id: "c1".to_owned(),
                body: json!({}),
            },
        },
    );
    assert_eq!(
        unknown,
        Refusal::UnknownKind {
            kind: "card".to_owned()
        },
        "a kind that this slice does not hold is refused, not stored"
    );

    let mut broken = machine("build-1");
    broken.session = Some("work; id".to_owned());
    let invalid = refused(
        &store,
        "mbp-7c1",
        Intent {
            device: "mbp-7c1".to_owned(),
            id: 2,
            seen: None,
            op: Op::MachinePut { record: broken },
        },
    );
    assert!(
        matches!(&invalid, Refusal::InvalidRecord { code, .. } if code == "invalid_session_name"),
        "{invalid:?}"
    );
    assert_eq!(invalid.code(), "invalid_record");
}

#[test]
fn a_refusal_quotes_a_bounded_part_of_what_a_computer_sent() {
    let (_temp, store) = store();
    let long = "a".repeat(5000);
    let refused = refused(
        &store,
        "mbp-7c1",
        Intent {
            device: "mbp-7c1".to_owned(),
            id: 1,
            seen: None,
            op: Op::ObjectPut {
                kind: long,
                id: "p1".to_owned(),
                body: json!({}),
            },
        },
    );
    let Refusal::UnknownKind { kind } = refused else {
        panic!("the kind is unknown: {refused:?}");
    };
    assert_eq!(
        kind.chars().count(),
        83,
        "a refusal quotes 80 characters and three dots, not a frame"
    );
    assert!(kind.ends_with("..."));
}

#[test]
fn an_intent_of_another_computer_is_refused_and_the_probes_are_bounded() {
    let (_temp, store) = store();
    let other = refused(&store, "mbp-7c1", put_machine("other-2a", 1, "build-1"));
    assert!(
        matches!(&other, Refusal::InvalidIntent { message } if message.contains("other-2a")),
        "{other:?}"
    );

    for index in 0..MAX_PROBES {
        applied(
            &store,
            "mbp-7c1",
            put_probe("mbp-7c1", index + 10, None, &format!("p{index}"), json!({ "n": index })),
        );
    }
    let too_many = refused(&store, "mbp-7c1", put_probe("mbp-7c1", 1000, None, "p-last", json!({})));
    assert_eq!(
        too_many,
        Refusal::TooMany {
            kind: KIND_PROBE.to_owned(),
            limit: MAX_PROBES
        }
    );
}

#[test]
fn a_push_above_the_intent_limit_is_not_accepted() {
    let (_temp, store) = store();
    let intents: Vec<Intent> = (1..=257)
        .map(|id| put_machine("mbp-7c1", id, &format!("build-{id}")))
        .collect();
    let error = store
        .push("mbp-7c1", &intents)
        .expect_err("the push is above the limit");
    assert!(matches!(error, StoreError::TooManyIntents { limit: 256 }), "{error}");
    assert_eq!(store.index(), 0, "a refused push writes nothing");
}

#[test]
fn a_file_of_a_newer_horizon_is_refused_and_a_damaged_one_is_reported() {
    let temp = tempfile::tempdir().expect("a directory");
    let path = temp.path().join("server.sqlite");
    let store = Store::open(&path).expect("a new store");
    store.close();
    {
        let connection = rusqlite::Connection::open(&path).expect("the file");
        connection
            .execute("UPDATE meta SET value = '99' WHERE key = 'schema_version'", [])
            .expect("a newer version");
    }
    match Store::open(&path) {
        Ok(store) => {
            store.close();
            panic!("a file of a newer Horizon must be refused");
        }
        Err(refused) => assert!(
            matches!(refused, StoreError::Newer { found: 99, expected: 6 }),
            "{refused}"
        ),
    }

    let damaged = temp.path().join("damaged.sqlite");
    std::fs::write(&damaged, b"this is not a database").expect("a damaged file");
    assert!(Store::open(&damaged).is_err(), "a file that is not a store is refused");
}

/// The store and the two files that SQLite keeps beside it are private: the
/// server user alone reads them, whatever the umask of the process that started
/// the server.
#[test]
fn the_store_and_its_write_ahead_files_are_private() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("a directory");
    let path = temp.path().join("server.sqlite");
    let store = Store::open(&path).expect("a new store");
    // One push makes the write-ahead log and its shared index, which a store
    // that has not written yet may not have made.
    applied(&store, "mbp-7c1", put_machine("mbp-7c1", 1, "build-1"));

    let mode_of = |path: &std::path::Path| {
        std::fs::metadata(path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
            .permissions()
            .mode()
            & 0o777
    };
    let log = temp.path().join("server.sqlite-wal");
    let index = temp.path().join("server.sqlite-shm");
    assert_eq!(mode_of(&path), 0o600, "the store is the server user's alone");
    assert!(log.exists(), "a written store keeps its write-ahead log");
    assert_eq!(mode_of(&log), 0o600, "the write-ahead log is private too");
    assert_eq!(mode_of(&index), 0o600, "the shared index is private too");
    store.close();

    // A file that an older Horizon made with the default mode is made private
    // again instead of staying readable for everyone who reaches its directory.
    let loose = temp.path().join("loose.sqlite");
    std::fs::write(&loose, b"").expect("an empty file");
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).expect("a loose mode");
    let store = Store::open(&loose).expect("the store of a loose file");
    assert_eq!(mode_of(&loose), 0o600, "a loose file is made private at the start");
    store.close();
}

/// One secret intent of a machine key, with a value.
fn put_secret(device: &str, id: u64, seen: Option<u64>, secret: &str, body: &str) -> Intent {
    Intent {
        device: device.to_owned(),
        id,
        seen,
        op: Op::SecretPut {
            id: secret.to_owned(),
            body: Some(horizon_sync::SecretBody::new(body.to_owned())),
        },
    }
}

fn remove_secret(device: &str, id: u64, secret: &str) -> Intent {
    Intent {
        device: device.to_owned(),
        id,
        seen: None,
        op: Op::SecretRemove { id: secret.to_owned() },
    }
}

/// A key pair as one secret body, taken from `private_key`.
fn key_body(private_key: &str) -> String {
    format!(
        "{{\"private_key\":{},\"public_key\":\"ssh-ed25519 AAAA test\",\"known_hosts\":\"\"}}",
        serde_json::to_string(private_key).expect("a string")
    )
}

#[test]
fn a_secret_travels_with_its_body_and_its_body_is_in_no_row_of_the_log() {
    let temp = tempfile::tempdir().expect("a directory");
    let path = temp.path().join("server.sqlite");
    let store = Store::open(&path).expect("a new store");
    let body = key_body("-----BEGIN OPENSSH PRIVATE KEY-----\nMARKER-7f3a-key-text\n-----END OPENSSH PRIVATE KEY-----");
    let index = applied(
        &store,
        "mbp-7c1",
        put_secret("mbp-7c1", 1, None, "machine-key/build-1", &body),
    );
    let changes = store.pull(0, "").expect("the rows of the server");
    let row = changes
        .rows
        .iter()
        .find(|row| row.kind == horizon_sync::KIND_SECRET)
        .expect("the secret row");
    assert_eq!(row.id, "machine-key/build-1");
    assert_eq!(row.version, index);
    assert_eq!(row.body, Value::String(body.clone()), "the body of the secret travels");

    // The log holds the kind, the id and the version of a secret change, and not
    // its body. The database holds no plain body either.
    store.close();
    let connection = rusqlite::Connection::open(&path).expect("the file");
    let mut statement = connection
        .prepare("SELECT kind, object_id, result_json FROM log")
        .expect("the log");
    let rows: Vec<(String, String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("the rows")
        .collect::<Result<_, _>>()
        .expect("the rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "secret");
    assert_eq!(rows[0].1, "machine-key/build-1");
    for text in [&rows[0].2, &rows[0].0, &rows[0].1] {
        assert!(!text.contains("MARKER-7f3a"), "the log quotes no body: {text}");
    }
    let sealed: Vec<u8> = connection
        .query_row(
            "SELECT sealed FROM secrets WHERE id = 'machine-key/build-1'",
            [],
            |row| row.get(0),
        )
        .expect("the sealed body");
    assert!(
        !String::from_utf8_lossy(&sealed).contains("MARKER-7f3a"),
        "the table holds no plain body"
    );
    assert!(
        std::fs::read(&path)
            .expect("the file")
            .windows(12)
            .all(|window| window != b"MARKER-7f3a"),
        "no part of the database holds the plain body"
    );
}

#[test]
fn the_key_of_the_secrets_is_a_private_file_beside_the_database() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("a directory");
    let path = temp.path().join("server.sqlite");
    let store = Store::open(&path).expect("a new store");
    applied(
        &store,
        "mbp-7c1",
        put_secret("mbp-7c1", 1, None, "machine-key/build-1", &key_body("first")),
    );
    store.close();
    let key = temp.path().join("secret.key");
    let bytes = std::fs::read(&key).expect("the key file");
    assert_eq!(bytes.len(), 32, "the key is 32 bytes");
    assert_eq!(
        std::fs::metadata(&key).expect("the key").permissions().mode() & 0o777,
        0o600,
        "the key file is private"
    );
    assert!(
        !std::fs::read(&path)
            .expect("the file")
            .windows(32)
            .any(|window| window == bytes.as_slice()),
        "the database does not hold the key"
    );

    // A second store of the same home uses the same key, so the body opens.
    let store = Store::open(&path).expect("the same store");
    let row = store
        .pull(0, "")
        .expect("the rows")
        .rows
        .into_iter()
        .find(|row| row.kind == horizon_sync::KIND_SECRET)
        .expect("the secret row");
    assert_eq!(row.body, Value::String(key_body("first")));
    assert!(!store.secrets_lost());
    store.close();

    // A file without its key is a server that cannot read what it holds: it keeps
    // running, reports the loss, and a computer that holds the value sends it
    // again.
    std::fs::remove_file(&key).expect("the key is lost");
    let store = Store::open(&path).expect("a store without its key keeps running");
    assert!(store.secrets_lost(), "the loss of the key is reported");
    assert!(
        store.pull(0, "").expect("the rows").rows.is_empty(),
        "a secret that this key cannot open is no row of a pull"
    );
    let row = store
        .pull(0, "")
        .expect("the rows")
        .rows
        .into_iter()
        .find(|row| row.kind == horizon_sync::KIND_SECRET);
    assert!(row.is_none());
    applied(
        &store,
        "mbp-7c1",
        put_secret("mbp-7c1", 2, None, "machine-key/build-1", &key_body("again")),
    );
    assert!(!store.secrets_lost(), "the value that was sent again opens");
    store.close();
}

#[test]
fn the_version_rule_of_a_secret_and_its_refusals() {
    let (_temp, store) = store();
    let body = key_body("one");
    let first = applied(
        &store,
        "mbp-7c1",
        put_secret("mbp-7c1", 1, None, "machine-key/build-1", &body),
    );
    assert!(matches!(
        refused(&store, "mbp-7c1", put_secret("mbp-7c1", 2, None, "machine-key/build-1", &body)),
        Refusal::AlreadyAnswered { version } if version == first
    ));
    applied(
        &store,
        "mbp-7c1",
        put_secret("mbp-7c1", 3, Some(first), "machine-key/build-1", &body),
    );
    let now = store.index();
    assert!(matches!(
        refused(&store, "mbp-7c1", put_secret("mbp-7c1", 4, Some(first), "machine-key/build-1", &body)),
        Refusal::ChangedSinceSeen { seen, version } if seen == first && version == now
    ));
    applied(&store, "mbp-7c1", remove_secret("mbp-7c1", 5, "machine-key/build-1"));
    assert!(
        store
            .pull(0, "")
            .expect("the rows")
            .rows
            .iter()
            .find(|row| row.kind == horizon_sync::KIND_SECRET)
            .is_some_and(|row| row.deleted),
        "a removed secret stays as a tombstone"
    );
    // A secret that the server removed is no row that a computer can see, so the
    // value that a computer still holds is created again: that is how a value of a
    // server whose key was lost comes back.
    applied(
        &store,
        "mbp-7c1",
        put_secret("mbp-7c1", 6, None, "machine-key/build-1", &body),
    );
}

#[test]
fn a_secret_without_a_value_a_bad_id_a_large_body_and_a_full_server_are_refused() {
    let (_temp, store) = store();
    let mut intent = put_secret("mbp-7c1", 1, None, "machine-key/build-1", &key_body("one"));
    intent.op = Op::SecretPut {
        id: "machine-key/build-1".to_owned(),
        body: None,
    };
    assert!(matches!(
        refused(&store, "mbp-7c1", intent),
        Refusal::InvalidIntent { .. }
    ));

    for id in [
        "machine-key/Build",
        "machine-key/-build",
        "machine-key/build-1/2",
        "credential/a b",
        "credential/",
        "machine-key/",
        "../machine-key/build-1",
        "machine-key/build.1",
        "secret/build-1",
    ] {
        let refused = refused(&store, "mbp-7c1", put_secret("mbp-7c1", 2, None, id, "value"));
        assert!(
            matches!(refused, Refusal::InvalidIntent { .. }),
            "{id} is refused: {refused:?}"
        );
    }

    let large = "v".repeat(horizon_sync::MAX_SECRET_BYTES + 1);
    assert!(matches!(
        refused(
            &store,
            "mbp-7c1",
            put_secret("mbp-7c1", 3, None, "credential/key", &large)
        ),
        Refusal::InvalidIntent { .. }
    ));

    // The limit of the server: 256 secrets, and the 257th is a typed refusal.
    for count in 0..horizon_sync::MAX_SECRETS {
        let id = format!("credential/key-{count}");
        applied(
            &store,
            "mbp-7c1",
            put_secret("mbp-7c1", 100 + count, None, &id, "value"),
        );
    }
    assert!(matches!(
        refused(
            &store,
            "mbp-7c1",
            put_secret("mbp-7c1", 999, None, "credential/one-more", "value")
        ),
        Refusal::TooMany { kind, limit } if kind == "secret" && limit == horizon_sync::MAX_SECRETS
    ));
    // A replace of a secret that is there is not a new one.
    let version = store
        .pull(0, "")
        .expect("the rows")
        .rows
        .into_iter()
        .find(|row| row.kind == horizon_sync::KIND_SECRET && row.id == "credential/key-0")
        .map(|row| row.version)
        .expect("the version of a secret");
    applied(
        &store,
        "mbp-7c1",
        put_secret("mbp-7c1", 1000, Some(version), "credential/key-0", "another"),
    );
}

#[test]
fn an_upgrade_from_the_older_schema_keeps_the_computers_that_it_already_had() {
    // A server of the older schema, with one computer of the socket and one that
    // joined with a token. The version that adds the greeting must keep both: a
    // row that the older file held was greeted before that version existed.
    let temp = tempfile::tempdir().expect("a directory");
    let path = temp.path().join("server.sqlite");
    let connection = rusqlite::Connection::open(&path).expect("the file");
    connection
        .execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
             CREATE TABLE log (idx INTEGER PRIMARY KEY, at_millis INTEGER NOT NULL,
                 device TEXT NOT NULL, intent INTEGER NOT NULL, kind TEXT NOT NULL,
                 object_id TEXT NOT NULL, result_json TEXT NOT NULL) STRICT;
             CREATE TABLE applied (device TEXT NOT NULL, intent INTEGER NOT NULL,
                 idx INTEGER NOT NULL, UNIQUE (device, intent)) STRICT;
             CREATE TABLE objects (kind TEXT NOT NULL, id TEXT NOT NULL, body_json TEXT NOT NULL,
                 version INTEGER NOT NULL, deleted INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (kind, id)) STRICT;
             CREATE INDEX objects_by_version ON objects (version);
             CREATE TABLE devices (name TEXT PRIMARY KEY, first_seen INTEGER NOT NULL,
                 last_seen INTEGER NOT NULL, last_intent INTEGER NOT NULL,
                 token_hash TEXT, created_millis INTEGER NOT NULL DEFAULT 0,
                 login TEXT NOT NULL DEFAULT 'ssh') STRICT;
             CREATE UNIQUE INDEX devices_by_token ON devices (token_hash) WHERE token_hash IS NOT NULL;
             CREATE TABLE invitations (code_hash TEXT PRIMARY KEY, created_millis INTEGER NOT NULL,
                 expires_millis INTEGER NOT NULL) STRICT;
             CREATE TABLE secrets (id TEXT PRIMARY KEY, nonce BLOB NOT NULL, sealed BLOB NOT NULL,
                 version INTEGER NOT NULL, deleted INTEGER NOT NULL DEFAULT 0) STRICT;
             CREATE INDEX secrets_by_version ON secrets (version);
             INSERT INTO meta (key, value) VALUES ('store_id', 'a-store-of-a-test'),
                 ('schema_version', '3');
             INSERT INTO devices (name, first_seen, last_seen, last_intent, login)
                 VALUES ('laptop-a', 100, 200, 0, 'ssh');
             INSERT INTO devices (name, first_seen, last_seen, last_intent, token_hash,
                 created_millis, login)
                 VALUES ('laptop-b', 300, 400, 0, 'a-hash-of-a-test', 300, 'token');",
        )
        .expect("the older schema");
    drop(connection);

    let store = Store::open(&path).expect("the upgrade of an older store");
    let rows = store.devices("laptop-a").expect("the computers of the server");
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, vec!["laptop-a", "laptop-b"], "the upgrade keeps every computer");
    assert_eq!(
        store
            .prune_unjoined(i64::MAX)
            .expect("the sweep of a version that joins"),
        0,
        "a computer that the older file held was greeted before this version"
    );
    store.close();
}

#[test]
fn a_section_of_the_settings_travels_by_the_version_rule_and_the_server_stores_it_unread() {
    let (_temp, store) = store();
    let first = applied(
        &store,
        "mbp-7c1",
        put_setting("mbp-7c1", 1, None, "appearance", section_body("light")),
    );

    // The body is the JSON that the computer sent, and the server did not read it.
    let rows = store.all().expect("the rows of the server").rows;
    let appearance = rows
        .iter()
        .find(|row| row.kind == KIND_SETTING && row.id == "appearance")
        .expect("the section of the server");
    assert_eq!(appearance.body, section_body("light"));
    assert_eq!(appearance.version, first);

    // The other computer read that version, so its change is the next one.
    let second = applied(
        &store,
        "other-2a",
        put_setting("other-2a", 1, Some(first), "appearance", section_body("dark")),
    );
    assert!(second > first);

    // The computer that was away sends the change on the version that it saw, and
    // the first change of that version won.
    let stale = refused(
        &store,
        "third-3b",
        put_setting("third-3b", 1, Some(first), "appearance", section_body("auto")),
    );
    assert_eq!(
        stale,
        Refusal::ChangedSinceSeen {
            seen: first,
            version: second
        }
    );
    let rows = store.all().expect("the rows of the server").rows;
    let appearance = rows
        .iter()
        .find(|row| row.kind == KIND_SETTING && row.id == "appearance")
        .expect("the section of the server");
    assert_eq!(
        appearance.body,
        section_body("dark"),
        "the value of the first change stands"
    );
}

#[test]
fn a_setting_that_breaks_a_rule_of_the_server_is_refused_and_writes_nothing() {
    let (_temp, store) = store();
    for (id, name) in [(1, ""), (2, "Appearance"), (3, "appearance/theme"), (4, "a")] {
        let intent = put_setting("mbp-7c1", id, None, name, section_body("light"));
        if name == "a" {
            // One character of `a-z` is a name that a newer Horizon may add.
            applied(&store, "mbp-7c1", intent);
            continue;
        }
        match refused(&store, "mbp-7c1", intent) {
            Refusal::InvalidIntent { message } => assert!(message.contains("setting id"), "{message}"),
            other => panic!("a name that is not one section is refused: {other:?}"),
        }
    }

    let long = "a".repeat(41);
    assert!(matches!(
        refused(
            &store,
            "mbp-7c1",
            put_setting("mbp-7c1", 9, None, &long, section_body("light"))
        ),
        Refusal::InvalidIntent { .. }
    ));

    // A section above the size of the settings travels as nothing, so an oversize
    // value costs one refusal and never one over-large frame.
    let large = json!({
        "format": 1,
        "config_version": 12,
        "section": { "text": "x".repeat(horizon_core::settings_sync::MAX_SECTION_BYTES) }
    });
    match refused(&store, "mbp-7c1", put_setting("mbp-7c1", 10, None, "workspaces", large)) {
        Refusal::InvalidIntent { message } => assert!(message.contains("bytes"), "{message}"),
        other => panic!("an oversize section is refused: {other:?}"),
    }
    assert_eq!(
        store
            .all()
            .expect("the rows of the server")
            .rows
            .iter()
            .filter(|row| row.kind == KIND_SETTING)
            .count(),
        1,
        "a refusal writes no object, so the one name that this build accepts is the only section"
    );
}

#[test]
fn the_server_keeps_at_most_the_sections_of_one_settings_store() {
    let (_temp, store) = store();
    for index in 0..MAX_SETTINGS {
        applied(
            &store,
            "mbp-7c1",
            put_setting(
                "mbp-7c1",
                index + 1,
                None,
                &format!("section_{index}"),
                section_body("light"),
            ),
        );
    }
    let refusal = refused(
        &store,
        "mbp-7c1",
        put_setting("mbp-7c1", MAX_SETTINGS + 1, None, "one_too_many", section_body("light")),
    );
    assert_eq!(
        refusal,
        Refusal::TooMany {
            kind: KIND_SETTING.to_owned(),
            limit: MAX_SETTINGS
        }
    );
}

#[test]
fn a_store_of_the_previous_schema_keeps_its_rows_and_gains_the_index_of_its_kinds() {
    let temp = tempfile::tempdir().expect("a directory");
    let path = temp.path().join("server.sqlite");
    let store = Store::open(&path).expect("a new store");
    applied(&store, "mbp-7c1", put_machine("mbp-7c1", 1, "build-1"));
    store.close();
    {
        // The file that the previous Horizon wrote: the same rows, the older
        // schema number and no index of the live kinds.
        let connection = rusqlite::Connection::open(&path).expect("the file");
        connection
            .execute_batch("DROP INDEX objects_live_by_kind; UPDATE meta SET value = '5' WHERE key = 'schema_version';")
            .expect("a file of the previous schema");
    }
    let store = Store::open(&path).expect("the store of the previous schema");
    assert_eq!(
        store
            .pull(0, "")
            .expect("the rows of the server")
            .rows
            .iter()
            .filter(|row| row.kind == KIND_MACHINE)
            .count(),
        1,
        "an upgraded store keeps its objects"
    );
    assert!(store.all().is_ok(), "the log and the objects read after the upgrade");
    store.close();
    let connection = rusqlite::Connection::open(&path).expect("the file");
    let indexed: Option<String> = connection
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'objects_live_by_kind'",
            [],
            |row| row.get(0),
        )
        .ok();
    assert_eq!(
        indexed.as_deref(),
        Some("objects_live_by_kind"),
        "the step added its index"
    );
}
