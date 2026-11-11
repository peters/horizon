//! The link protocol over in-memory duplex streams, a temporary server file and a
//! temporary queue file: the frames, the version, the waits, the reset after a
//! restored file, and the queue that survives a restart.

use std::{
    io::Write as _,
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use horizon_core::Machine;
use horizon_sync::{
    Client, Frame, FrameError, Intent, KIND_MACHINE, Link, LinkErrorCode, LinkState, MAX_FRAME_BYTES, MAX_OPEN_WAITS,
    ObjectMirror, Op, PROTOCOL_VERSION, Refusal, Store, SyncError, Wait, read_frame, serve, tip, write_frame,
};

/// One computer with a link to a server store, and the thread of that link.
struct Peer {
    client: Client,
    link: Link<UnixStream, UnixStream>,
    serving: JoinHandle<()>,
}

impl Peer {
    /// Connects `device` to `store`, with its queue file in `queue_dir`.
    fn connect(store: &Store, queue_dir: &std::path::Path, device: &str) -> Self {
        let (computer, server) = UnixStream::pair().expect("a socket pair");
        let reading = server.try_clone().expect("a second end");
        let serving_store = store.clone();
        let serving = thread::spawn(move || {
            if let Err(error) = serve(reading, server, &serving_store) {
                eprintln!("a link of the test ended: {error}");
            }
        });
        let mut client = Client::open(queue_dir.join(format!("{device}.queue.json")), device).expect("a queue");
        let mut link = Link::new(computer.try_clone().expect("the client end"), computer);
        client.hello(&mut link).expect("the hello");
        Self { client, link, serving }
    }

    fn push(&mut self) {
        self.client.push_pending(&mut self.link).expect("the push");
    }

    fn pull(&mut self, mirror: &mut ObjectMirror) -> horizon_sync::PullOutcome {
        self.client.pull(&mut self.link, mirror).expect("the pull")
    }
}

fn machine(name: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: "203.0.113.10".to_owned(),
        user: Some("root".to_owned()),
        provider: Some("Hetzner, hel1".to_owned()),
        added_at_millis: 1,
        ..Machine::default()
    }
}

fn add_machine(peer: &mut Peer, name: &str) {
    peer.client
        .enqueue(None, Op::MachinePut { record: machine(name) })
        .expect("the intent is queued");
    peer.push();
}

fn new_store(temp: &tempfile::TempDir) -> Store {
    Store::open(&temp.path().join("server.sqlite")).expect("the server store")
}

#[test]
fn a_computer_pushes_a_machine_and_pulls_it_back_into_its_mirror() {
    let temp = tempfile::tempdir().expect("a directory");
    let store = new_store(&temp);
    let mut peer = Peer::connect(&store, temp.path(), "mbp-7c1");
    assert_eq!(peer.client.state(), &LinkState::Behind, "after the hello a pull is due");

    add_machine(&mut peer, "build-1");
    assert_eq!(peer.client.pending().len(), 0, "the queue is settled");
    assert_eq!(store.index(), 1);

    let mut mirror = ObjectMirror::default();
    let pulled = peer.pull(&mut mirror);
    assert_eq!(pulled.index, 1);
    assert!(!pulled.reset);
    assert_eq!(mirror.len(), 1);
    let row = mirror.get(KIND_MACHINE, "build-1").expect("the machine");
    assert_eq!(row.version, 1);
    let record: Machine = serde_json::from_value(row.body.clone()).expect("the machine record");
    assert_eq!(record.host, "203.0.113.10");
    assert_eq!(
        peer.client.state(),
        &LinkState::InStep { index: 1 },
        "the mirror is in step after the pull"
    );
    assert_eq!(peer.client.mark(), tip(store.store_id(), 1), "the mark names this log");

    // A second pull from the same index changes nothing and keeps the state.
    let again = peer.pull(&mut mirror);
    assert_eq!(again.index, 1);
    assert_eq!(again.rows, 0);
    assert!(!again.reset);
}

