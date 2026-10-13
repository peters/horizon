use super::*;
use base64::Engine;
use horizon_browser_protocol::cloud_view::{CloudStageChunk, CloudStageReceipt, CloudStagedFiles};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

const BROWSER: &str = "browser-1";
const NAME: &str = "notes 1.txt";

/// A staging store with one complete file `NAME` in each batch.
fn staged(root: &std::path::Path, batches: &[&str]) -> Staging {
    let mut staging = Staging::new(root.join(".uploads")).unwrap();
    for batch in batches {
        stage(&mut staging, batch, NAME).unwrap();
    }
    staging
}

fn stage(staging: &mut Staging, batch: &str, name: &str) -> Result<CloudStageReceipt, String> {
    let bytes = b"synthetic upload";
    let chunk = CloudStageChunk {
        id: BROWSER.into(),
        batch: batch.into(),
        name: name.into(),
        size: bytes.len() as u64,
        offset: 0,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        sha256: Some(Sha256::digest(bytes).iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })),
    };
    staging.stage(&chunk, Instant::now())
}

fn files(batch: &str) -> CloudStagedFiles {
    CloudStagedFiles {
        batch: batch.into(),
        names: vec![NAME.into()],
    }
}

/// Answers `request` with the batch, or cancels it without one.
fn answer(relay: &mut Relay, chooser: &FileChooserHandle, staging: &mut Staging, request: u64, batch: Option<&str>) {
    let files = batch.map(files);
    relay.answer(
        chooser,
        staging,
        BROWSER,
        CloudChooserAnswer { request, files },
        Instant::now(),
    );
}

/// A relay that a host presents, with one pending request.
fn presented(multiple: bool) -> (Relay, FileChooserHandle, u64) {
    let (mut relay, chooser) = (Relay::default(), FileChooserHandle::default());
    relay.presented(Instant::now());
    let id = chooser.open_for_test(multiple, "", "https://upload.test");
    relay.tick(&chooser, Instant::now());
    (relay, chooser, id)
}

#[test]
fn a_chooser_without_an_upload_host_is_cancelled_at_once() {
    let chooser = FileChooserHandle::default();
    let id = chooser.open_for_test(false, "", "https://upload.test");
    Relay::default().tick(&chooser, Instant::now());
    assert_eq!(chooser.take_answer_for_test(id), Some(FileChooserAnswer::Cancel));
}

#[test]
fn a_chooser_waits_while_a_host_polls_and_ends_when_the_host_leaves_or_waits_too_long() {
    let start = Instant::now();
    let (mut relay, chooser, id) = presented(true);
    relay.tick(&chooser, start + Duration::from_secs(9));
    assert_eq!(chooser.take_answer_for_test(id), None);
    relay.tick(&chooser, start + PRESENTER_GRACE + Duration::from_secs(1));
    assert_eq!(chooser.take_answer_for_test(id), Some(FileChooserAnswer::Cancel));

    let (mut relay, chooser, id) = presented(false);
    let mut now = start;
    while now < start + MAX_PENDING.saturating_sub(Duration::from_secs(5)) {
        now += Duration::from_secs(5);
        relay.presented(now);
        relay.tick(&chooser, now);
    }
    assert_eq!(chooser.take_answer_for_test(id), None);
    now += Duration::from_secs(10);
    relay.presented(now);
    relay.tick(&chooser, now);
    assert_eq!(chooser.take_answer_for_test(id), Some(FileChooserAnswer::Cancel));
}

#[test]
fn an_answer_attaches_only_staged_files_and_failures_return_to_the_picker() {
    let root = tempfile::tempdir().unwrap();
    let mut staging = staged(root.path(), &["b1"]);
    let (mut relay, chooser, id) = presented(false);

    answer(&mut relay, &chooser, &mut staging, id, Some("missing"));
    let error = relay.state(&chooser, true, staging.limits()).error.unwrap();
    assert_eq!((error.request, error.sequence), (id, 1));
    assert!(error.message.contains("no longer has these files"));
    assert_eq!(chooser.take_answer_for_test(id), None);

    answer(&mut relay, &chooser, &mut staging, id, Some("b1"));
    let path = root.path().join(".uploads/browser-1/b1").join(NAME);
    assert_eq!(
        chooser.take_answer_for_test(id),
        Some(FileChooserAnswer::Files(vec![path]))
    );
    assert!(
        stage(&mut staging, "b1", "late.txt")
            .unwrap_err()
            .contains("already attached")
    );
    chooser.retry_for_test(id, "The page refused the file");
    relay.tick(&chooser, Instant::now());
    let error = relay.state(&chooser, true, staging.limits()).error.unwrap();
    assert_eq!(
        (error.request, error.sequence, error.message.as_str()),
        (id, 2, "The page refused the file")
    );

    answer(&mut relay, &chooser, &mut staging, id, None);
    assert_eq!(chooser.take_answer_for_test(id), Some(FileChooserAnswer::Cancel));
}

