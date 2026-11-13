//! A take whose recorder ends before the hotkey comes up.
//!
//! A recorder that cannot record answers one line and ends at once, and the person
//! must read that line while the key is still down: a mark that keeps saying
//! `Listening` makes the person talk into a recorder that is already gone.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::tests::{TARGET, Take, plan, transcriber_script};
use super::{SpeechEventKind, Transcriber};
use crate::speech::command::CommandLine;

/// A recorder that ends on its own is a take that ends while the hotkey is still
/// down: the person reads the sentence at once and not after the release, so no
/// second of speech goes into a recorder that is already gone.
#[test]
fn a_recorder_that_ends_while_the_key_is_held_fails_the_take_without_a_release() {
    let take = Take::new(Arc::new(()));
    let prepared = take.prepared();
    let pid = take.pid_file("slow");
    let program = take.script(
        "slow",
        &format!(
            "#!/bin/sh\ncp '{}' \"$1\"\nprintf '%s\\n' \"$$\" > '{}'\nsleep 0.3\nprintf 'the sound server is away\\n' >&2\nexit 1\n",
            prepared.display(),
            pid.display()
        ),
    );
    let recorder = CommandLine::parse(&format!("{program} {{file}}")).expect("a line");
    let (transcriber, _marker) = transcriber_script(&take, "never");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    take.wait_for_recorder("slow");
    // No key event follows: the recorder ends by itself, three tenths of a second
    // after it started, and the worker reads that on its next look at the child.
    let started = Instant::now();
    let events = take.wait_for_event(&["failed"]);
    let noticed = started.elapsed();
    let SpeechEventKind::Failed(message) = &events[0] else {
        panic!("a failure: {events:?}");
    };
    assert!(message.contains("ended early"), "{message}");
    assert!(message.contains("the sound server is away"), "{message}");
    assert!(
        noticed < Duration::from_secs(1),
        "the person reads the failure while the key is still down, and not after {noticed:?}"
    );
    // The release of a key whose take already ended does nothing more: no second
    // event, no transcription and no file of a take.
    take.input.end();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(take.events(), Vec::new(), "a later release adds no event");
    assert!(take.files().is_empty(), "the file of the take is gone");
}
