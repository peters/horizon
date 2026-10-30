use super::*;
use crate::app::test_support::test_app;
use horizon_core::browser::remote_recovery::HeldRemoteAllocation;
use horizon_core::browser::{RemoteAllocation, remote_slots};
use horizon_core::{PanelKind, PanelOptions, browser_actor};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Polls the queue and applies what the coordination worker answered, as a
/// later frame does.
fn poll_recovery(app: &mut HorizonApp, queue: &RecoveryQueue) {
    app.poll_remote_recovery_queue(queue);
    app.settle_browser_host_io();
}

fn released_slot(root: &std::path::Path) -> remote_slots::SlotLease {
    // Concurrent test forks can retain a released descriptor until exec.
    (0..100)
        .find_map(|_| match remote_slots::acquire_slot(root, "quota", 2) {
            Ok(lease) => Some(lease),
            Err(remote_slots::SlotError::Busy { .. }) => {
                std::thread::sleep(Duration::from_millis(5));
                None
            }
            Err(error) => panic!("{error}"),
        })
        .expect("matching lease released after forked children exec")
}

#[test]
fn host_dispatch_authorizes_waits_and_completes_exact_recovery() {
    let (temp, mut app) = test_app();
    let workspace = app.board.create_workspace("recovery");
    let (command, args) = ("/bin/sh", vec!["-c".into(), "exit 0".into()]);
    let agent = app
        .board
        .create_panel(
            PanelOptions {
                command: Some(command.into()),
                args,
                kind: PanelKind::Codex,
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("agent");
    let actor = browser_actor(&app.board.panel(agent).expect("agent").local_id);
    let workspace_local = app.board.workspace(workspace).expect("workspace").local_id.clone();
    let queue = RecoveryQueue::new(temp.path().join("queue"));
    let identity = manifest::AgentIdentity::new(&actor, Some(manifest::host_instance()));
    let listener = TcpListener::bind("127.0.0.1:0").expect("provider");
    let endpoint = format!("http://{}", listener.local_addr().expect("address"));
    let (observed_tx, observed_rx) = mpsc::channel();
    let (reply_tx, reply_rx) = mpsc::channel();
    let provider = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("exact probe");
        stream.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).expect("request");
            request.push(byte[0]);
        }
        assert!(request.starts_with(b"GET /session/private-fixture/url HTTP/1.1\r\n"));
        observed_tx.send(()).expect("observed");
        reply_rx.recv_timeout(Duration::from_secs(5)).expect("release reply");
        let body = r#"{"value":{"error":"invalid session id","message":"private-fixture"}}"#;
        write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("reply");
    });
    let allocation = RemoteAllocation::unresolved_for_test(&endpoint, "private-fixture").expect("allocation");
    let unrelated = RemoteAllocation::default();
    for (held, owner) in [(&allocation, actor.as_str()), (&unrelated, "another-owner")] {
        app.browser_create_host.remote_allocations.insert(HeldRemoteAllocation {
            allocation: held.clone(),
            provider: "fixture".into(),
            owner: owner.into(),
            workspace: workspace_local.clone(),
            lease: Some(remote_slots::acquire_slot(temp.path(), "quota", 2).expect("lease")),
        });
    }
    let denied = queue
        .enqueue(identity, Some(unrelated.reference().into()))
        .expect("unauthorized request");
    poll_recovery(&mut app, &queue);
    let result = queue
        .take(identity, &denied)
        .expect("result")
        .expect("completed refusal");
    assert_eq!(result.error.as_deref(), Some("allocation_unavailable"));
    assert!(result.allocations.is_empty());

    let id = queue
        .enqueue(identity, Some(allocation.reference().into()))
        .expect("request");
    poll_recovery(&mut app, &queue);
    observed_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("probe started asynchronously");
    assert!(queue.take(identity, &id).expect("no premature result").is_none());
    assert_eq!(app.browser_create_host.recovery_requests.len(), 1);
    reply_tx.send(()).expect("allow exact absence reply");
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        poll_recovery(&mut app, &queue);
        if let Some(result) = queue.take(identity, &id).expect("result") {
            break result;
        }
        assert!(Instant::now() < deadline, "host must complete the queued request");
        std::thread::sleep(Duration::from_millis(5));
    };
    provider.join().expect("provider completed");
    assert!(result.error.is_none());
    assert_eq!(result.allocations.len(), 1);
    assert_eq!(result.allocations[0].status, RemoteRecoveryStatus::Released);
    assert!(
        !serde_json::to_string(&result)
            .expect("public result")
            .contains("private-fixture")
    );
    assert!(app.browser_create_host.recovery_requests.is_empty());
    poll_recovery(&mut app, &queue);
    let _available = released_slot(temp.path());
    assert!(matches!(
        remote_slots::acquire_slot(temp.path(), "quota", 2),
        Err(remote_slots::SlotError::Busy { .. })
    ));
}

