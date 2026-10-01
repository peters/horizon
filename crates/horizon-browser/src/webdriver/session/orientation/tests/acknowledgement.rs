use super::*;
use crate::{BrowserCoordination, CoordinationOwnership, CoordinationSignals, HandoffRequest};
use std::sync::{Arc, Mutex, mpsc};

#[derive(Debug)]
struct Observer {
    audit: Owner,
    current: Mutex<CoordinationOwnership>,
    refused: AtomicBool,
}
impl BrowserCoordination for Observer {
    fn prepare(&self, id: &str, timeout: Duration) -> bool {
        self.audit.prepare(id, timeout)
    }
    fn initialize(&self, id: &str, state: &crate::CoordinationState) -> std::io::Result<()> {
        self.audit.initialize(id, state)
    }
    fn update(&self, id: &str, state: &crate::CoordinationState) -> std::io::Result<()> {
        self.audit.update(id, state)
    }
    fn set_user_active(&self, id: &str, active: bool) -> std::io::Result<()> {
        self.audit.set_user_active(id, active)
    }
    fn signals(&self, _: &str) -> std::io::Result<CoordinationSignals> {
        let current = self.current.lock().unwrap();
        Ok(CoordinationSignals {
            owner: current.owner.clone(),
            handoff: current.handoff.clone(),
            actions: Vec::new(),
        })
    }
    fn observe_ownership(&self, _: &str) -> std::io::Result<CoordinationOwnership> {
        if self.refused.load(Ordering::Acquire) {
            return Err(std::io::Error::other("private observer failure"));
        }
        Ok(self.current.lock().unwrap().clone())
    }
    fn acknowledge_handoff(&self, _: &str, _: &str) -> std::io::Result<bool> {
        Ok(false)
    }
    fn record_action(&self, id: &str, entry: &crate::BrowserAuditEntry) -> std::io::Result<()> {
        self.audit.record_action(id, entry)
    }
    fn remove(&self, id: &str, timeout: Duration) -> bool {
        self.audit.remove(id, timeout)
    }
}

fn measured(
    user: bool,
    final_replies: Vec<Reply>,
) -> (
    Driver,
    Server,
    std::thread::JoinHandle<Vec<serde_json::Value>>,
    Arc<Observer>,
) {
    let mut replies = vec![baseline(), Reply::json(200, &json!({"value":null}))];
    replies.extend(observation(4, 2));
    replies.push(baseline());
    replies.extend(final_replies);
    let classic = Server::start(replies);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    let owner = Arc::new(Observer {
        audit: Owner(
            Mutex::new(Some("agent".into())),
            Mutex::new(Vec::new()),
            Mutex::new(Vec::new()),
        ),
        current: Mutex::new(CoordinationOwnership {
            owner: Some("agent".into()),
            handoff: None,
        }),
        refused: AtomicBool::new(false),
    });
    driver.config.coordination = Some(owner.clone());
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
    if user {
        driver.begin_user_orientation(
            "user-rotation".into(),
            RemoteOrientation::Landscape,
            &events(),
            &AtomicBool::new(false),
        );
    } else {
        driver.begin_orientation(&rotation_request(), &events(), &AtomicBool::new(false));
    }
    driver.tick_orientation(&events(), &AtomicBool::new(false));
    assert!(driver.pending_orientation.as_ref().unwrap().verified.is_some());
    assert_eq!(classic.recorded().len(), 7);
    assert!(driver.tick_coordination(&events()).is_empty());
    (driver, classic, worker, owner)
}

fn failed_once(driver: &Driver, owner: &Observer, user: bool, code: &str) {
    assert!(driver.pending_orientation.is_none());
    assert!(
        driver.orientation_error.as_deref().unwrap().starts_with(code),
        "expected {code}, received {:?}",
        driver.orientation_error
    );
    assert_eq!(
        *owner.audit.1.lock().unwrap(),
        vec![crate::BrowserAuditStatus::Dispatched, crate::BrowserAuditStatus::Failed]
    );
    if user {
        assert_eq!(driver.orientation_completed.completed.len(), 1);
        assert!(
            driver.orientation_completed.completed[0]
                .error
                .as_deref()
                .unwrap()
                .starts_with(code)
        );
    }
}

