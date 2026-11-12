//! A take that a slow link did not carry: the file stays, one retry sends it,
//! and the folder of the takes keeps only the last three takes of a day.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;
use std::time::Duration;

use super::tests::{Take, plan, transcriber_script};
use super::{MAX_TAKE_BYTES, SpeechEventKind, TRANSCRIBE_BOUND, TakeBounds, Transcriber};
use crate::panel::PanelId;
use crate::speech::command::CommandLine;

const TARGET: PanelId = PanelId(7);

#[test]
fn a_take_that_a_slow_transcriber_did_not_answer_stays_for_one_retry() {
    let take = Take::with_bounds(
        Arc::new(()),
        TakeBounds {
            transcribe: Duration::from_millis(200),
            ..TakeBounds::default()
        },
    );
    let prepared = take.prepared();
    let recorder = take.recorder("rec", &prepared);
    let slow = take.script("slow", "#!/bin/sh\nsleep 5\nprintf 'late'\n");
    let transcriber = CommandLine::parse(&format!("{slow} {{file}}")).expect("a line");
    take.input.begin(
        plan(recorder.clone(), Transcriber::Command(transcriber.clone())),
        TARGET,
    );
    take.wait_for_recorder("rec");
    take.clock.advance(Duration::from_secs(1));
    take.input.end();
    let events = take.wait_for_event(&["kept"]);
    assert!(
        matches!(&events[0], SpeechEventKind::Kept(message) if message.contains("did not answer")),
        "{events:?}"
    );
    let kept = take.files();
    assert_eq!(kept.len(), 1, "the take stays for one retry: {kept:?}");
    let kept_path = take.folder().join(&kept[0]);
    assert_eq!(
        fs::metadata(&kept_path).expect("the kept take").permissions().mode() & 0o777,
        0o600,
        "a kept take is private"
    );

    // The service answers at the retry, and the take of the same file is sent.
    let (quick, marker) = transcriber_script(&take, "heisan");
    take.input.retry(plan(recorder, Transcriber::Command(quick)), TARGET);
    let events = take.wait_for_event(&["writing", "text"]);
    assert!(
        matches!(&events[1], SpeechEventKind::Text(text) if text == "heisan"),
        "{events:?}"
    );
    assert!(marker.exists(), "the retry transcribed the kept take");
    take.wait_for("the delete of the sent take", || take.files().is_empty());
}

#[test]
fn the_upload_bound_grows_with_the_size_of_the_take() {
    use super::upload_bound;
    // A take of 640 KB, which is twenty seconds of a 16 kHz recording.
    assert_eq!(
        upload_bound(640 * 1024, super::TRANSCRIBE_BOUND),
        Duration::from_secs(40),
        "the way up of a take at 128 kbit/s holds forty seconds"
    );
    assert_eq!(
        upload_bound(1024, super::TRANSCRIBE_BOUND),
        super::TRANSCRIBE_BOUND,
        "a small take still gets the bound of the answer"
    );
    assert!(
        upload_bound(super::MAX_TAKE_BYTES, super::TRANSCRIBE_BOUND) > Duration::from_secs(600),
        "the largest take gets minutes on a slow uplink"
    );
}

#[test]
fn a_service_that_stays_away_keeps_the_take_and_an_answer_that_refused_does_not() {
    use crate::speech::service::ServiceError;
    for kept in [
        ServiceError::Unreachable {
            host: "speech.example".into(),
        },
        ServiceError::Tls {
            host: "speech.example".into(),
            reason: "certificate".into(),
        },
        ServiceError::NoAnswer {
            host: "speech.example".into(),
            seconds: 30,
        },
        ServiceError::UploadTooSlow {
            host: "speech.example".into(),
            seconds: 40,
        },
    ] {
        assert!(kept.keeps_the_take(), "{kept}");
    }
    for dropped in [
        ServiceError::Refused {
            host: "speech.example".into(),
            status: 401,
            message: "no".into(),
        },
        ServiceError::NoText {
            host: "speech.example".into(),
        },
        ServiceError::EndedEarly {
            host: "speech.example".into(),
        },
        ServiceError::Unresolved {
            host: "speech.example".into(),
        },
    ] {
        assert!(!dropped.keeps_the_take(), "{dropped}");
    }
}
