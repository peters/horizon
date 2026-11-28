//! The bridge on a pipe: two turns in one program, the order of the events, the
//! wake, a line above the bound, the end of a program, a quiet program, a
//! program that ignores its stop, and the send after an end.
//!
//! The stand-in of every test is `/bin/sh` with a script that the test writes
//! into a folder of its own. It prints hand-written JSON lines and reads the
//! lines of the test, so no test starts an agent program and no test reaches the
//! network.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::super::ending::end_child;
use super::super::worker::fail_start;
use crate::agent_bridge::{Bridge, BridgeError, BridgeEvent, ClaudeCodec, MAX_TEXT_BYTES, ProgramSpec, STOP_TIMEOUT};

/// The longest time that a test waits for one event of its stand-in.
const WAIT: Duration = Duration::from_secs(20);

/// The time between two reads of a value of a test.
const POLL: Duration = Duration::from_millis(5);

const SESSION: &str = "00000000-0000-4000-8000-000000000001";

/// The script of two turns: one init, one text and one result for each line that
/// the test sends.
const TWO_TURNS: &str = r#"#!/bin/sh
n=0
while IFS= read -r line; do
  n=$((n+1))
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
  printf '%s\n' "{\"type\":\"assistant\",\"message\":{\"id\":\"msg-$n\",\"content\":[{\"type\":\"text\",\"text\":\"turn $n\"}]}}"
  printf '%s\n' '{"type":"result","is_error":false,"duration_ms":5}'
done
"#;

/// A folder for one test, and the spec of a stand-in script in it.
fn stand_in(folder: &Path, body: &str) -> ProgramSpec {
    let path = folder.join("stand-in.sh");
    std::fs::write(&path, body).expect("the stand-in script of the test");
    ProgramSpec {
        program: PathBuf::from("/bin/sh"),
        args: vec![path.into_os_string()],
        env: Vec::new(),
        env_remove: Vec::new(),
        cwd: folder.to_path_buf(),
    }
}

/// The wake of a bridge, and the number of the times that it was spoken.
fn wake_counter() -> (Arc<dyn Fn() + Send + Sync>, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let woken = Arc::clone(&count);
    let wake = move || {
        woken.fetch_add(1, Ordering::Relaxed);
    };
    (Arc::new(wake), count)
}

/// The bridge of one stand-in script.
fn start(folder: &Path, body: &str, wake: Arc<dyn Fn() + Send + Sync>) -> Bridge {
    Bridge::start(stand_in(folder, body), Box::new(ClaudeCodec::new()), wake).expect("the bridge of the test")
}

/// Collects events until `wanted` holds for them, and answers every event that
/// came.
fn wait_for(bridge: &Bridge, what: &str, wanted: impl Fn(&[BridgeEvent]) -> bool) -> Vec<BridgeEvent> {
    let deadline = Instant::now() + WAIT;
    let mut events = Vec::new();
    while !wanted(&events) {
        events.extend(bridge.poll());
        assert!(Instant::now() < deadline, "the test waited for {what}: {events:?}");
        std::thread::sleep(POLL);
    }
    events
}

/// The events of one whole turn.
fn turn(number: u32, text: &str) -> Vec<BridgeEvent> {
    vec![
        BridgeEvent::SessionStarted {
            session_id: SESSION.to_owned(),
        },
        BridgeEvent::TurnStarted,
        BridgeEvent::AgentText {
            message_id: format!("msg-{number}"),
            text: text.to_owned(),
        },
        BridgeEvent::TurnEnded {
            failed: false,
            duration_ms: Some(5),
            cost_usd: None,
        },
    ]
}

/// Waits until `wanted` holds, and names what the test waited for.
fn wait_until(what: &str, wanted: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !wanted() {
        assert!(Instant::now() < deadline, "the test waited for {what}");
        std::thread::sleep(POLL);
    }
}

