use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel, sync_channel};

use horizon_core::{PanelId, SpeechHotkeyMode};

use super::super::capture::{CaptureCmd, CaptureHandle};
use super::super::worker::{Job, WorkerEvent, WorkerHandle};
use super::{MIN_PCM_SAMPLES, ProfileRuntime, SpeechEvent, SpeechSystem, State};

struct Harness {
    capture_cmd_rx: Receiver<CaptureCmd>,
    pcm_tx: Sender<(u64, Result<Vec<f32>, String>)>,
    job_rx: Receiver<Job>,
    worker_event_tx: Sender<WorkerEvent>,
    _other_job_rx: Receiver<Job>,
    _other_worker_event_tx: Sender<WorkerEvent>,
}

fn test_system() -> (SpeechSystem, Harness) {
    let (capture_cmd_tx, capture_cmd_rx) = channel();
    let (pcm_tx, pcm_rx) = channel();
    let capture = CaptureHandle::from_test_channels(capture_cmd_tx, pcm_rx);
    let (job_tx, job_rx) = sync_channel(1);
    let (worker_event_tx, worker_event_rx) = channel();
    let worker = WorkerHandle::from_test_channels(job_tx, worker_event_rx);
    let (other_job_tx, other_job_rx) = sync_channel(1);
    let (other_worker_event_tx, other_worker_event_rx) = channel();
    let other_worker = WorkerHandle::from_test_channels(other_job_tx, other_worker_event_rx);
    let speech = SpeechSystem {
        capture,
        profiles: vec![
            ProfileRuntime {
                label: "Test".to_string(),
                binding: None,
                worker,
            },
            ProfileRuntime {
                label: "Other".to_string(),
                binding: None,
                worker: other_worker,
            },
        ],
        resolved_bindings: Vec::new(),
        state: State::Idle,
        hotkey_mode: SpeechHotkeyMode::Hold,
        generation: 0,
        active_backend: None,
        last_used: 0,
    };
    (
        speech,
        Harness {
            capture_cmd_rx,
            pcm_tx,
            job_rx,
            worker_event_tx,
            _other_job_rx: other_job_rx,
            _other_worker_event_tx: other_worker_event_tx,
        },
    )
}

fn start_and_stop(speech: &mut SpeechSystem, harness: &Harness, target: PanelId) -> u64 {
    speech.start(target, 0);
    let generation = speech.generation;
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("start command"),
        CaptureCmd::Start(value) if value == generation
    ));
    assert_eq!(speech.state, State::Recording { target, profile: 0 });

    speech.stop();
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("stop command"),
        CaptureCmd::Stop
    ));
    assert_eq!(speech.state, State::AwaitingPcm { target, profile: 0 });
    generation
}

fn submit_recording(speech: &mut SpeechSystem, harness: &Harness, target: PanelId) -> Job {
    let generation = start_and_stop(speech, harness, target);
    harness
        .pcm_tx
        .send((generation, Ok(vec![0.0; MIN_PCM_SAMPLES])))
        .expect("queue PCM");
    assert!(speech.poll().is_empty());
    let job = harness.job_rx.try_recv().expect("worker job");
    assert_eq!(job.target, target);
    assert_eq!(job.generation, generation);
    assert_eq!(job.pcm.len(), MIN_PCM_SAMPLES);
    assert_eq!(
        speech.state,
        State::Transcribing {
            target,
            profile: 0,
            generation,
        }
    );
    job
}

#[test]
fn short_tap_stops_and_returns_to_idle_without_worker_job() {
    let (mut speech, harness) = test_system();
    let generation = start_and_stop(&mut speech, &harness, PanelId(7));
    harness
        .pcm_tx
        .send((generation, Ok(vec![0.0; MIN_PCM_SAMPLES - 1])))
        .expect("queue short PCM");

    assert!(speech.poll().is_empty());
    assert_eq!(speech.state, State::Idle);
    assert!(matches!(harness.job_rx.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn cancel_returns_to_idle_from_recording_and_awaiting_pcm() {
    let (mut speech, harness) = test_system();
    let target = PanelId(7);

    speech.start(target, 0);
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("start command"),
        CaptureCmd::Start(1)
    ));
    speech.cancel();
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("recording cancel"),
        CaptureCmd::Cancel
    ));
    assert_eq!(speech.state, State::Idle);

    start_and_stop(&mut speech, &harness, target);
    speech.cancel();
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("awaiting-PCM cancel"),
        CaptureCmd::Cancel
    ));
    assert_eq!(speech.state, State::Idle);
}

