//! The claim of Device panel requests runs on the coordination worker, and the
//! frame or pump wake that follows applies each request it took, in claim order.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::super::claim::ClaimedRequest;
use super::*;
use horizon_core::{
    Panel, PanelContent,
    browser::{BrowserPanelState, BrowserStatus},
};

/// Queues one request for the first agent panel of the fixture board.
fn enqueue(root: &Path, app: &HorizonApp, operation: Operation, timeout: Duration) -> Request {
    let actor = format!("horizon:{}", app.board.panels[0].local_id);
    let identity = manifest::AgentIdentity::new(&actor, Some(manifest::host_instance()));
    device::enqueue_at(root, identity, operation, timeout).expect("enqueue")
}

/// Applies the outcomes of the coordination worker and every claimed request,
/// as the frames and pump wakes that follow one another do.
fn settle(app: &mut HorizonApp, ctx: &Context) {
    app.settle_browser_host_io();
    app.drain_claimed_device_requests(ctx);
    app.settle_browser_host_io();
}

/// Answers the queue's requests until `request` has one, within a bound.
fn answer(root: &Path, request: &Request, mut poll: impl FnMut()) -> Outcome {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        poll();
        if let Some(outcome) = device::take_result_at(root, request).expect("result") {
            return outcome;
        }
        assert!(Instant::now() < deadline, "the request was never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A file lock that a helper thread holds until the test releases it, as the
/// queue lock of a coordination worker that is busy does.
#[cfg(unix)]
fn hold_lock(path: &Path) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    std::fs::create_dir_all(path.parent().expect("lock directory")).expect("lock directory");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .expect("lock file");
    file.lock().expect("hold the lock");
    let (release, released) = mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _held = file;
        // The bound keeps a failing assertion from leaving the lock held.
        let _ = released.recv_timeout(Duration::from_secs(10));
    });
    (release, holder)
}

#[cfg(unix)]
#[test]
fn a_held_coordination_lock_does_not_stall_the_frame_that_claims() {
    let root = tempfile::tempdir().expect("temp dir");
    let (_temp, ctx, mut app) = app();
    let queued = enqueue(root.path(), &app, Operation::List, Duration::from_secs(5));
    let (release, holder) = hold_lock(&root.path().join("runtime/device-panel-requests/queue.json.lock"));

    let started = Instant::now();
    app.claim_device_requests(Some(root.path()));
    let elapsed = started.elapsed();
    let claiming = app.device_claims_outstanding();
    // Release the queue before any assertion can fail, so the claim can end.
    release.send(()).expect("the lock holder waits");
    assert!(
        elapsed < Duration::from_millis(1500),
        "the claim stalled the frame: {elapsed:?}"
    );
    assert!(claiming, "the claim runs on the coordination worker");
    settle(&mut app, &ctx);
    holder.join().expect("the lock holder ends");

    // A claim that could not take the queue takes nothing, so a later one does.
    let outcome = answer(root.path(), &queued, || {
        app.claim_device_requests(Some(root.path()));
        settle(&mut app, &ctx);
    });
    assert!(matches!(outcome, Outcome::Panels { .. }));
}

/// A session switch resets the render caches. The requests a claim took and
/// the answers the host owes for them are not render caches: a reset that
/// dropped them would lose the old board's answers and zero the count of a
/// write that still runs.
#[test]
fn a_render_cache_reset_keeps_the_claim_and_the_answers_it_owes() {
    let root = tempfile::tempdir().expect("temp dir");
    let (_temp, ctx, mut app) = app();
    enqueue(root.path(), &app, Operation::List, Duration::from_secs(5));
    let (release, released) = mpsc::channel::<()>();
    app.browser_create_host.io.write(move || {
        let _ = released.recv();
    });

    app.claim_device_requests(Some(root.path()));
    assert!(app.device_claims_outstanding(), "the claim runs on the worker");
    app.panel_render_caches = crate::app::PanelRenderCaches::default();
    assert!(
        app.device_claims_outstanding(),
        "the reset of the render caches keeps the claim and the answers it owes"
    );

    release.send(()).expect("release the coordination worker");
    settle(&mut app, &ctx);
    assert!(
        !app.device_claims_outstanding(),
        "the claim settled and its request was answered"
    );
}

#[test]
fn a_claimed_request_applies_only_after_the_worker_releases_it() {
    let root = tempfile::tempdir().expect("temp dir");
    let (_temp, ctx, mut app) = app();
    let queued = enqueue(root.path(), &app, Operation::List, Duration::from_secs(5));
    let (release, released) = mpsc::channel::<()>();
    app.browser_create_host.io.write(move || {
        let _ = released.recv();
    });

    let started = Instant::now();
    app.claim_device_requests(Some(root.path()));
    assert!(!app.drain_claimed_device_requests(&ctx), "nothing was claimed yet");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the frame waited for the coordination worker"
    );
    assert!(
        device::take_result_at(root.path(), &queued).expect("result").is_none(),
        "nothing applies before the claim ends"
    );

    release.send(()).expect("the worker waits");
    let outcome = answer(root.path(), &queued, || settle(&mut app, &ctx));
    assert!(matches!(outcome, Outcome::Panels { .. }));
}

