use super::*;
use crate::{contract::Platform, driver::References, recipe::Action};
use horizon_browser::{ClassicTransport, WebDriverHttpError};
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

type Reply = std::result::Result<Value, WebDriverHttpError>;
type RecordedCall = (String, String, Option<Value>, Duration);
struct Scripted {
    replies: Mutex<VecDeque<Reply>>,
    calls: Mutex<Vec<RecordedCall>>,
    alert_latency: Duration,
}
impl ClassicTransport for Scripted {
    fn request(&self, method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Reply {
        self.calls
            .lock()
            .unwrap()
            .push((method.into(), path.into(), body.cloned(), timeout));
        if path.ends_with("/alert/text") && !self.alert_latency.is_zero() {
            std::thread::sleep(self.alert_latency.min(timeout));
            if timeout < self.alert_latency {
                return Err(WebDriverHttpError::Io(std::io::Error::from(
                    std::io::ErrorKind::TimedOut,
                )));
            }
        }
        self.replies.lock().unwrap().pop_front().unwrap_or_else(|| {
            Err(WebDriverHttpError::WebDriver {
                error: "no such alert".into(),
                message: "No alert is open".into(),
            })
        })
    }
}

fn fixture(platform: Platform, replies: Vec<Reply>) -> (NativeDriver, Arc<Scripted>) {
    fixture_with_alert_latency(platform, replies, Duration::ZERO)
}

fn fixture_with_alert_latency(
    platform: Platform,
    replies: Vec<Reply>,
    alert_latency: Duration,
) -> (NativeDriver, Arc<Scripted>) {
    let transport = Arc::new(Scripted {
        replies: Mutex::new(replies.into()),
        calls: Mutex::new(Vec::new()),
        alert_latency,
    });
    let driver = NativeDriver {
        transport: transport.clone(),
        session_id: "private-session".into(),
        app_id: "com.example.app".into(),
        platform,
        arguments: BTreeMap::from([("API_KEY".into(), "private-api-key".into())]),
        references: References::default(),
        closed: false,
        deadline: None,
        lifetime_deadline: None,
    };
    (driver, transport)
}

#[test]
fn ios_deep_link_accepts_only_the_named_open_confirmation() {
    let (mut driver, transport) = fixture(
        Platform::Ios,
        vec![
            Ok(json!({"value": null})),
            Ok(json!({"value": "Open in “Example”?"})),
            Ok(json!({"value": ["Cancel", "Open"]})),
            Ok(json!({"value": null})),
        ],
    );
    driver
        .act(&Action::DeepLink {
            url: "myapp://debug/state".into(),
        })
        .unwrap();
    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(
        calls[0].2.as_ref().unwrap()["args"][0],
        json!({"bundleId": "com.example.app", "url": "myapp://debug/state"})
    );
    assert_eq!(calls[1].1, "/session/private-session/alert/text");
    assert_eq!(
        calls[3].2.as_ref().unwrap()["args"][0],
        json!({"action": "accept", "buttonLabel": "Open"})
    );
}

#[test]
fn permission_and_unexpected_buttons_are_never_accepted() {
    for (text, buttons) in [
        ("Allow Example to use your location?", vec!["Cancel", "Open"]),
        ("Open in “Example”?", vec!["Don't Allow", "Allow"]),
        ("Open in “Example”?", vec!["Open"]),
    ] {
        let (mut driver, transport) = fixture(
            Platform::Ios,
            vec![
                Ok(json!({"value": null})),
                Ok(json!({"value": text})),
                Ok(json!({"value": buttons})),
            ],
        );
        assert_eq!(
            driver.act(&Action::DeepLink {
                url: "myapp://debug/state".into()
            }),
            Err(Error::DeepLinkConfirmationBlocked)
        );
        assert!(!transport.calls.lock().unwrap().iter().any(|call| {
            call.2
                .as_ref()
                .is_some_and(|body| body["args"][0]["action"] == "accept")
        }));
    }
}

#[test]
fn unsupported_driver_keeps_sanitized_reason_without_replaying_the_link() {
    let (mut driver, transport) = fixture(Platform::Ios, vec![Err(WebDriverHttpError::WebDriver {
        error: "unknown command".into(),
        message: "mobile: deepLink requires Xcode 14.3. URL myapp://debug/state in private-session\nAPI_KEY=private-api-key".into(),
    })]);
    let error = driver
        .act(&Action::DeepLink {
            url: "myapp://debug/state".into(),
        })
        .unwrap_err();
    assert!(matches!(error, Error::ActionUnsupported(_)));
    let text = error.to_string();
    assert!(text.contains("unknown command") && text.contains("Xcode 14.3"));
    for private in ["myapp://debug/state", "private-session", "private-api-key"] {
        assert!(!text.contains(private));
    }
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
}

#[test]
fn transport_loss_does_not_retry_an_effectful_link() {
    let (mut driver, transport) = fixture(
        Platform::Ios,
        vec![Err(WebDriverHttpError::Io(std::io::Error::from(
            std::io::ErrorKind::TimedOut,
        )))],
    );
    assert_eq!(
        driver.act(&Action::DeepLink {
            url: "myapp://debug/state".into()
        }),
        Err(Error::TransportFailed)
    );
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
}

#[test]
fn android_keeps_its_package_deep_link_without_ios_alert_commands() {
    let (mut driver, transport) = fixture(Platform::Android, vec![Ok(json!({"value": null}))]);
    driver
        .act(&Action::DeepLink {
            url: "myapp://debug/state".into(),
        })
        .unwrap();
    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].2.as_ref().unwrap()["args"][0],
        json!({"package": "com.example.app", "url": "myapp://debug/state"})
    );
}