#[test]
fn ten_computers_push_over_their_links_and_the_log_has_no_gap() {
    let temp = tempfile::tempdir().expect("a directory");
    let store = new_store(&temp);
    let started = Instant::now();
    let pushed = Arc::new(AtomicU64::new(0));
    let mut threads = Vec::new();
    for computer in 0..10 {
        let store = store.clone();
        let queue_dir = temp.path().to_path_buf();
        let pushed = Arc::clone(&pushed);
        threads.push(thread::spawn(move || {
            let device = format!("computer-{computer}");
            let mut peer = Peer::connect(&store, &queue_dir, &device);
            let name = format!("build-{computer}");
            for _ in 0..10 {
                add_machine(&mut peer, &name);
                let mut mirror = ObjectMirror::default();
                let _ = peer.pull(&mut mirror);
                assert!(
                    mirror.get(KIND_MACHINE, &name).is_some(),
                    "the machine of {device} is in its own mirror"
                );
                pushed.fetch_add(1, Ordering::SeqCst);
            }
            drop(peer.client);
            drop(peer.link);
            let _ = peer.serving.join();
        }));
    }
    for thread in threads {
        thread.join().expect("the computer ended");
    }
    let elapsed = started.elapsed();
    assert_eq!(pushed.load(Ordering::SeqCst), 100);
    assert_eq!(store.index(), 100, "ten computers write one run of indexes");
    assert_eq!(
        store.all().expect("the objects").rows.len(),
        10,
        "each computer kept one machine, and each write replaced it"
    );
    println!(
        "10 linked computers, 100 intents, {:.3} s, no push waited for a lock",
        elapsed.as_secs_f64()
    );
    let path = store.path().to_path_buf();
    store.close();
    let connection = rusqlite::Connection::open(&path).expect("the file");
    let indexes: Vec<i64> = connection
        .prepare("SELECT idx FROM log ORDER BY idx")
        .expect("the log")
        .query_map([], |row| row.get(0))
        .expect("the rows")
        .collect::<Result<_, _>>()
        .expect("the indexes");
    assert_eq!(indexes, (1..=100).collect::<Vec<i64>>(), "log.idx is one run from 1");
}

#[test]
fn a_wait_answers_on_a_change_of_another_computer_and_at_its_bound() {
    let temp = tempfile::tempdir().expect("a directory");
    let store = new_store(&temp);
    let mut waiting = Peer::connect(&store, temp.path(), "mbp-7c1");
    let mut mirror = ObjectMirror::default();

    let started = Instant::now();
    let woken = thread::scope(|scope| {
        scope.spawn(|| {
            let mut other = Peer::connect(&store, temp.path(), "other-2a");
            thread::sleep(Duration::from_millis(50));
            add_machine(&mut other, "build-1");
            drop(other.client);
            drop(other.link);
            let _ = other.serving.join();
        });
        waiting
            .client
            .wait(&mut waiting.link, &mut mirror)
            .expect("the wait answers")
    });
    assert_eq!(woken.rows, 1, "the wait woke on the change of another computer");
    assert_eq!(mirror.len(), 1);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the wait woke before its bound"
    );

    // The bound of a wait is 50 seconds, and a test does not hold a link for it:
    // the store test holds that bound with a short one.
    assert_eq!(horizon_sync::WAIT_BOUND, Duration::from_secs(50), "the bound of a wait");
    assert_eq!(horizon_sync::MAX_OPEN_WAITS, 32, "the waits of one link");
}