#[test]
fn moving_a_restored_panel_cannot_retarget_an_earlier_orphan() {
    use horizon_core::browser::{BrowserConfig, BrowserPanelState};
    use horizon_core::{Panel, PanelContent, PanelId};
    let (_temp, mut app) = test_app();
    let original = app.board.create_workspace("original");
    let restored = app.board.create_workspace("restored");
    let workspace = app.board.workspace(original).expect("workspace").local_id.clone();
    let allocation =
        RemoteAllocation::unresolved_for_test("http://127.0.0.1:1", "retired-private").expect("allocation");
    allocation.mark_published();
    allocation.expect_workspace(&workspace);
    allocation.retain_scope(horizon_browser::RemoteAllocationScope {
        admission_fallback: false,
        host: manifest::host_instance().into(),
        workspace: Some(workspace.clone()),
        owner: Some("original-owner".into()),
    });
    app.browser_create_host.remote_allocations.insert(HeldRemoteAllocation {
        allocation,
        provider: "fixture".into(),
        owner: "original-owner".into(),
        workspace: workspace.clone(),
        lease: None,
    });
    let browser = BrowserPanelState::restored_remote("saved-panel", &BrowserConfig::default(), "fixture".into(), None);
    assert!(browser.remote_allocation().is_none());
    let panel_id = PanelId(500);
    let mut panel = Panel::from_content(
        panel_id,
        original,
        PanelKind::Browser,
        PanelContent::Browser(Box::new(browser)),
    );
    panel.local_id = "saved-panel".into();
    app.board.panels.push(panel);
    app.refresh_remote_recovery_scope();
    app.board.assign_panel_to_workspace(panel_id, restored);
    app.refresh_remote_recovery_scope();
    let summaries = app
        .browser_create_host
        .remote_allocations
        .summaries(Some(("original-owner", &workspace)));
    assert_eq!(
        summaries.len(),
        1,
        "restored placement cannot mutate the retired allocation"
    );
    assert_eq!(summaries[0].status, RemoteRecoveryStatus::Unresolved);
}

#[test]
fn moving_then_bulk_closing_refreshes_the_remote_allocation_scope() {
    use horizon_core::browser::BrowserPanelState;
    use horizon_core::{Panel, PanelContent, PanelId};
    let (_temp, mut app) = test_app();
    let original = app.board.create_workspace("original");
    let destination = app.board.create_workspace("destination");
    let original_local = app.board.workspace(original).expect("original").local_id.clone();
    let destination_local = app.board.workspace(destination).expect("destination").local_id.clone();
    let browser = BrowserPanelState::inert_remote("fixture", "fixture");
    let allocation = browser.remote_allocation().expect("allocation").clone();
    allocation.mark_published();
    let panel_id = PanelId(500);
    let panel = Panel::from_content(
        panel_id,
        original,
        PanelKind::Browser,
        PanelContent::Browser(Box::new(browser)),
    );
    let path = manifest::manifest_path_for_root(app.session_store.home().root(), &panel.local_id);
    manifest::write_at(
        &path,
        &manifest::BrowserManifest {
            panel_local_id: panel.local_id.clone(),
            host: Some(manifest::host_instance().into()),
            owner: Some(manifest::ManifestOwner {
                name: "owner".into(),
                tty: None,
                updated_at: manifest::now_millis(),
            }),
            ..manifest::BrowserManifest::default()
        },
    )
    .expect("manifest");
    app.board.panels.push(panel);
    app.board
        .workspace_mut(original)
        .expect("workspace")
        .panels
        .push(panel_id);
    app.refresh_remote_recovery_scope();
    app.settle_browser_host_io();
    assert_eq!(
        manifest::read_at(&path)
            .expect("manifest")
            .workspace
            .expect("scope")
            .local_id,
        original_local
    );
    let retain = |snapshot: manifest::BrowserManifest| {
        allocation.retain_scope(horizon_browser::RemoteAllocationScope {
            admission_fallback: false,
            host: snapshot.host.expect("host"),
            workspace: snapshot.workspace.map(|workspace| workspace.local_id),
            owner: snapshot.owner.map(|owner| owner.name),
        });
    };
    let recoverable_through = |workspace: &str| {
        allocation
            .status_for(manifest::host_instance(), "owner", workspace, true)
            .is_some()
    };

    // Hold the coordination worker, as a disk that does not answer would,
    // with the tick's stamp of the old placement still to run: the close
    // never waits for the stamp of the move.
    let (release, released) = std::sync::mpsc::channel::<()>();
    app.browser_create_host.io.write(move || {
        let _ = released.recv();
    });
    app.browser_create_host.forget_stamped_placement();
    assert!(app.restamp_browser_manifests_for_placement());
    app.board.assign_panel_to_workspace(panel_id, destination);
    app.close_workspace_panels(destination);
    assert!(app.board.panel(panel_id).is_none());
    let stale = manifest::read_at(&path).expect("a retirement snapshot before the stamp lands");
    assert_eq!(stale.workspace.as_ref().expect("scope").local_id, original_local);
    retain(stale);
    // The driver removes the manifest once it kept that scope.
    std::fs::remove_file(&path).expect("retired manifest");
    assert!(
        !recoverable_through(&original_local) && !recoverable_through(&destination_local),
        "a snapshot taken before the stamp landed authorizes no workspace until it lands"
    );

    release.send(()).expect("the worker waits");
    app.settle_browser_host_io();
    assert!(
        !recoverable_through(&original_local),
        "the workspace the panel left cannot recover it"
    );
    assert!(
        recoverable_through(&destination_local),
        "the workspace it moved to can, as when the stamp lands before the close"
    );
}