#[test]
fn start_is_ignored_while_busy_and_for_unknown_profiles() {
    let (mut speech, harness) = test_system();
    let first = PanelId(1);
    let second = PanelId(2);

    speech.start(first, 0);
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("first start"),
        CaptureCmd::Start(1)
    ));
    speech.start(second, 1);
    assert_eq!(
        speech.state,
        State::Recording {
            target: first,
            profile: 0
        }
    );
    assert!(matches!(harness.capture_cmd_rx.try_recv(), Err(TryRecvError::Empty)));

    speech.cancel();
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("cancel"),
        CaptureCmd::Cancel
    ));
    speech.start(second, 99);
    assert_eq!(speech.state, State::Idle);
    assert!(matches!(harness.capture_cmd_rx.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn toggle_stops_only_the_panel_that_owns_the_recording() {
    let (mut speech, harness) = test_system();
    let target = PanelId(3);

    speech.toggle(target);
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("start command"),
        CaptureCmd::Start(1)
    ));
    speech.toggle(PanelId(4));
    assert_eq!(speech.state, State::Recording { target, profile: 0 });
    assert!(matches!(harness.capture_cmd_rx.try_recv(), Err(TryRecvError::Empty)));

    speech.toggle(target);
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("stop command"),
        CaptureCmd::Stop
    ));
    assert_eq!(speech.state, State::AwaitingPcm { target, profile: 0 });
}

#[test]
fn mic_button_reuses_the_last_hotkey_profile() {
    let (mut speech, harness) = test_system();
    let target = PanelId(5);

    speech.start(target, 1);
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("profile start"),
        CaptureCmd::Start(1)
    ));
    speech.cancel();
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("cancel"),
        CaptureCmd::Cancel
    ));

    speech.toggle(target);
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("toggle start"),
        CaptureCmd::Start(2)
    ));
    assert_eq!(speech.state, State::Recording { target, profile: 1 });
}

#[test]
fn current_capture_error_cancels_recording_and_returns_to_idle() {
    let (mut speech, harness) = test_system();
    let target = PanelId(7);
    speech.start(target, 0);
    let generation = speech.generation;
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("start command"),
        CaptureCmd::Start(value) if value == generation
    ));
    harness
        .pcm_tx
        .send((generation, Err("microphone disconnected".to_string())))
        .expect("queue capture error");

    let events = speech.poll();

    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("cancel command"),
        CaptureCmd::Cancel
    ));
    assert_eq!(speech.state, State::Idle);
    assert!(matches!(
        events.as_slice(),
        [SpeechEvent::Error(message)] if message == "microphone disconnected"
    ));
    assert!(matches!(harness.job_rx.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn disconnected_capture_worker_returns_recording_to_idle() {
    let (mut speech, harness) = test_system();
    speech.start(PanelId(7), 0);
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("start command"),
        CaptureCmd::Start(1)
    ));
    drop(harness.pcm_tx);

    let events = speech.poll();

    assert_eq!(speech.state, State::Idle);
    assert!(matches!(
        events.as_slice(),
        [SpeechEvent::Error(message)] if message == "speech capture worker stopped unexpectedly"
    ));
}