#[test]
fn an_answer_to_a_replaced_chooser_becomes_a_notice_and_never_reaches_the_new_one() {
    let root = tempfile::tempdir().unwrap();
    let mut staging = staged(root.path(), &["b1"]);
    let (mut relay, chooser, first) = presented(false);
    let second = chooser.open_for_test(false, "", "https://upload.test");
    relay.tick(&chooser, Instant::now());
    answer(&mut relay, &chooser, &mut staging, first, Some("b1"));
    let state = relay.state(&chooser, true, staging.limits());
    assert_eq!(state.request.unwrap().id, second);
    assert!(state.error.is_none());
    assert!(state.notice.unwrap().message.contains("closed the file selection"));
    assert_eq!(chooser.take_answer_for_test(second), None);
    assert!(stage(&mut staging, "b1", "more.txt").is_ok(), "no page got the batch");

    answer(&mut relay, &chooser, &mut staging, first, None);
    assert_eq!(relay.notice.as_ref().unwrap().sequence, 1, "a stale cancel is silent");
}

#[test]
fn drops_send_only_staged_worker_paths_and_report_each_failure() {
    let root = tempfile::tempdir().unwrap();
    let mut staging = staged(root.path(), &["b1", "b2"]);
    let mut relay = Relay::default();
    relay.navigated();
    let mut drop_at = |relay: &mut Relay, x: f64, batch: &str, send: &mut dyn FnMut(BrowserCommand) -> bool| {
        let files = files(batch);
        relay.drop_files(
            &mut staging,
            BROWSER,
            &CloudFileDrop {
                page: 1,
                x,
                y: 30.5,
                files,
            },
            Instant::now(),
            send,
        );
    };
    let mut sent = None;
    drop_at(&mut relay, 12.0, "b1", &mut |command| {
        sent = Some(command);
        true
    });
    let Some(BrowserCommand::DropFiles { x, y, paths }) = sent else {
        panic!("the drop must reach the browser");
    };
    assert_eq!((x, y), (12.0, 30.5));
    assert_eq!(paths, [root.path().join(".uploads/browser-1/b1").join(NAME)]);
    assert!(relay.notice.is_none());

    let failures = [
        (f64::NAN, "b2", true, "outside the page"),
        (-1.0, "b2", true, "outside the page"),
        (1.0, "missing", true, "no longer has these files"),
        (1.0, "b2", false, "busy"),
        (1.0, "b2", true, "page changed"),
    ];
    for (sequence, (x, batch, accepted, message)) in failures.into_iter().enumerate() {
        if message == "page changed" {
            relay.navigated();
        }
        drop_at(&mut relay, x, batch, &mut |_| accepted);
        let notice = relay.notice.clone().unwrap();
        assert_eq!(notice.sequence, sequence as u64 + 1);
        assert!(notice.message.starts_with("File drop failed:"), "{}", notice.message);
        assert!(notice.message.contains(message), "{}", notice.message);
    }
}

#[test]
fn state_reports_support_the_pending_request_and_the_limits() {
    let chooser = FileChooserHandle::default();
    let limits = CloudUploadLimits::default();
    let idle = Relay::default().state(&chooser, false, limits);
    assert!(!idle.file_chooser && !idle.drops && idle.request.is_none());
    assert_eq!(idle.version, CLOUD_UPLOAD_VERSION);
    let (relay, chooser, id) = presented(true);
    let pending = relay.state(&chooser, true, limits);
    assert!(pending.file_chooser && pending.drops);
    let request = pending.request.unwrap();
    assert_eq!(
        (request.id, request.multiple, request.origin.as_str()),
        (id, true, "https://upload.test")
    );
    assert_eq!(pending.limits, limits);
}