#[test]
fn two_claimed_requests_apply_in_claim_order() {
    for (first_visible, second_visible) in [(true, false), (false, true)] {
        let root = tempfile::tempdir().expect("temp dir");
        let (_temp, ctx, mut app) = app();
        let create = request(
            &app,
            Operation::Create {
                endpoint: "127.0.0.1:5900".into(),
                identity: None,
                ssh: None,
            },
        );
        let panel = one(app.apply_device_request(&create, &ctx));
        let operation = |visible| Operation::Visibility {
            panel_id: panel.panel_id.clone(),
            visible,
        };
        let claimed = [
            enqueue(root.path(), &app, operation(first_visible), Duration::from_secs(5)),
            enqueue(root.path(), &app, operation(second_visible), Duration::from_secs(5)),
        ];
        app.device_claim.claimed = claimed
            .iter()
            .map(|request| ClaimedRequest {
                root: Some(root.path().to_path_buf()),
                request: request.clone(),
                browser_manifest: None,
            })
            .collect();

        assert!(app.drain_claimed_device_requests(&ctx));
        app.settle_browser_host_io();
        let shown: Vec<_> = claimed
            .iter()
            .map(|request| {
                match device::take_result_at(root.path(), request)
                    .expect("result")
                    .expect("answer")
                {
                    Outcome::Panels { panels } => panels[0].visible,
                    other => panic!("expected an observation: {other:?}"),
                }
            })
            .collect();
        assert_eq!(shown, [first_visible, second_visible], "the answers follow claim order");
        let id = app.board.panel_id_by_local_id(&panel.panel_id).expect("viewer");
        assert_eq!(
            app.board.panel(id).expect("viewer").visible,
            second_visible,
            "the last request the claim took is the one that stands"
        );
    }
}

#[test]
fn a_claim_answers_every_request_it_took() {
    let root = tempfile::tempdir().expect("temp dir");
    let (_temp, ctx, mut app) = app();
    let queued: Vec<_> = (0..2)
        .map(|_| enqueue(root.path(), &app, Operation::List, Duration::from_secs(5)))
        .collect();

    for request in &queued {
        let outcome = answer(root.path(), request, || {
            app.claim_device_requests(Some(root.path()));
            settle(&mut app, &ctx);
        });
        assert!(matches!(outcome, Outcome::Panels { .. }));
    }
    assert!(
        !app.device_claims_outstanding(),
        "no claimed request is left unanswered"
    );
}

#[test]
fn an_exit_refuses_a_claimed_request_once_and_drops_an_expired_one() {
    let root = tempfile::tempdir().expect("temp dir");
    let (_temp, _ctx, mut app) = app();
    let queued = enqueue(root.path(), &app, Operation::List, Duration::from_secs(5));
    let expired = enqueue(root.path(), &app, Operation::List, Duration::ZERO);
    app.claim_device_requests(Some(root.path()));
    app.settle_browser_host_io();
    assert_eq!(
        app.device_claim.claimed.len(),
        2,
        "the claim's requests wait for a frame that the exit ends"
    );

    app.begin_shutdown();
    app.flush_browser_host_io();

    let refused = device::take_result_at(root.path(), &queued)
        .expect("result")
        .expect("answer");
    assert!(
        matches!(&refused, Outcome::Failed { code, .. } if code == "host_shutdown"),
        "an exit answers a request it will not apply: {refused:?}"
    );
    assert!(
        device::take_result_at(root.path(), &queued).expect("result").is_none(),
        "a request is answered once"
    );
    let expired_outcome = device::take_result_at(root.path(), &expired)
        .expect("result")
        .expect("answer");
    assert!(
        matches!(&expired_outcome, Outcome::Failed { code, .. } if code == "request_expired"),
        "a request past its deadline is expired, not refused for a shutdown: {expired_outcome:?}"
    );
    assert!(!app.device_claims_outstanding());
}

#[test]
fn a_session_switch_refuses_a_claimed_request_once() {
    let root = tempfile::tempdir().expect("temp dir");
    let (_temp, _ctx, mut app) = app();
    let target = app
        .session_store
        .create_session_from_runtime(RuntimeState::default())
        .expect("target session");
    let queued = enqueue(root.path(), &app, Operation::List, Duration::from_secs(5));
    app.claim_device_requests(Some(root.path()));
    app.settle_browser_host_io();
    assert_eq!(app.device_claim.claimed.len(), 1);

    app.activate_runtime_session(&egui::Context::default(), &target);

    let refused = device::take_result_at(root.path(), &queued)
        .expect("result")
        .expect("answer");
    assert!(
        matches!(&refused, Outcome::Failed { code, .. } if code == "host_shutdown"),
        "a switch answers a request it will not apply: {refused:?}"
    );
    assert!(
        device::take_result_at(root.path(), &queued).expect("result").is_none(),
        "a request is answered once"
    );
}