#[test]
fn two_turns_of_one_program_arrive_in_order_and_speak_the_wake() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (wake, woken) = wake_counter();
    let bridge = start(folder.path(), TWO_TURNS, wake);
    bridge.send("alpha".to_owned()).expect("the first turn");
    let first = wait_for(&bridge, "the first turn", |events| {
        events
            .iter()
            .any(|event| matches!(event, BridgeEvent::TurnEnded { .. }))
    });
    assert_eq!(first, turn(1, "turn 1"));
    bridge.send("bravo".to_owned()).expect("the second turn");
    let second = wait_for(&bridge, "the second turn", |events| {
        events
            .iter()
            .any(|event| matches!(event, BridgeEvent::TurnEnded { .. }))
    });
    assert_eq!(second, turn(2, "turn 2"));
    assert!(woken.load(Ordering::Relaxed) >= 2, "the wake was spoken");
    bridge.stop();
}

#[test]
fn a_line_above_the_bound_is_dropped_and_the_next_line_parses() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let mut body = String::from("#!/bin/sh\ns='");
    body.push_str(&"a".repeat(100));
    body.push_str("'\n");
    body.push_str("i=0\nwhile [ $i -lt 14 ]; do s=$s$s; i=$((i+1)); done\n");
    body.push_str("printf '%s\\n' \"$s\"\n");
    body.push_str(
        "printf '%s\\n' '{\"type\":\"assistant\",\"message\":{\"id\":\"msg-1\",\"content\":[{\"type\":\"text\",\"text\":\"alpha after\"}]}}'\n",
    );
    body.push_str("read -r line\n");
    let (wake, _) = wake_counter();
    let bridge = start(folder.path(), &body, wake);
    let events = wait_for(&bridge, "the line after the long line", |events| {
        events
            .iter()
            .any(|event| matches!(event, BridgeEvent::AgentText { .. }))
    });
    assert_eq!(
        events,
        vec![
            BridgeEvent::Dropped {
                reason: crate::agent_bridge::DropReason::TooLong
            },
            BridgeEvent::AgentText {
                message_id: "msg-1".to_owned(),
                text: "alpha after".to_owned(),
            },
        ]
    );
    bridge.stop();
}

#[test]
fn the_end_of_the_program_carries_its_code_and_the_last_of_its_words() {
    let body = r#"#!/bin/sh
printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
printf '%s\n' 'the account refused the call' >&2
exit 3
"#;
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (wake, _) = wake_counter();
    let bridge = start(folder.path(), body, wake);
    let events = wait_for(&bridge, "the end of the program", |events| {
        events.iter().any(|event| matches!(event, BridgeEvent::Ended { .. }))
    });
    let Some(BridgeEvent::Ended { exit_code, stderr_tail }) = events.last() else {
        panic!("the last event of the program is its end: {events:?}");
    };
    assert_eq!(*exit_code, Some(3));
    assert!(stderr_tail.contains("refused the call"), "{stderr_tail}");
    bridge.stop();
}

#[test]
fn a_quiet_program_answers_a_poll_at_once() {
    let body = r#"#!/bin/sh
printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
read -r line
"#;
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (wake, _) = wake_counter();
    let bridge = start(folder.path(), body, wake);
    wait_for(&bridge, "the session of the program", |events| !events.is_empty());
    let started = Instant::now();
    let quiet = bridge.poll();
    let elapsed = started.elapsed();
    assert!(quiet.is_empty(), "{quiet:?}");
    assert!(elapsed < Duration::from_millis(50), "the poll took {elapsed:?}");
    bridge.stop();
}

#[test]
fn a_program_that_ignores_the_close_and_the_termination_signal_leaves_no_process() {
    let body = r#"#!/bin/sh
trap '' TERM
printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
while :; do :; done
"#;
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (wake, _) = wake_counter();
    let bridge = start(folder.path(), body, wake);
    let pid = bridge.pid();
    wait_for(&bridge, "the session of the program", |events| !events.is_empty());
    let started = Instant::now();
    bridge.stop();
    let elapsed = started.elapsed();
    assert!(
        elapsed < STOP_TIMEOUT + Duration::from_secs(1),
        "the stop took {elapsed:?}"
    );
    wait_until("the program of the bridge", || !crate::process::is_alive(pid));
}

