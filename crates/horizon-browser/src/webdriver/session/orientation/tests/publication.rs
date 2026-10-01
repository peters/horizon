use super::*;
use crate::remote::RemoteOrientationState;
use crate::session::BrowserEvent;
use std::sync::{Arc, Mutex, mpsc};

#[derive(Debug)]
struct BlockingPublication {
    owner: Owner,
    entered: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl crate::BrowserCoordination for BlockingPublication {
    fn prepare(&self, panel: &str, timeout: Duration) -> bool {
        self.owner.prepare(panel, timeout)
    }
    fn initialize(&self, panel: &str, state: &crate::CoordinationState) -> std::io::Result<()> {
        self.owner.initialize(panel, state)
    }
    fn update(&self, panel: &str, state: &crate::CoordinationState) -> std::io::Result<()> {
        self.owner.update(panel, state)?;
        if let Some(entered) = self.entered.lock().unwrap().take() {
            entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
        }
        Ok(())
    }
    fn set_user_active(&self, panel: &str, active: bool) -> std::io::Result<()> {
        self.owner.set_user_active(panel, active)
    }
    fn signals(&self, panel: &str) -> std::io::Result<crate::CoordinationSignals> {
        self.owner.signals(panel)
    }
    fn acknowledge_handoff(&self, panel: &str, actor: &str) -> std::io::Result<bool> {
        self.owner.acknowledge_handoff(panel, actor)
    }
    fn remove(&self, panel: &str, timeout: Duration) -> bool {
        self.owner.remove(panel, timeout)
    }
    fn record_action(&self, panel: &str, entry: &crate::BrowserAuditEntry) -> std::io::Result<()> {
        self.owner.record_action(panel, entry)
    }
}

#[test]
fn cancellation_during_publication_prevents_post_for_agent_and_user() {
    for user in [false, true] {
        for teach in [false, true] {
            let classic = Server::start(vec![baseline()]);
            let (link, worker) = bidi_fixture(false, false);
            let mut driver = fixture_driver(&classic, link);
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let coordination = Arc::new(BlockingPublication {
                owner: Owner(Mutex::new(Some("agent".into())), Mutex::default(), Mutex::default()),
                entered: Mutex::new(Some(entered_tx)),
                release: Mutex::new(release_rx),
            });
            driver.config.coordination = Some(coordination.clone());
            driver.remote_orientation = Some(RemoteOrientationState {
                support: OrientationSupport::Supported,
                applied: Some(RemoteOrientation::Portrait),
            });
            driver.orientation_document = DocumentOrientation::Clean;
            let panel_slot = driver.panel_slot.clone();
            let stopped = Arc::new(AtomicBool::new(false));
            let rotation_stopped = stopped.clone();
            let (tx, rx) = mpsc::channel();
            let sender = BrowserEventSender { tx, ..events() };
            let rotation = std::thread::spawn(move || {
                if user {
                    driver.begin_user_orientation(
                        "user-rotation".into(),
                        RemoteOrientation::Landscape,
                        &sender,
                        &rotation_stopped,
                    );
                } else {
                    driver.service_browser_request(&rotation_request(), &sender, &rotation_stopped);
                }
                driver
            });
            entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            if teach {
                panel_slot.set_teach_recording(true);
            } else {
                stopped.store(true, Ordering::Release);
            }
            release_tx.send(()).unwrap();
            let driver = rotation.join().unwrap();
            let code = if teach {
                "orientation_user_active:"
            } else {
                "browser_unavailable:"
            };
            assert!(
                classic.recorded().len() == 1 && classic.recorded()[0].path == "/session/test/execute/sync",
                "cancelled publication must not dispatch POST"
            );
            assert!(driver.pending_orientation.is_none());
            assert!(driver.orientation_error.as_ref().unwrap().starts_with(code));
            assert_eq!(
                *coordination.owner.1.lock().unwrap(),
                vec![crate::BrowserAuditStatus::Dispatched, crate::BrowserAuditStatus::Failed]
            );
            let last = rx
                .try_iter()
                .filter_map(|event| match event {
                    BrowserEvent::OrientationChanged(view) => Some(view),
                    _ => None,
                })
                .last()
                .unwrap();
            assert!(last.pending.is_none());
            assert!(last.state.applied.is_none());
            assert!(last.error.unwrap().starts_with(code));
            if user {
                assert_eq!(driver.orientation_completed.completed.len(), 1);
                assert_eq!(driver.orientation_completed.completed[0].action_id, "user-rotation");
            }
            drop(driver);
            assert!(worker.join().unwrap().is_empty());
        }
    }
}

#[test]
fn rotation_publishes_pending_and_persists_cleared_state_before_blocking_post() {
    for user in [false, true] {
        let (release, blocked) = mpsc::channel();
        let classic = Server::start(vec![
            baseline(),
            Reply::json(200, &json!({"value":null})).blocked_until(blocked),
        ]);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = fixture_driver(&classic, link);
        let owner = Arc::new(Owner(
            Mutex::new(Some("agent".into())),
            Mutex::new(Vec::new()),
            Mutex::new(Vec::new()),
        ));
        driver.config.coordination = Some(owner.clone());
        driver.remote_orientation = Some(RemoteOrientationState {
            support: OrientationSupport::Supported,
            applied: Some(RemoteOrientation::Portrait),
        });
        driver.orientation_document = DocumentOrientation::Clean;
        let (tx, rx) = mpsc::channel();
        let sender = BrowserEventSender { tx, ..events() };
        let rotation = std::thread::spawn(move || {
            if user {
                driver.begin_user_orientation(
                    "user-rotation".into(),
                    RemoteOrientation::Landscape,
                    &sender,
                    &AtomicBool::new(false),
                );
            } else {
                driver.begin_orientation(&rotation_request(), &sender, &AtomicBool::new(false));
            }
            driver
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while classic.recorded().len() < 2 {
            assert!(Instant::now() < deadline, "POST must reach the response latch");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            BrowserEvent::OrientationChanged(view)
                if view.state.applied.is_none()
                    && view.pending == Some(RemoteOrientation::Landscape)
                    && view.action_id.as_deref() == Some(if user { "user-rotation" } else { "rotation" })
        ));
        assert_eq!(
            *owner.2.lock().unwrap(),
            vec![Some(RemoteOrientationState {
                support: OrientationSupport::Supported,
                applied: None,
            })],
            "coordination must already be unverified while POST is blocked"
        );
        assert!(rx.try_recv().is_err(), "the blocked POST has not completed");
        release.send(()).unwrap();
        let driver = rotation.join().unwrap();
        assert!(driver.pending_orientation.is_some());
        assert_eq!(driver.orientation_document, DocumentOrientation::NeedsMeasurement);
        assert_eq!(classic.recorded().len(), 2);
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }
}

#[test]
fn publication_consuming_rotation_deadline_never_dispatches_post() {
    let classic = Server::start(vec![baseline()]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    driver.config.coordination = Some(Arc::new(super::navigation::SlowPublication));
    driver.remote_orientation = Some(RemoteOrientationState {
        support: OrientationSupport::Supported,
        applied: Some(RemoteOrientation::Portrait),
    });
    driver.orientation_document = DocumentOrientation::Clean;
    let mut request = rotation_request();
    request.action = BrowserControlAction::Orientation {
        orientation: RemoteOrientation::Landscape,
        timeout_millis: 500,
    };
    let (tx, rx) = mpsc::channel();
    let sender = BrowserEventSender { tx, ..events() };
    driver.service_browser_request(&request, &sender, &AtomicBool::new(false));
    assert!(driver.pending_orientation.is_none());
    assert_eq!(driver.remote_orientation.unwrap().applied, None);
    assert_eq!(driver.orientation_document, DocumentOrientation::NeedsMeasurement);
    assert!(
        driver
            .orientation_error
            .as_ref()
            .unwrap()
            .starts_with("orientation_timeout:")
    );
    assert!(
        classic.recorded().len() == 1 && classic.recorded()[0].path == "/session/test/execute/sync",
        "no mutation after publication used its deadline"
    );
    let updates: Vec<_> = rx
        .try_iter()
        .filter_map(|event| match event {
            BrowserEvent::OrientationChanged(view) => Some(view),
            _ => None,
        })
        .collect();
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].pending, Some(RemoteOrientation::Landscape));
    assert_eq!(updates[1].pending, None);
    assert!(updates[1].error.as_ref().unwrap().starts_with("orientation_timeout:"));
    assert!(updates.iter().all(|view| view.state.applied.is_none()));
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}