/// Holds `path` with an OS file lock from a helper thread until the test
/// signals it, as an MCP server or a slow disk holds the coordination files.
#[cfg(unix)]
fn hold_lock(path: &std::path::Path) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
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
fn a_held_recovery_lock_never_stalls_the_frame_that_claims() {
    let (temp, mut app) = test_app();
    let queue = RecoveryQueue::new(temp.path().join("queue"));
    let identity = manifest::AgentIdentity::new("horizon:missing-agent", Some(manifest::host_instance()));
    let id = queue.enqueue(identity, None).expect("recovery request");
    let (release, holder) = hold_lock(&temp.path().join("queue/runtime/browser-recovery/queue.json.lock"));

    let started = Instant::now();
    app.poll_remote_recovery_queue(&queue);
    let elapsed = started.elapsed();
    let claiming = app.browser_create_host.io.busy();
    release.send(()).expect("the lock holder waits");
    assert!(
        elapsed < Duration::from_millis(1500),
        "the claim stalled the frame: {elapsed:?}"
    );
    assert!(claiming, "the claim runs on the coordination worker");

    app.settle_browser_host_io();
    holder.join().expect("the lock holder ends");
    let answer = loop {
        poll_recovery(&mut app, &queue);
        if let Some(answer) = queue.take(identity, &id).expect("result") {
            break answer;
        }
        // A held lock makes the claim fail once; the next poll claims the request.
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the request was never answered"
        );
    };
    assert_eq!(
        answer.error.as_deref(),
        Some("allocation_unavailable"),
        "the refused request was answered on the coordination worker"
    );
}

/// The scope read runs on the tick and on every close, so a worker that is
/// still reading must not take one more read of every remote panel per call.
#[cfg(unix)]
#[test]
fn a_scope_read_in_flight_is_not_queued_again() {
    let (_temp, mut app) = test_app();
    // The read of an earlier call is still on the worker.
    app.browser_create_host.scope_reading = true;
    let (release, released) = mpsc::channel::<()>();
    app.browser_create_host.io.write(move || {
        let _ = released.recv();
    });

    let started = Instant::now();
    app.refresh_remote_recovery_scope();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "the call waited for the worker"
    );

    release.send(()).expect("the worker waits");
    app.settle_browser_host_io();
    assert!(
        app.browser_create_host.scope_reading,
        "a call that queued no read keeps the guard of the read it found"
    );
}

/// The owner a manifest holds, for the read of a remote panel's scope.
fn owned_manifest(owner: &str) -> manifest::BrowserManifest {
    manifest::BrowserManifest {
        owner: Some(manifest::ManifestOwner {
            name: owner.into(),
            tty: None,
            updated_at: manifest::now_millis(),
        }),
        ..manifest::BrowserManifest::default()
    }
}