#[test]
fn document_replacement_after_measurement_or_during_final_ownership_never_acknowledges() {
    for user in [false, true] {
        for after_ownership in [false, true] {
            let changed = Reply::json(200, &json!({"value":"replacement-document"}));
            let replies = if after_ownership {
                vec![baseline(), changed]
            } else {
                vec![changed]
            };
            let (mut driver, classic, worker, owner) = measured(user, replies);
            driver.tick_orientation(&events(), &AtomicBool::new(false));
            failed_once(&driver, &owner, user, "orientation_navigation_invalidated:");
            assert_eq!(
                classic
                    .recorded()
                    .iter()
                    .filter(|call| call.path.ends_with("/orientation") && call.method == "POST")
                    .count(),
                1
            );
            drop(driver);
            assert!(worker.join().unwrap().is_empty());
        }
    }
}

#[test]
fn stop_teach_and_ownership_takeover_during_final_read_refuse_once() {
    for user in [false, true] {
        for reason in ["stop", "teach", "owner", "handoff"] {
            if user && matches!(reason, "owner" | "handoff") {
                continue;
            }
            let (release, blocked) = mpsc::channel();
            let (mut driver, classic, worker, owner) =
                measured(user, vec![baseline().blocked_until(blocked), baseline()]);
            let panel_slot = driver.panel_slot.clone();
            let stopped = Arc::new(AtomicBool::new(false));
            let rotation_stopped = stopped.clone();
            let rotation = std::thread::spawn(move || {
                driver.tick_orientation(&events(), &rotation_stopped);
                driver
            });
            let deadline = Instant::now() + Duration::from_secs(3);
            while classic.recorded().len() < 8 {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            let code = match reason {
                "stop" => {
                    stopped.store(true, Ordering::Release);
                    "browser_unavailable:"
                }
                "teach" => {
                    panel_slot.set_teach_recording(true);
                    "orientation_user_active:"
                }
                "owner" => {
                    owner.current.lock().unwrap().owner = Some("replacement-owner".into());
                    "orientation_ownership_lost:"
                }
                "handoff" => {
                    owner.current.lock().unwrap().handoff = Some(HandoffRequest {
                        request_id: "new-handoff".into(),
                        reason: "human request".into(),
                    });
                    "orientation_handoff_pending:"
                }
                _ => unreachable!(),
            };
            release.send(()).unwrap();
            let driver = rotation.join().unwrap();
            failed_once(&driver, &owner, user, code);
            drop(driver);
            assert!(worker.join().unwrap().is_empty());
        }
    }
}

#[test]
fn final_read_deadline_and_observer_failure_keep_measured_results_unacknowledged() {
    for user in [false, true] {
        for timeout in [false, true] {
            let reply = if timeout {
                baseline().delayed(Duration::from_millis(300))
            } else {
                baseline()
            };
            let (mut driver, _classic, worker, owner) = measured(user, vec![reply, baseline()]);
            if timeout {
                driver.pending_orientation.as_mut().unwrap().deadline = Instant::now() + Duration::from_millis(100);
            } else {
                owner.refused.store(true, Ordering::Release);
            }
            driver.tick_orientation(&events(), &AtomicBool::new(false));
            failed_once(
                &driver,
                &owner,
                user,
                if timeout {
                    "orientation_timeout:"
                } else {
                    "orientation_ownership_unverified:"
                },
            );
            assert!(
                !driver
                    .orientation_error
                    .as_deref()
                    .unwrap()
                    .contains("private observer failure")
            );
            drop(driver);
            assert!(worker.join().unwrap().is_empty());
        }
    }
}

#[test]
fn custom_coordinator_without_readonly_observation_refuses_confirmation() {
    for user in [false, true] {
        let (mut driver, _classic, worker, _) = measured(user, vec![baseline()]);
        driver.config.coordination = Some(Arc::new(super::navigation::SlowPublication));
        driver.tick_orientation(&events(), &AtomicBool::new(false));
        assert!(driver.pending_orientation.is_none());
        assert!(
            driver
                .orientation_error
                .as_deref()
                .unwrap()
                .starts_with("orientation_ownership_unverified:")
        );
        if user {
            assert_eq!(driver.orientation_completed.completed.len(), 1);
            assert!(driver.orientation_completed.completed[0].error.is_some());
        }
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }
}