#[test]
fn a_queue_file_survives_a_restart_and_resends_in_order_with_the_same_ids() {
    let temp = tempfile::tempdir().expect("a directory");
    let store = new_store(&temp);
    let queue = temp.path().join("mbp-7c1.queue.json");

    let mut client = Client::open(queue.clone(), "mbp-7c1").expect("a queue");
    for name in ["build-1", "build-2", "build-3"] {
        client
            .enqueue(None, Op::MachinePut { record: machine(name) })
            .expect("the intent is queued");
    }
    assert_eq!(
        client.pending().iter().map(|intent| intent.id).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    drop(client);

    let mut client = Client::open(queue, "mbp-7c1").expect("the same computer reads its queue");
    assert_eq!(
        client.pending().iter().map(|intent| intent.id).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "a restart keeps the order and the counters"
    );

    // The same queue file, a link of its own: the identifiers are the ones that
    // the first session made.
    let (computer, server) = UnixStream::pair().expect("a socket pair");
    let reading = server.try_clone().expect("a second end");
    let serving_store = store.clone();
    let serving = thread::spawn(move || {
        if let Err(error) = serve(reading, server, &serving_store) {
            eprintln!("a link of the test ended: {error}");
        }
    });
    let mut link = Link::new(computer.try_clone().expect("the client end"), computer);
    client.hello(&mut link).expect("the hello");
    let outcome = client.push_pending(&mut link).expect("the push after the restart");
    assert_eq!(outcome.applied, vec![1, 2, 3], "the same identifiers, in order");
    assert_eq!(client.pending().len(), 0);
    assert_eq!(store.index(), 3, "each intent applied once");
    drop(client);
    drop(link);
    let _ = serving.join();
}

#[test]
fn a_restored_server_file_makes_the_next_pull_reset_the_mirror() {
    let temp = tempfile::tempdir().expect("a directory");
    let path = temp.path().join("server.sqlite");
    let copy = temp.path().join("server.copy.sqlite");
    let mut mirror = ObjectMirror::default();

    let store = Store::open(&path).expect("the server store");
    let mut kept = Peer::connect(&store, temp.path(), "mbp-7c1");
    for name in ["build-1", "build-2", "build-3"] {
        add_machine(&mut kept, name);
    }
    assert_eq!(store.index(), 3);
    store.close();
    std::fs::copy(&path, &copy).expect("a copy of the server file");
    drop(kept.link);
    let _ = kept.serving.join();
    drop(kept.client);

    let store = Store::open(&path).expect("the server again");
    let mut writing = Peer::connect(&store, temp.path(), "other-2a");
    for name in ["build-4", "build-5", "build-6"] {
        add_machine(&mut writing, name);
    }
    assert_eq!(store.index(), 6);
    drop(writing.client);
    drop(writing.link);
    let _ = writing.serving.join();
    store.close();

    // The computer of the first three changes pulls the six and holds index 6.
    let store = Store::open(&path).expect("the server of six changes");
    let mut kept = Peer::connect(&store, temp.path(), "mbp-7c1");
    let pulled = kept.pull(&mut mirror);
    assert_eq!(pulled.index, 6);
    assert_eq!(mirror.len(), 6);
    let mark = kept.client.mark().to_owned();
    assert_eq!(mark, tip(store.store_id(), 6));
    drop(kept.client);
    drop(kept.link);
    let _ = kept.serving.join();
    store.close();

    // A restore of the copy: the server reaches three changes again.
    std::fs::copy(&copy, &path).expect("the restore");
    let restored = Store::open(&path).expect("the restored server");
    assert_eq!(restored.index(), 3, "the server is behind the computer");

    let (computer, server) = UnixStream::pair().expect("a socket pair");
    let reading = server.try_clone().expect("a second end");
    let serving_store = restored.clone();
    let serving = thread::spawn(move || {
        if let Err(error) = serve(reading, server, &serving_store) {
            eprintln!("a link of the test ended: {error}");
        }
    });
    let mut client = Client::open(temp.path().join("mbp-7c1.queue.json"), "mbp-7c1").expect("its queue");
    let mut link = Link::new(computer.try_clone().expect("the client end"), computer);
    client.hello(&mut link).expect("the hello of a restored server");
    assert_eq!(client.index(), 6, "the computer still holds the log of six changes");
    let answer = client.pull(&mut link, &mut mirror).expect("the pull");
    assert!(answer.reset, "a restored server tells the computer to reset");
    assert_eq!(answer.index, 3);
    assert_eq!(mirror.len(), 3, "the mirror equals the restored server");
    assert_eq!(
        client.mark(),
        tip(restored.store_id(), 3),
        "and the mark names the restored log"
    );
    drop(client);
    drop(link);
    let _ = serving.join();
    restored.close();
}

#[test]
fn an_oversize_frame_and_a_newer_protocol_are_refused_with_a_typed_error() {
    let temp = tempfile::tempdir().expect("a directory");
    let store = new_store(&temp);

    // An oversize frame: the server answers and ends the link.
    let (mut computer, server) = UnixStream::pair().expect("a socket pair");
    let reading = server.try_clone().expect("a second end");
    let serving_store = store.clone();
    let serving = thread::spawn(move || serve(reading, server, &serving_store));
    let mut writer = computer.try_clone().expect("the write end");
    let mut oversize = u32::try_from(MAX_FRAME_BYTES + 1)
        .expect("one length")
        .to_be_bytes()
        .to_vec();
    oversize.extend_from_slice(b"{}");
    writer.write_all(&oversize).expect("the oversize frame");
    let answer = read_frame(&mut computer).expect("an answer frame").expect("one frame");
    let Frame::Error(error) = answer else {
        panic!("the server answers an error frame, not {answer:?}");
    };
    assert_eq!(error.code, LinkErrorCode::FrameTooLarge);
    assert!(
        matches!(read_frame(&mut computer), Ok(None) | Err(FrameError::Io(_))),
        "the server ends the link after an oversize frame"
    );
    let ended = serving.join().expect("the link thread");
    assert!(ended.is_err(), "the link reports why it ended");

    // A hello of a newer protocol: the answer names both versions.
    let (mut computer, server) = UnixStream::pair().expect("a socket pair");
    let reading = server.try_clone().expect("a second end");
    let serving_store = store.clone();
    let serving = thread::spawn(move || serve(reading, server, &serving_store));
    let mut writer = computer.try_clone().expect("the write end");
    write_frame(
        &mut writer,
        &Frame::Hello(horizon_sync::Hello {
            protocol: PROTOCOL_VERSION + 1,
            device: "mbp-7c1".to_owned(),
            store: None,
            token: None,
        }),
    )
    .expect("a hello of a newer protocol");
    let answer = read_frame(&mut computer).expect("an answer frame").expect("one frame");
    let Frame::Error(error) = answer else {
        panic!("the server answers an error frame, not {answer:?}");
    };
    assert_eq!(error.code, LinkErrorCode::ProtocolVersion);
    let spoken = PROTOCOL_VERSION;
    let asked = PROTOCOL_VERSION + 1;
    assert!(
        error.message.contains(&asked.to_string()) && error.message.contains(&spoken.to_string()),
        "the message names both versions: {}",
        error.message
    );
    let _ = serving.join();

    // A frame before the hello, and a device name that breaks the rule.
    for (first, code) in [
        (Frame::Wait(Wait { since: 0 }), LinkErrorCode::NotHello),
        (
            Frame::Hello(horizon_sync::Hello {
                protocol: PROTOCOL_VERSION,
                device: "MBP-7c1".to_owned(),
                store: None,
                token: None,
            }),
            LinkErrorCode::BadDevice,
        ),
    ] {
        let (mut computer, server) = UnixStream::pair().expect("a socket pair");
        let reading = server.try_clone().expect("a second end");
        let serving_store = store.clone();
        let serving = thread::spawn(move || serve(reading, server, &serving_store));
        let mut writer = computer.try_clone().expect("the write end");
        write_frame(&mut writer, &first).expect("the first frame");
        let answer = read_frame(&mut computer).expect("an answer frame").expect("one frame");
        let Frame::Error(error) = answer else {
            panic!("the server answers an error frame, not {answer:?}");
        };
        assert_eq!(error.code, code);
        let _ = serving.join();
    }
}

#[test]
fn a_push_of_too_many_intents_and_a_link_of_too_many_waits_are_refused() {
    let temp = tempfile::tempdir().expect("a directory");
    let store = new_store(&temp);
    let mut peer = Peer::connect(&store, temp.path(), "mbp-7c1");

    let intents: Vec<Intent> = (1..=257)
        .map(|id| Intent {
            device: "mbp-7c1".to_owned(),
            id,
            seen: None,
            op: Op::MachinePut {
                record: machine(&format!("build-{id}")),
            },
        })
        .collect();
    let refused = peer.link.push(&intents).expect_err("a push above the limit is refused");
    assert!(
        matches!(&refused, SyncError::Refused { code, .. } if *code == "too_many_intents"),
        "{refused}"
    );
    assert_eq!(store.index(), 0, "a refused push writes nothing");
    drop(peer.client);
    drop(peer.link);
    let _ = peer.serving.join();

    // One link opens the maximum of waits, and the next wait is refused.
    let (mut computer, server) = UnixStream::pair().expect("a socket pair");
    let reading = server.try_clone().expect("a second end");
    let serving_store = store.clone();
    let serving = thread::spawn(move || serve(reading, server, &serving_store));
    let mut writer = computer.try_clone().expect("the write end");
    write_frame(
        &mut writer,
        &Frame::Hello(horizon_sync::Hello {
            protocol: PROTOCOL_VERSION,
            device: "mbp-7c2".to_owned(),
            store: None,
            token: None,
        }),
    )
    .expect("the hello");
    let answer = read_frame(&mut computer).expect("an answer").expect("one frame");
    assert!(matches!(answer, Frame::HelloOk(_)), "{answer:?}");
    for _ in 0..MAX_OPEN_WAITS {
        write_frame(&mut writer, &Frame::Wait(Wait { since: 0 })).expect("a wait");
    }
    write_frame(&mut writer, &Frame::Wait(Wait { since: 0 })).expect("the wait above the limit");
    let answer = read_frame(&mut computer).expect("an answer").expect("one frame");
    let Frame::Error(error) = answer else {
        panic!("the server answers the wait above the limit with an error, not {answer:?}");
    };
    assert_eq!(error.code, LinkErrorCode::TooManyWaits);
    drop(computer);
    drop(writer);
    let _ = serving.join();
}

#[test]
fn a_version_rule_holds_over_the_link_for_a_generic_object() {
    let temp = tempfile::tempdir().expect("a directory");
    let store = new_store(&temp);
    let mut first = Peer::connect(&store, temp.path(), "mbp-7c1");
    let mut second = Peer::connect(&store, temp.path(), "other-2a");

    first
        .client
        .enqueue(
            None,
            Op::ObjectPut {
                kind: "probe".to_owned(),
                id: "p1".to_owned(),
                body: serde_json::json!({ "n": 1 }),
            },
        )
        .expect("the intent is queued");
    let outcome = first.client.push_pending(&mut first.link).expect("the push");
    let index = match outcome.applied.first().copied() {
        Some(_) => store.index(),
        None => panic!("the first create is applied: {outcome:?}"),
    };

    second
        .client
        .enqueue(
            None,
            Op::ObjectPut {
                kind: "probe".to_owned(),
                id: "p1".to_owned(),
                body: serde_json::json!({ "n": 2 }),
            },
        )
        .expect("the intent is queued");
    let outcome = second.client.push_pending(&mut second.link).expect("the push");
    assert_eq!(outcome.refused.len(), 1, "the create is refused: {outcome:?}");
    assert_eq!(
        outcome.refused[0].1,
        Refusal::AlreadyAnswered { version: index },
        "the first intent on the version wins"
    );

    second
        .client
        .enqueue(
            Some(index - 1),
            Op::ObjectPut {
                kind: "probe".to_owned(),
                id: "p1".to_owned(),
                body: serde_json::json!({ "n": 3 }),
            },
        )
        .expect("the intent is queued");
    let outcome = second.client.push_pending(&mut second.link).expect("the push");
    assert_eq!(
        outcome.refused[0].1,
        Refusal::ChangedSinceSeen {
            seen: index - 1,
            version: index
        },
        "a stale version is refused with both numbers"
    );
}

#[test]
fn a_client_is_off_before_a_server_and_away_with_one_that_left() {
    let temp = tempfile::tempdir().expect("a directory");
    let mut client = Client::open(temp.path().join("queue.json"), "mbp-7c1").expect("a queue");
    assert_eq!(client.state(), &LinkState::Off);
    client
        .enqueue(
            None,
            Op::MachineRemove {
                name: "build-1".to_owned(),
            },
        )
        .expect("the intent is queued");
    client.away();
    assert_eq!(client.state(), &LinkState::Away { pending: 1 });
    client.off();
    assert_eq!(client.state(), &LinkState::Off);

    let other = Client::open(temp.path().join("queue.json"), "other-2a").expect_err("one file is one computer");
    assert!(
        matches!(&other, SyncError::Queue(message) if message.contains("mbp-7c1")),
        "{other}"
    );
    assert!(matches!(
        Client::open(temp.path().join("bad.json"), "MBP"),
        Err(SyncError::Device(name)) if name == "MBP"
    ));
}
