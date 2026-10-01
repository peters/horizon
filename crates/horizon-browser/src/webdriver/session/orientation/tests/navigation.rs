use super::*;
use crate::navigation::AgentActionExecution;
use crate::session::{BrowserEvent, BrowserEventSender};
use std::sync::{Arc, Mutex, mpsc};

#[test]
fn every_navigation_path_publishes_and_persists_unverified_before_completion() {
    for operation in ["navigate", "agent_navigate", "reload", "back", "forward"] {
        let failure = Reply::json(
            500,
            &json!({"value":{"error":"unknown error","message":"navigation refused"}}),
        );
        let replies = if operation == "agent_navigate" {
            vec![
                Reply::json(200, &json!({"value":null})),
                failure,
                Reply::json(200, &json!({"value":null})),
            ]
        } else {
            vec![failure]
        };
        let classic = Server::start(replies);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = fixture_driver(&classic, link);
        driver.config.browser.backend = crate::BackendKind::SafariWebDriver;
        driver.remote_orientation = Some(crate::remote::RemoteOrientationState {
            support: OrientationSupport::Supported,
            applied: Some(RemoteOrientation::Landscape),
        });
        let owner = Arc::new(Owner(Mutex::new(None), Mutex::new(Vec::new()), Mutex::new(Vec::new())));
        driver.config.coordination = Some(owner.clone());
        let (tx, rx) = mpsc::channel();
        let events = BrowserEventSender { tx, ..events() };
        let failed = match operation {
            "navigate" => driver.navigate("https://example.test/next", &events).is_err(),
            "agent_navigate" => {
                let mut request = rotation_request();
                request.action = BrowserControlAction::Navigate {
                    url: "https://example.test/next".into(),
                    wait: crate::NavigationWait::Commit,
                    timeout_millis: Some(5000),
                };
                matches!(
                    driver.navigate_action(&request, &events),
                    AgentActionExecution::Done(Err(_))
                )
            }
            "reload" => driver.reload(&events).is_err(),
            "back" => driver.traverse(-1, &events).is_err(),
            "forward" => driver.traverse(1, &events).is_err(),
            _ => unreachable!(),
        };
        assert!(failed, "{operation}");
        let states = owner.2.lock().unwrap();
        assert_eq!(states.len(), 1, "{operation}: cleared status must be persisted once");
        assert_eq!(states[0].unwrap().applied, None, "{operation}");
        assert_eq!(states[0].unwrap().support, OrientationSupport::Supported, "{operation}");
        let observed: Vec<_> = rx.try_iter().collect();
        let invalidation = observed
            .iter()
            .position(|event| matches!(event, BrowserEvent::OrientationChanged(view) if view.state.applied.is_none()))
            .unwrap();
        let completion = observed
            .iter()
            .position(|event| matches!(event, BrowserEvent::NavigationFailed(_)))
            .unwrap();
        assert!(
            invalidation < completion,
            "{operation}: publish before navigation settles"
        );
        assert_eq!(driver.orientation_document, DocumentOrientation::NeedsMeasurement);
        let commands = classic.recorded();
        let mutation = commands
            .iter()
            .find(|command| !command.path.ends_with("/timeouts"))
            .unwrap();
        assert!(mutation.path.ends_with(match operation {
            "reload" => "/refresh",
            "back" => "/back",
            "forward" => "/forward",
            _ => "/url",
        }));
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }
}

#[derive(Debug)]
struct SlowPublication;
impl crate::BrowserCoordination for SlowPublication {
    fn prepare(&self, _: &str, _: Duration) -> bool {
        true
    }
    fn initialize(&self, _: &str, _: &crate::CoordinationState) -> std::io::Result<()> {
        Ok(())
    }
    fn update(&self, _: &str, state: &crate::CoordinationState) -> std::io::Result<()> {
        assert_eq!(state.remote_orientation.unwrap().applied, None);
        std::thread::sleep(Duration::from_millis(600));
        Ok(())
    }
    fn set_user_active(&self, _: &str, _: bool) -> std::io::Result<()> {
        Ok(())
    }
    fn signals(&self, _: &str) -> std::io::Result<crate::CoordinationSignals> {
        Ok(crate::CoordinationSignals::default())
    }
    fn acknowledge_handoff(&self, _: &str, _: &str) -> std::io::Result<bool> {
        Ok(false)
    }
    fn remove(&self, _: &str, _: Duration) -> bool {
        true
    }
}

#[test]
fn coordination_delay_cannot_dispatch_navigation_after_its_deadline() {
    let classic = Server::start(vec![Reply::json(200, &json!({"value":null}))]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    driver.config.browser.backend = crate::BackendKind::SafariWebDriver;
    driver.config.coordination = Some(Arc::new(SlowPublication));
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState {
        support: OrientationSupport::Supported,
        applied: Some(RemoteOrientation::Landscape),
    });
    let (tx, rx) = mpsc::channel();
    let events = BrowserEventSender { tx, ..events() };
    let mut request = rotation_request();
    request.action = BrowserControlAction::Navigate {
        url: "https://example.test/next".into(),
        wait: crate::NavigationWait::Commit,
        timeout_millis: Some(500),
    };
    let AgentActionExecution::Done(Ok(BrowserControlValue::Navigation { navigation })) =
        driver.navigate_action(&request, &events)
    else {
        panic!("navigation must settle with a typed timeout");
    };
    assert_eq!(navigation.state, crate::NavigationState::TimedOut);
    assert!(!navigation.loading);
    assert!(driver.classic_timeout_to_restore.is_some());
    assert!(!driver.classic_navigation_in_flight());
    assert_eq!(driver.orientation_document, DocumentOrientation::NeedsMeasurement);
    assert_eq!(driver.remote_orientation.unwrap().applied, None);
    let commands = classic.recorded();
    assert_eq!(commands.len(), 1, "only apply the session timeout; never POST /url");
    assert!(commands[0].path.ends_with("/timeouts"));
    assert!(!rx.try_iter().any(|event| matches!(event, BrowserEvent::Loading(true))));
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}
