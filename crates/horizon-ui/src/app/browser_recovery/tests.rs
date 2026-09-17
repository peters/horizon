use super::*;
use crate::app::test_support::test_app;
use horizon_core::browser::remote_recovery::HeldRemoteAllocation;
use horizon_core::browser::{RemoteAllocation, remote_slots};
use horizon_core::{PanelKind, PanelOptions, browser_actor};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn host_dispatch_authorizes_waits_and_completes_exact_recovery() {
    let (temp, mut app) = test_app();
    let workspace = app.board.create_workspace("recovery");
    let (command, args) = if cfg!(windows) {
        ("cmd.exe", vec!["/C".into(), "exit 0".into()])
    } else {
        ("/bin/sh", vec!["-c".into(), "exit 0".into()])
    };
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
    app.poll_remote_recovery_queue(&queue);
    let result = queue
        .take(identity, &denied)
        .expect("result")
        .expect("completed refusal");
    assert_eq!(result.error.as_deref(), Some("allocation_unavailable"));
    assert!(result.allocations.is_empty());

    let id = queue
        .enqueue(identity, Some(allocation.reference().into()))
        .expect("request");
    app.poll_remote_recovery_queue(&queue);
    observed_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("probe started asynchronously");
    assert!(queue.take(identity, &id).expect("no premature result").is_none());
    assert_eq!(app.browser_create_host.recovery_requests.len(), 1);
    reply_tx.send(()).expect("allow exact absence reply");
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        app.poll_remote_recovery_queue(&queue);
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
    app.poll_remote_recovery_queue(&queue);
    // Concurrent test forks can retain a released descriptor until exec.
    let _available = (0..100)
        .find_map(|_| match remote_slots::acquire_slot(temp.path(), "quota", 2) {
            Ok(lease) => Some(lease),
            Err(remote_slots::SlotError::Busy { .. }) => {
                std::thread::sleep(Duration::from_millis(5));
                None
            }
            Err(error) => panic!("{error}"),
        })
        .expect("matching lease released after forked children exec");
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
    assert_eq!(
        manifest::read_at(&path)
            .expect("manifest")
            .workspace
            .expect("scope")
            .local_id,
        original_local
    );

    app.board.assign_panel_to_workspace(panel_id, destination);
    app.close_workspace_panels(destination);
    assert!(app.board.panel(panel_id).is_none());
    let retired = manifest::read_at(&path).expect("driver retirement snapshot");
    assert_eq!(retired.workspace.as_ref().expect("scope").local_id, destination_local);
    allocation.retain_scope(horizon_browser::RemoteAllocationScope {
        admission_fallback: false,
        host: retired.host.expect("host"),
        workspace: retired.workspace.map(|workspace| workspace.local_id),
        owner: retired.owner.map(|owner| owner.name),
    });
    assert!(
        allocation
            .status_for(manifest::host_instance(), "owner", &original_local, true)
            .is_none()
    );
    assert!(
        allocation
            .status_for(manifest::host_instance(), "owner", &destination_local, true)
            .is_some()
    );
}