/// The scope a read records: a successful read reports the owner the manifest
/// holds, also when it holds none, so a cleared owner clears the record. A read
/// that fails is left out, so the record keeps the scope of the last good read.
#[test]
fn a_cleared_owner_clears_the_record_and_a_failed_read_keeps_its_scope() {
    use horizon_core::browser::BrowserPanelState;
    use horizon_core::{Panel, PanelContent, PanelId};

    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("scope");
    let workspace_local = app.board.workspace(workspace).expect("workspace").local_id.clone();
    let browser = BrowserPanelState::inert_remote("fixture", "fixture");
    let allocation = browser.remote_allocation().expect("allocation").clone();
    allocation.mark_published();
    let panel_id = PanelId(500);
    let panel = Panel::from_content(
        panel_id,
        workspace,
        PanelKind::Browser,
        PanelContent::Browser(Box::new(browser)),
    );
    let local_id = panel.local_id.clone();
    app.browser_create_host.remote_allocations.insert(HeldRemoteAllocation {
        allocation: allocation.clone(),
        provider: "fixture".into(),
        owner: "owner".into(),
        workspace: workspace_local.clone(),
        lease: None,
    });
    let scope = || RemotePanelScope {
        local_id: local_id.clone(),
        allocation: allocation.clone(),
        workspace: workspace_local.clone(),
    };
    // The record authorizes the agent `owner` in this workspace until a read
    // says otherwise.
    let readonly = |app: &mut HorizonApp, owner: &str| {
        app.browser_create_host
            .remote_allocations
            .summaries(Some((owner, &workspace_local)))
            .len()
    };
    assert_eq!(readonly(&mut app, "owner"), 1, "the record starts under its owner");

    let read = read_remote_panel_scopes(vec![scope()], |_| Some(owned_manifest("owner")));
    app.apply_remote_recovery_scopes(read);
    assert_eq!(
        readonly(&mut app, "owner"),
        1,
        "the owner the manifest holds authorizes it"
    );

    // A successful read that finds no owner reports none, and the record is
    // cleared: an allocation whose manifest lost its owner authorizes nobody.
    let read = read_remote_panel_scopes(vec![scope()], |_| Some(manifest::BrowserManifest::default()));
    assert!(
        matches!(read.as_slice(), [(_, None)]),
        "a successful read reports the owner it found"
    );
    app.apply_remote_recovery_scopes(read);
    assert_eq!(readonly(&mut app, "owner"), 0, "a cleared owner clears the record");
    assert_eq!(
        app.browser_create_host.remote_allocations.summaries(None).len(),
        1,
        "the record still exists"
    );

    // A read that fails is left out of the answer, so the record keeps the
    // scope that the last good read recorded.
    let read = read_remote_panel_scopes(vec![scope()], |_| Some(owned_manifest("owner")));
    app.apply_remote_recovery_scopes(read);
    assert_eq!(readonly(&mut app, "owner"), 1, "the record has its owner again");
    let read = read_remote_panel_scopes(vec![scope()], |_| None);
    assert!(read.is_empty(), "a read that failed reports nothing");
    app.apply_remote_recovery_scopes(read);
    assert_eq!(
        readonly(&mut app, "owner"),
        1,
        "a failed read keeps the scope that the last good read recorded"
    );
}

/// A lock that outlives the wait of a write makes the write fail. The request
/// stays claimed, so the host writes the same answer again while the request is
/// still waiting.
#[cfg(unix)]
#[test]
fn a_refused_recovery_answer_is_written_after_the_lock_is_released() {
    let (temp, mut app) = test_app();
    let queue = RecoveryQueue::new(temp.path().join("queue"));
    let identity = manifest::AgentIdentity::new("horizon:missing-agent", Some(manifest::host_instance()));
    let id = queue.enqueue(identity, None).expect("recovery request");
    let request = queue.claim(manifest::host_instance()).expect("claim").remove(0);
    // This frame answers the request it holds; its claim is not what the test
    // measures, and a claim job would wait out the same lock.
    app.browser_create_host.recovery_claiming = true;
    app.browser_create_host.recovery_requests.push(request);
    let (release, holder) = hold_lock(&temp.path().join("queue/runtime/browser-recovery/queue.json.lock"));

    app.poll_remote_recovery_queue(&queue);
    app.settle_browser_host_io();
    assert!(
        queue.take(identity, &id).expect("result").is_none(),
        "the answer waits for the lock"
    );

    release.send(()).expect("the lock holder waits");
    holder.join().expect("the lock holder ends");
    let started = Instant::now();
    let answer = loop {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the refused answer was never written"
        );
        std::thread::sleep(crate::app::answer_retries::RETRY_INTERVAL);
        poll_recovery(&mut app, &queue);
        if let Some(answer) = queue.take(identity, &id).expect("result") {
            break answer;
        }
    };
    assert_eq!(answer.error.as_deref(), Some("allocation_unavailable"));
}
