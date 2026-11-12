//! The slot of one picture, the queue of one size, and the sentences of a viewer.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use horizon_vnc::Wake;
use horizon_vnc::{Status, ViewOptions};

use super::{MAX_LABEL_CHARS, Shared, State, VIEW_DEBOUNCE, ViewQueue};
use crate::picture::Header;
use crate::picture::header_bytes;

/// One shared state whose wakes a test counts.
fn shared() -> (Arc<Shared>, Arc<AtomicUsize>) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&wakes);
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        taken: std::sync::Condvar::new(),
        wake: Box::new(move |_wake: Wake| {
            counted.fetch_add(1, Ordering::SeqCst);
        }),
        latest_full: Mutex::new(None),
    });
    (shared, wakes)
}

/// One picture of `sequence` at `width` by `height`.
fn picture(sequence: u64, width: u32, height: u32) -> crate::Picture {
    let header = Header {
        sequence,
        captured_at_micros: 1,
        width,
        height,
    };
    crate::Picture {
        header,
        planes: vec![128; (width as usize * height as usize) * 3 / 2],
    }
}

#[test]
fn a_picture_that_the_frame_did_not_take_is_replaced_without_a_second_convert() {
    let (shared, wakes) = shared();
    // The first picture is converted at once, because no picture waits.
    assert!(
        !shared.publish_picture(picture(0, 16, 16)),
        "the first picture is converted"
    );
    assert_eq!(shared.lock().received_frame_sequence, 1);
    let converted = wakes.load(Ordering::SeqCst);
    assert!(converted > 0, "a new picture asks the UI for one frame");

    // Two pictures arrive while the frame has not taken the first one. Neither is
    // converted: the newest replaces the one before it, so a slow frame never
    // pays for a picture that nobody shows.
    assert!(shared.publish_picture(picture(1, 16, 16)), "a picture waits");
    assert!(shared.publish_picture(picture(2, 16, 16)), "a picture waits");
    assert_eq!(shared.lock().received_frame_sequence, 1, "one convert, not three");
    assert_eq!(shared.lock().raw.as_ref().map(|raw| raw.header.sequence), Some(2));

    // The frame takes the first picture, and the newest one is converted for the
    // frame after it.
    let updates = shared_updates(&shared);
    assert_eq!(updates.image.map(|image| image.frame.size()), Some([16, 16]));
    shared.convert_waiting().expect("the picture that waited");
    assert_eq!(shared.lock().received_frame_sequence, 2);
    let updates = shared_updates(&shared);
    assert!(updates.image.is_some(), "the newest picture is the next one shown");
    assert!(
        shared_updates(&shared).image.is_none(),
        "an old picture never follows a newer one"
    );
}

/// The waiting image and status of one shared state, as a frame takes them.
fn shared_updates(shared: &Arc<Shared>) -> super::Updates {
    let mut state = shared.lock();
    let updates = super::Updates {
        image: state.image.take(),
        status: state.status.take(),
        received_frame_sequence: state.received_frame_sequence,
        desktop: state.desktop,
        server_name: state.server_name.clone(),
        input: state.hello.clone(),
        stats: state.stats,
        input_error: state.input_error.clone(),
    };
    if updates.image.is_some() {
        state.image_taken = true;
    }
    drop(state);
    shared.taken.notify_all();
    updates
}

#[test]
fn the_size_of_the_viewer_is_asked_for_once_it_was_stable() {
    let mut queue = ViewQueue::default();
    let start = Instant::now();
    queue.offer([640, 480], start);
    assert_eq!(
        queue.due(start + Duration::from_millis(100)),
        None,
        "the size is not stable yet"
    );
    assert_eq!(
        queue.deadline(start + Duration::from_millis(100)),
        Some(Duration::from_millis(50))
    );
    // A drag that changes the size again starts the wait again.
    queue.offer([800, 600], start + Duration::from_millis(120));
    assert_eq!(queue.due(start + Duration::from_millis(200)), None);
    assert_eq!(
        queue.due(start + VIEW_DEBOUNCE + Duration::from_millis(120)),
        Some([800, 600]),
        "one request for the size that the drag ended at"
    );
    assert_eq!(queue.due(start + VIEW_DEBOUNCE * 2), None, "one size is one request");
    queue.offer([800, 600], start + VIEW_DEBOUNCE * 3);
    assert_eq!(
        queue.due(start + VIEW_DEBOUNCE * 4),
        None,
        "the size that was asked for is no news"
    );
    queue.offer([1024, 768], start + VIEW_DEBOUNCE * 4);
    assert_eq!(
        queue.due(start + VIEW_DEBOUNCE * 5),
        Some([1024, 768]),
        "a later size is one more request"
    );
}

#[test]
fn a_status_of_the_program_is_the_one_sentence_of_a_panel() {
    let (shared, wakes) = shared();
    let before = wakes.load(Ordering::SeqCst);
    shared.publish_status(Status::disconnected(
        "frame_source_standin is not installed on this computer",
    ));
    assert!(
        wakes.load(Ordering::SeqCst) > before,
        "a status asks the UI for one frame"
    );
    let updates = shared_updates(&shared);
    assert!(matches!(updates.status, Some(Status::Disconnected { .. })));
    assert_eq!(MAX_LABEL_CHARS, 200);
}

#[test]
fn a_picture_that_the_options_refuse_is_a_failure_of_the_viewer() {
    let (shared, _wakes) = shared();
    shared.lock().options = ViewOptions {
        max_width: 0,
        ..Default::default()
    };
    assert!(!shared.publish_picture(picture(0, 16, 16)));
    assert!(matches!(shared.lock().status, Some(Status::Disconnected { .. })));
}

#[test]
fn the_header_bytes_of_one_picture_are_read_back_as_that_header() {
    let header = Header {
        sequence: 9,
        captured_at_micros: 1_000,
        width: 640,
        height: 480,
    };
    assert_eq!(Header::parse(&header_bytes(&header)), Ok(header));
}

#[test]
fn a_snapshot_of_the_names_of_the_control_messages_of_a_program() {
    // The reader of the control stream is covered where it frames a message; this
    // proves that a session's own copy of a message is the one that is read.
    use crate::control::{Message, Phase, State as ControlState, decode};
    let body = serde_json::to_vec(&crate::control::state_body(&ControlState {
        phase: Phase::Ended,
        reason: Some("the source ended".into()),
    }))
    .expect("one message");
    assert!(matches!(
        decode(&body),
        Ok(Message::State(ControlState {
            phase: Phase::Ended,
            ..
        }))
    ));
}
