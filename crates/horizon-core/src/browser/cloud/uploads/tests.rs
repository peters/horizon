use super::*;
use horizon_browser_protocol::cloud_view::{
    CLOUD_UPLOAD_VERSION, CloudBrowserNotice, CloudChooserError, CloudChooserRequest,
};
use std::cell::RefCell;

/// Starts fake transfers and keeps their result senders.
#[derive(Default)]
struct Starts {
    started: RefCell<Vec<(Vec<PathBuf>, CloudUploadLimits, Purpose)>>,
    finish: RefCell<Vec<mpsc::Sender<Result<CloudStagedFiles, String>>>>,
}

impl Starts {
    fn start(&self, paths: Vec<PathBuf>, limits: CloudUploadLimits, purpose: Purpose) -> Transfer {
        let (finished, done) = mpsc::channel();
        self.started.borrow_mut().push((paths, limits, purpose));
        self.finish.borrow_mut().push(finished);
        Transfer {
            purpose,
            progress: Arc::default(),
            stop: Arc::default(),
            child: Arc::default(),
            done,
        }
    }

    fn complete(&self, result: Result<CloudStagedFiles, String>) {
        self.finish.borrow_mut().remove(0).send(result).unwrap();
    }
}

fn state(request: Option<u64>) -> CloudUploadState {
    CloudUploadState {
        version: CLOUD_UPLOAD_VERSION,
        limits: CloudUploadLimits {
            max_files: 4,
            ..CloudUploadLimits::default()
        },
        file_chooser: true,
        drops: true,
        page: 7,
        request: request.map(|id| CloudChooserRequest {
            id,
            multiple: true,
            accept: ".txt".into(),
            origin: "https://upload.test".into(),
        }),
        error: None,
        notice: None,
    }
}

fn staged(batch: &str) -> CloudStagedFiles {
    CloudStagedFiles {
        batch: batch.into(),
        names: vec!["notes.txt".into()],
    }
}

fn outgoing(relay: &Relay) -> CloudUploadPoll {
    next_poll(&relay.outgoing)
}

#[test]
fn the_worker_request_shows_in_the_panel_picker_until_the_worker_ends_it() {
    let (mut relay, chooser) = (Relay::default(), FileChooserHandle::default());
    assert!(relay.apply(Some(state(None)), &chooser).is_none());
    assert!(chooser.supported() && chooser.request().is_none());
    relay.apply(Some(state(Some(4))), &chooser);
    let shown = chooser.request().unwrap();
    assert_eq!(
        (shown.multiple, shown.accept.as_str(), shown.origin.as_str()),
        (true, ".txt", "https://upload.test")
    );
    relay.apply(Some(state(Some(5))), &chooser);
    let replaced = chooser.request().unwrap();
    assert_ne!(replaced.id, shown.id);
    relay.apply(Some(state(None)), &chooser);
    assert!(chooser.request().is_none());
    relay.apply(None, &chooser);
    assert!(!chooser.supported(), "an older worker relays no chooser");
    relay.apply(Some(state(Some(6))), &chooser);
    relay.close(&chooser);
    assert!(chooser.request().is_none(), "a reconnect mirrors the request again");
}

#[test]
fn picked_files_reach_the_worker_only_after_their_transfer() {
    let (mut relay, chooser, starts) = (Relay::default(), FileChooserHandle::default(), Starts::default());
    relay.apply(Some(state(Some(4))), &chooser);
    let host = chooser.request().unwrap().id;
    assert!(chooser.respond(host, FileChooserAnswer::Files(vec!["/local/notes.txt".into()])));
    relay.take_answer(&chooser, &|paths, limits, purpose| starts.start(paths, limits, purpose));
    let (paths, limits, purpose) = starts.started.borrow()[0].clone();
    assert_eq!(paths, [PathBuf::from("/local/notes.txt")]);
    assert_eq!(limits.max_files, 4);
    assert_eq!(purpose, Purpose::Answer { worker: 4, host });
    assert!(outgoing(&relay).answer.is_none(), "nothing before the files arrived");
    assert!(relay.progress().is_some());

    starts.complete(Ok(staged("b1")));
    assert!(relay.finish(&chooser).is_none());
    let answer = outgoing(&relay).answer.unwrap();
    assert_eq!((answer.request, answer.files), (4, Some(staged("b1"))));
    assert!(outgoing(&relay).answer.is_none(), "one poll takes the answer");
    assert!(relay.progress().is_none());
}