#[test]
fn a_request_past_its_deadline_answers_request_expired() {
    let root = tempfile::tempdir().expect("temp dir");
    let (_temp, ctx, mut app) = app();
    let queued = enqueue(root.path(), &app, Operation::List, Duration::ZERO);

    let outcome = answer(root.path(), &queued, || {
        app.claim_device_requests(Some(root.path()));
        settle(&mut app, &ctx);
    });
    assert!(
        matches!(&outcome, Outcome::Failed { code, .. } if code == "request_expired"),
        "a claimed request whose deadline passed is refused: {outcome:?}"
    );
}

#[test]
fn the_pump_wake_flag_is_set_while_a_claim_is_outstanding_and_cleared_after() {
    let root = tempfile::tempdir().expect("temp dir");
    let (_temp, ctx, app) = app();
    let queued = enqueue(root.path(), &app, Operation::List, Duration::from_secs(5));
    let bridge = crate::app::DeviceRequestBridge::with_root(root.path().to_path_buf());
    bridge.install(app, ctx);
    assert!(!bridge.holds_claims(), "no claim is outstanding yet");

    bridge.poll_on_ui_thread();
    assert!(
        bridge.holds_claims(),
        "a claim the worker runs keeps the pump waking, so it applies without a frame"
    );
    let outcome = answer(root.path(), &queued, || {
        bridge.poll_on_ui_thread();
    });
    assert!(matches!(outcome, Outcome::Panels { .. }));
    // The outcome of the answer's own write applies on a later wake, and then
    // the host owes the agent nothing more.
    let deadline = Instant::now() + Duration::from_secs(10);
    while bridge.holds_claims() {
        assert!(Instant::now() < deadline, "the drained host still wakes the pump");
        std::thread::sleep(Duration::from_millis(5));
        bridge.poll_on_ui_thread();
    }
}

/// A ready browser panel of the first agent's workspace, as a capture needs.
fn browser_panel(app: &mut HorizonApp) -> String {
    let mut browser = BrowserPanelState::inert();
    browser.status = BrowserStatus::Ready;
    let mut panel = Panel::from_content(
        PanelId(999),
        app.board.panels[0].workspace_id,
        PanelKind::Browser,
        PanelContent::Browser(Box::new(browser)),
    );
    panel.visible = false;
    let local = panel.local_id.clone();
    app.board.panels.push(panel);
    local
}

#[cfg(unix)]
#[test]
fn a_fifo_backed_manifest_read_does_not_stall_the_frame_that_claims() {
    let root = tempfile::tempdir().expect("temp dir");
    let (_temp, ctx, mut app) = app();
    let local = browser_panel(&mut app);
    let manifest_path = manifest::manifest_path_for_root(root.path(), &local);
    std::fs::create_dir_all(manifest_path.parent().expect("manifest directory")).expect("manifest directory");
    let made = std::process::Command::new("mkfifo")
        .arg(&manifest_path)
        .status()
        .expect("mkfifo runs");
    assert!(made.success());
    let queued = enqueue(
        root.path(),
        &app,
        Operation::BrowserScreenshot {
            input: device::ScreenshotInput {
                panel_id: local,
                copy_to_clipboard: false,
            },
        },
        Duration::from_secs(5),
    );
    // The read waits until the test releases it, so a frame that reads the
    // manifest itself waits for the release and fails the bound below.
    let (release, released) = mpsc::channel::<()>();
    let writer = std::thread::spawn(move || {
        let _ = released.recv_timeout(Duration::from_secs(10));
        std::fs::write(manifest_path, b"{}").expect("the read waits");
    });

    let started = Instant::now();
    app.claim_device_requests(Some(root.path()));
    // The frame that follows applies nothing: the claim job still reads the
    // manifest, and the frame does not wait for that read.
    assert!(
        !app.drain_claimed_device_requests(&ctx),
        "the capture waits for its claim"
    );
    let elapsed = started.elapsed();
    // A job that the claim job holds back proves which thread reads the file.
    let (finished, ran) = mpsc::channel::<()>();
    app.browser_create_host.io.write(move || {
        let _ = finished.send(());
    });
    let held = ran.recv_timeout(Duration::from_millis(300)).is_err();
    release.send(()).expect("the writer waits");
    assert!(
        elapsed < Duration::from_millis(1500),
        "the frame waited for the manifest read: {elapsed:?}"
    );
    assert!(held, "the claim job reads the manifest on the coordination worker");

    let outcome = answer(root.path(), &queued, || settle(&mut app, &ctx));
    writer.join().expect("the writer ends");
    assert!(
        matches!(&outcome, Outcome::Failed { code, .. } if code == "browser_unavailable"),
        "a claim job that reads no manifest still refuses the capture: {outcome:?}"
    );
}