#[test]
fn expired_lifetime_never_starts_a_link() {
    let (mut driver, transport) = fixture(Platform::Ios, vec![]);
    driver.lifetime_deadline = Some(Instant::now());
    assert_eq!(
        driver.act(&Action::DeepLink {
            url: "myapp://debug/state".into()
        }),
        Err(Error::WaitTimeout)
    );
    assert!(transport.calls.lock().unwrap().is_empty());
}

#[test]
fn missing_confirmation_leaves_a_successful_link_successful() {
    let (mut driver, transport) = fixture(Platform::Ios, vec![Ok(json!({"value": null}))]);
    driver
        .act(&Action::DeepLink {
            url: "myapp://debug/state".into(),
        })
        .unwrap();
    assert!(
        transport
            .calls
            .lock()
            .unwrap()
            .iter()
            .skip(1)
            .all(|call| call.0 == "GET")
    );
}

#[test]
fn delayed_no_alert_responses_keep_a_successful_link_successful() {
    let latency = Duration::from_millis(500);
    let (mut driver, transport) = fixture_with_alert_latency(Platform::Ios, vec![Ok(json!({"value": null}))], latency);
    driver
        .act(&Action::DeepLink {
            url: "myapp://debug/state".into(),
        })
        .unwrap();
    let calls = transport.calls.lock().unwrap();
    assert!(calls.len() >= 2);
    assert!(calls.iter().skip(1).all(|call| call.0 == "GET" && call.3 >= latency));
    assert!(calls.iter().all(|call| call.3 <= super::super::COMMAND_TIMEOUT));
}

#[test]
fn a_confirmation_after_the_poll_window_can_finish_within_the_command_budget() {
    let (mut driver, transport) = fixture_with_alert_latency(
        Platform::Ios,
        vec![
            Ok(json!({"value": null})),
            Ok(json!({"value": "Open in “Example”?"})),
            Ok(json!({"value": ["Cancel", "Open"]})),
            Ok(json!({"value": null})),
        ],
        Duration::from_millis(2100),
    );
    driver
        .act(&Action::DeepLink {
            url: "myapp://debug/state".into(),
        })
        .unwrap();
    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(
        calls.last().unwrap().2.as_ref().unwrap()["args"][0],
        json!({"action": "accept", "buttonLabel": "Open"})
    );
}

#[test]
fn confirmation_poll_cannot_succeed_after_the_original_lifetime() {
    let (mut driver, transport) = fixture(Platform::Ios, vec![Ok(json!({"value": null}))]);
    driver.lifetime_deadline = Some(Instant::now() + Duration::from_millis(30));
    assert_eq!(
        driver.act(&Action::DeepLink {
            url: "myapp://debug/state".into()
        }),
        Err(Error::WaitTimeout)
    );
    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].3 <= Duration::from_millis(30));
}