#[test]
fn auto_finalized_pcm_while_recording_is_submitted() {
    let (mut speech, harness) = test_system();
    let target = PanelId(7);
    speech.start(target, 0);
    let generation = speech.generation;
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("start command"),
        CaptureCmd::Start(value) if value == generation
    ));
    harness
        .pcm_tx
        .send((generation, Ok(vec![0.0; MIN_PCM_SAMPLES])))
        .expect("queue auto-finalized PCM");

    assert!(speech.poll().is_empty());

    let job = harness.job_rx.try_recv().expect("worker job");
    assert_eq!(job.target, target);
    assert_eq!(job.generation, generation);
    assert_eq!(job.pcm.len(), MIN_PCM_SAMPLES);
    assert_eq!(
        speech.state,
        State::Transcribing {
            target,
            profile: 0,
            generation,
        }
    );
    assert!(matches!(harness.capture_cmd_rx.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn matching_worker_success_and_failure_complete_the_active_generation() {
    let (mut speech, harness) = test_system();
    let target = PanelId(7);
    let job = submit_recording(&mut speech, &harness, target);
    harness
        .worker_event_tx
        .send(WorkerEvent::ModelLoaded {
            generation: job.generation,
            backend: "Metal".to_string(),
        })
        .expect("queue backend");
    harness
        .worker_event_tx
        .send(WorkerEvent::Done {
            target,
            generation: job.generation,
            text: "hello".to_string(),
        })
        .expect("queue result");

    let events = speech.poll();
    assert_eq!(speech.active_backend(), Some("Metal"));
    assert_eq!(speech.state, State::Idle);
    assert!(matches!(
        events.as_slice(),
        [SpeechEvent::Text { target: event_target, text }]
            if *event_target == target && text == "hello"
    ));

    let job = submit_recording(&mut speech, &harness, target);
    harness
        .worker_event_tx
        .send(WorkerEvent::Failed {
            target,
            generation: job.generation,
            message: "failed".to_string(),
        })
        .expect("queue failure");
    let events = speech.poll();
    assert_eq!(speech.state, State::Idle);
    assert!(matches!(events.as_slice(), [SpeechEvent::Error(message)] if message == "failed"));
}

#[test]
fn stale_worker_completion_cannot_finish_or_emit_into_new_generation() {
    let (mut speech, harness) = test_system();
    let target = PanelId(7);
    let old_job = submit_recording(&mut speech, &harness, target);
    speech.cancel();
    assert_eq!(speech.state, State::Idle);
    assert!(speech.profiles[0].worker.generation_is_cancelled(old_job.generation));
    let current_job = submit_recording(&mut speech, &harness, target);

    for event in [
        WorkerEvent::ModelLoaded {
            generation: old_job.generation,
            backend: "stale backend".to_string(),
        },
        WorkerEvent::Done {
            target,
            generation: old_job.generation,
            text: "stale".to_string(),
        },
        WorkerEvent::Failed {
            target,
            generation: old_job.generation,
            message: "stale failure".to_string(),
        },
    ] {
        harness.worker_event_tx.send(event).expect("queue stale event");
    }
    assert!(speech.poll().is_empty());
    assert_eq!(speech.active_backend(), None);
    assert_eq!(
        speech.state,
        State::Transcribing {
            target,
            profile: 0,
            generation: current_job.generation,
        }
    );

    harness
        .worker_event_tx
        .send(WorkerEvent::Done {
            target,
            generation: current_job.generation,
            text: "current".to_string(),
        })
        .expect("queue current result");
    let events = speech.poll();
    assert!(matches!(
        events.as_slice(),
        [SpeechEvent::Text { target: event_target, text }]
            if *event_target == target && text == "current"
    ));
    assert_eq!(speech.state, State::Idle);
}

#[test]
fn disconnected_worker_returns_active_transcription_to_idle() {
    let (mut speech, harness) = test_system();
    submit_recording(&mut speech, &harness, PanelId(7));
    drop(harness.worker_event_tx);

    let events = speech.poll();

    assert_eq!(speech.state, State::Idle);
    assert!(matches!(
        events.as_slice(),
        [SpeechEvent::Error(message)]
            if message == "speech transcription worker stopped unexpectedly"
    ));
}

#[test]
fn disconnected_worker_rejects_submission_without_leaving_busy_state() {
    let (mut speech, harness) = test_system();
    let generation = start_and_stop(&mut speech, &harness, PanelId(7));
    drop(harness.job_rx);
    harness
        .pcm_tx
        .send((generation, Ok(vec![0.0; MIN_PCM_SAMPLES])))
        .expect("queue PCM");

    let events = speech.poll();

    assert_eq!(speech.state, State::Idle);
    assert!(matches!(
        events.as_slice(),
        [SpeechEvent::Error(message)]
            if message == "speech transcription worker stopped unexpectedly"
    ));
}

#[test]
fn full_worker_queue_is_bounded_and_returns_to_idle() {
    let (mut speech, harness) = test_system();
    let target = PanelId(7);
    let first_generation = start_and_stop(&mut speech, &harness, target);
    harness
        .pcm_tx
        .send((first_generation, Ok(vec![0.0; MIN_PCM_SAMPLES])))
        .expect("queue first PCM");
    assert!(speech.poll().is_empty());
    assert!(matches!(speech.state, State::Transcribing { .. }));

    speech.cancel();
    let second_generation = start_and_stop(&mut speech, &harness, target);
    harness
        .pcm_tx
        .send((second_generation, Ok(vec![0.0; MIN_PCM_SAMPLES])))
        .expect("queue second PCM");

    let events = speech.poll();

    assert_eq!(speech.state, State::Idle);
    assert!(matches!(
        events.as_slice(),
        [SpeechEvent::Error(message)]
            if message == "speech transcriber is still stopping a previous recording"
    ));
}

#[test]
fn cancelled_capture_generation_is_ignored_after_restart() {
    let (mut speech, harness) = test_system();
    let target = PanelId(7);
    speech.start(target, 0);
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("first start"),
        CaptureCmd::Start(1)
    ));
    speech.cancel();
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("cancel"),
        CaptureCmd::Cancel
    ));

    speech.start(target, 0);
    assert!(matches!(
        harness.capture_cmd_rx.try_recv().expect("second start"),
        CaptureCmd::Start(2)
    ));
    harness
        .pcm_tx
        .send((1, Ok(vec![0.0; MIN_PCM_SAMPLES])))
        .expect("queue stale PCM");
    assert!(speech.poll().is_empty());
    assert_eq!(speech.state, State::Recording { target, profile: 0 });
    assert!(matches!(harness.job_rx.try_recv(), Err(TryRecvError::Empty)));
}