#[test]
fn a_send_after_the_end_of_the_program_is_an_error() {
    let body = r#"#!/bin/sh
printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
"#;
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (wake, _) = wake_counter();
    let bridge = start(folder.path(), body, wake);
    wait_for(&bridge, "the end of the program", |events| {
        events.iter().any(|event| matches!(event, BridgeEvent::Ended { .. }))
    });
    assert!(matches!(bridge.send("alpha".to_owned()), Err(BridgeError::Closed)));
    bridge.stop();
}

#[test]
fn a_send_refuses_an_empty_text_and_a_text_above_the_bound() {
    let body = r"#!/bin/sh
read -r line
";
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (wake, _) = wake_counter();
    let bridge = start(folder.path(), body, wake);
    assert!(matches!(bridge.send(String::new()), Err(BridgeError::EmptyText)));
    assert!(matches!(
        bridge.send("a".repeat(MAX_TEXT_BYTES + 1)),
        Err(BridgeError::TextTooLong)
    ));
    assert!(bridge.send("a".repeat(MAX_TEXT_BYTES)).is_ok());
    bridge.stop();
}

#[test]
fn an_over_long_result_line_ends_the_turn_that_it_closes() {
    // The measured program repeats a large tool input in the `result` line of a
    // turn, under `permission_denials`, so the one line that ends the turn is
    // above the bound of the reader.
    let body = r#"#!/bin/sh
printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
s='aaaaaaaaaa'
i=0
while [ $i -lt 18 ]; do s=$s$s; i=$((i+1)); done
printf '{"type":"result","permission_denials":[{"tool_input":{"content":"%s"}}],"is_error":true}\n' "$s"
read -r line
"#;
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (wake, _) = wake_counter();
    let bridge = start(folder.path(), body, wake);
    let events = wait_for(&bridge, "the end of the over-long turn", |events| {
        events
            .iter()
            .any(|event| matches!(event, BridgeEvent::TurnEnded { .. }))
    });
    assert_eq!(
        events,
        vec![
            BridgeEvent::SessionStarted {
                session_id: SESSION.to_owned()
            },
            BridgeEvent::TurnStarted,
            BridgeEvent::Dropped {
                reason: crate::agent_bridge::DropReason::TooLong
            },
            BridgeEvent::TurnEnded {
                failed: true,
                duration_ms: None,
                cost_usd: None,
            },
        ]
    );
    bridge.stop();
}

#[test]
fn a_bridge_that_is_dropped_ends_its_program() {
    // The program ignores the close of its standard input: only a signal ends it.
    let body = r#"#!/bin/sh
printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
exec sleep 60
"#;
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (wake, _) = wake_counter();
    let bridge = start(folder.path(), body, wake);
    let pid = bridge.pid();
    wait_for(&bridge, "the session of the program", |events| !events.is_empty());
    drop(bridge);
    wait_until("the program of a dropped bridge", || !crate::process::is_alive(pid));
}

#[test]
fn a_start_that_fails_waits_for_its_program() {
    let child = std::process::Command::new("/bin/sh")
        .args(["-c", "exec sleep 60"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("a stand-in program of the test");
    let pid = child.id();
    let child = Arc::new(Mutex::new(Some(child)));
    let _ = fail_start(&child, pid, BridgeError::Pipes);
    assert!(
        !crate::process::is_alive(pid),
        "a failed start leaves no entry of the process table behind"
    );
}

#[test]
fn a_signal_waits_for_the_identity_of_the_child_when_it_fills_after_the_start() {
    // A system that reads the start time of a process with a program fills the
    // identity of a child after its start. A signal that reads the cell before it
    // fills reaches no process, so it waits for the cell within a small bound.
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "exec sleep 60"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("a stand-in program of the test");
    let pid = child.id();
    let cell = Arc::new(std::sync::OnceLock::new());
    let identity = crate::process::HangingChild::new(pid, Arc::clone(&cell));
    let late = Arc::clone(&cell);
    let filler = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        let _ = late.set(crate::process::start_time(pid));
    });
    end_child(&identity);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut ended = false;
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            ended = true;
            break;
        }
        std::thread::sleep(POLL);
    }
    if !ended {
        let _ = child.kill();
    }
    let _ = child.wait();
    let _ = filler.join();
    assert!(ended, "the signal waited for the identity and ended the child");
}
