//! The server store, judged on a temporary file: idempotence, the index, the
//! version rule, the limits and the time of the server.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use horizon_core::Machine;
use horizon_sync::{
    Changes, Intent, KIND_MACHINE, KIND_PROBE, MAX_PROBES, Op, Outcome, Refusal, Store, StoreError, tip,
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
        67,
        "a refusal quotes 64 characters and three dots, not a frame"
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
            matches!(refused, StoreError::Newer { found: 99, expected: 1 }),
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