#[test]
fn failures_return_to_the_picker_and_cancels_reach_the_worker() {
    let (mut relay, chooser, starts) = (Relay::default(), FileChooserHandle::default(), Starts::default());
    relay.apply(Some(state(Some(4))), &chooser);
    let host = chooser.request().unwrap().id;
    let start = |paths, limits, purpose| starts.start(paths, limits, purpose);
    assert!(chooser.respond(host, FileChooserAnswer::Files(vec!["/local/a".into()])));
    relay.take_answer(&chooser, &start);
    starts.complete(Err("Cannot read /local/a".into()));
    relay.finish(&chooser);
    assert_eq!(chooser.take_error().as_deref(), Some("Cannot read /local/a"));
    assert!(outgoing(&relay).answer.is_none());

    let mut failed = state(Some(4));
    failed.error = Some(CloudChooserError {
        request: 4,
        sequence: 1,
        message: "The page refused the file".into(),
    });
    relay.apply(Some(failed), &chooser);
    assert_eq!(chooser.take_error().as_deref(), Some("The page refused the file"));

    assert!(chooser.respond(host, FileChooserAnswer::Cancel));
    relay.take_answer(&chooser, &start);
    let cancel = outgoing(&relay).answer.unwrap();
    assert_eq!((cancel.request, cancel.files), (4, None));
}

#[test]
fn a_transfer_for_a_replaced_request_never_answers_the_new_one() {
    let (mut relay, chooser, starts) = (Relay::default(), FileChooserHandle::default(), Starts::default());
    relay.apply(Some(state(Some(4))), &chooser);
    let host = chooser.request().unwrap().id;
    assert!(chooser.respond(host, FileChooserAnswer::Files(vec!["/local/a".into()])));
    relay.take_answer(&chooser, &|paths, limits, purpose| starts.start(paths, limits, purpose));
    relay.apply(Some(state(Some(5))), &chooser);
    assert!(relay.progress().is_none(), "the replaced request stops its transfer");
    assert!(starts.finish.borrow()[0].send(Ok(staged("b1"))).is_err());
    assert!(relay.finish(&chooser).is_none());
    assert!(outgoing(&relay).answer.is_none());
}

#[test]
fn drops_carry_the_page_that_the_person_saw_and_report_failures() {
    let (mut relay, chooser, starts) = (Relay::default(), FileChooserHandle::default(), Starts::default());
    let start = |paths, limits, purpose| starts.start(paths, limits, purpose);
    assert!(
        relay
            .drop_files(vec!["/local/a".into()], 1.0, 2.0, &start)
            .unwrap_err()
            .contains("connected")
    );
    let mut older = state(None);
    older.drops = false;
    relay.apply(Some(older), &chooser);
    assert!(
        relay
            .drop_files(vec!["/local/a".into()], 1.0, 2.0, &start)
            .unwrap_err()
            .contains("newer image")
    );
    relay.apply(None, &chooser);
    assert!(
        relay
            .drop_files(vec!["/local/a".into()], 1.0, 2.0, &start)
            .unwrap_err()
            .contains("newer image"),
        "a state without uploads comes from a worker that predates them"
    );
    relay.apply(Some(state(None)), &chooser);
    relay.drop_files(vec!["/local/a".into()], 1.0, 2.0, &start).unwrap();
    assert!(
        relay
            .drop_files(vec!["/local/b".into()], 1.0, 2.0, &start)
            .unwrap_err()
            .contains("current upload")
    );
    starts.complete(Ok(staged("b1")));
    assert!(relay.finish(&chooser).is_none());
    let drop = outgoing(&relay).drop.unwrap();
    assert_eq!((drop.page, drop.x, drop.y, drop.files), (7, 1.0, 2.0, staged("b1")));

    relay.drop_files(vec!["/local/a".into()], 1.0, 2.0, &start).unwrap();
    starts.complete(Err("Cannot read /local/a".into()));
    assert_eq!(
        relay.finish(&chooser).as_deref(),
        Some("File drop failed: Cannot read /local/a")
    );
    relay.drop_files(vec!["/local/a".into()], 1.0, 2.0, &start).unwrap();
    starts.finish.borrow_mut().clear();
    assert_eq!(
        relay.finish(&chooser).as_deref(),
        Some("File drop failed: The upload stopped")
    );
}

#[test]
fn notices_and_errors_show_once_and_never_again_after_a_reconnect() {
    let (mut relay, chooser) = (Relay::default(), FileChooserHandle::default());
    let mut earlier = state(Some(4));
    earlier.notice = Some(CloudBrowserNotice {
        sequence: 3,
        message: "An earlier notice".into(),
    });
    earlier.error = Some(CloudChooserError {
        request: 4,
        sequence: 2,
        message: "An earlier error".into(),
    });
    assert!(relay.apply(Some(earlier.clone()), &chooser).is_none());
    assert!(chooser.take_error().is_none());
    let mut next = earlier.clone();
    next.notice = Some(CloudBrowserNotice {
        sequence: 4,
        message: format!("File drop failed\u{7}: {}", "x".repeat(500)),
    });
    let shown = relay.apply(Some(next.clone()), &chooser).unwrap();
    assert!(shown.starts_with("File drop failed: ") && shown.chars().count() <= 301);
    assert!(relay.apply(Some(next), &chooser).is_none());
}
