use super::*;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Mutex;

struct Transport {
    replies: Mutex<VecDeque<Result<Value, HttpError>>>,
    calls: Mutex<Vec<(String, String, Option<Value>)>>,
    timeouts: Mutex<Vec<Duration>>,
}
impl Transport {
    fn new(replies: Vec<Result<Value, HttpError>>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            calls: Mutex::new(Vec::new()),
            timeouts: Mutex::new(Vec::new()),
        }
    }
}
impl ClassicTransport for Transport {
    fn request(&self, method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Result<Value, HttpError> {
        self.timeouts.lock().unwrap().push(timeout);
        self.calls
            .lock()
            .unwrap()
            .push((method.into(), path.into(), body.cloned()));
        self.replies.lock().unwrap().pop_front().expect("mock response")
    }
}
fn page(width: u32, height: u32, orientation: &Value) -> Value {
    json!({"value":{"width":width,"height":height,"visual_width":width,"visual_height":height,"orientation":orientation}})
}
#[test]
fn negotiation_distinguishes_unsupported_unverified_and_confirmed() {
    for (replies, support, applied) in [
        (
            vec![
                Ok(json!({"value":"LANDSCAPE"})),
                Ok(page(900, 600, &json!("landscape"))),
            ],
            OrientationSupport::Supported,
            Some(RemoteOrientation::Landscape),
        ),
        (
            vec![Ok(json!({"value":"LANDSCAPE"})), Ok(page(600, 900, &json!("portrait")))],
            OrientationSupport::Supported,
            None,
        ),
        (
            vec![
                Ok(json!({"value":"LANDSCAPE"})),
                Err(HttpError::WebDriver {
                    error: "javascript error".into(),
                    message: "page unavailable".into(),
                }),
            ],
            OrientationSupport::Supported,
            None,
        ),
        (
            vec![Ok(json!({"value":"invalid"}))],
            OrientationSupport::Unverified,
            None,
        ),
        (
            vec![Err(HttpError::WebDriver {
                error: "unknown command".into(),
                message: "unsupported".into(),
            })],
            OrientationSupport::Unsupported,
            None,
        ),
        (
            vec![Err(HttpError::WebDriver {
                error: "unknown error".into(),
                message: "temporary".into(),
            })],
            OrientationSupport::Unverified,
            None,
        ),
    ] {
        let call_count = replies.len();
        let transport = Transport::new(replies);
        assert_eq!(
            probe(&transport, "/session/example"),
            RemoteOrientationState { support, applied }
        );
        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls.len(), call_count);
        assert_eq!(calls[0].0, "GET");
        if call_count == 2 {
            assert_eq!(calls[1].1, "/session/example/execute/sync");
            let timeouts = transport.timeouts.lock().unwrap();
            assert!(timeouts[1] < timeouts[0], "both observations share one deadline");
        }
    }
}

#[test]
fn round_trip_requires_device_and_page_acknowledgement() {
    let transport = Transport::new(vec![
        Ok(json!({"value":null})),
        Ok(json!({"value":"LANDSCAPE"})),
        Ok(page(1106, 820, &json!("landscape-primary"))),
        Ok(json!({"value":null})),
        Ok(json!({"value":"PORTRAIT"})),
        Ok(page(820, 1106, &json!("portrait"))),
    ]);
    let deadline = Instant::now() + Duration::from_secs(1);
    for orientation in [RemoteOrientation::Landscape, RemoteOrientation::Portrait] {
        set(&transport, "/session/example", orientation, deadline).unwrap();
        assert!(
            observe(&transport, "/session/example", orientation, deadline)
                .unwrap()
                .is_some()
        );
    }
    let calls = transport.calls.lock().unwrap();
    assert_eq!(
        calls[0],
        (
            "POST".into(),
            "/session/example/orientation".into(),
            Some(json!({"orientation":"LANDSCAPE"}))
        )
    );
    assert_eq!(calls[3].2, Some(json!({"orientation":"PORTRAIT"})));
}
#[test]
fn geometry_and_orientation_disagreement_cannot_pass() {
    for measured in [
        page(820, 1106, &json!("landscape")),
        page(1106, 820, &json!("portrait-primary")),
        page(0, 0, &Value::Null),
        json!({"value":{"width":1106,"height":820,"visual_width":400,"visual_height":800,"orientation":"landscape"}}),
    ] {
        let transport = Transport::new(vec![Ok(json!({"value":"LANDSCAPE"})), Ok(measured)]);
        assert_eq!(
            observe(
                &transport,
                "/session/example",
                RemoteOrientation::Landscape,
                Instant::now() + Duration::from_secs(1)
            )
            .unwrap_err()
            .code,
            "orientation_unverified"
        );
    }
    let transport = Transport::new(vec![Ok(json!({"value":"PORTRAIT"}))]);
    assert!(
        observe(
            &transport,
            "/session/example",
            RemoteOrientation::Landscape,
            Instant::now() + Duration::from_secs(1)
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
}
#[test]
fn expired_action_does_not_send_mutation() {
    let transport = Transport::new(vec![]);
    assert_eq!(
        set(
            &transport,
            "/session/example",
            RemoteOrientation::Landscape,
            Instant::now()
        )
        .unwrap_err()
        .code,
        "orientation_timeout"
    );
    assert!(transport.calls.lock().unwrap().is_empty());
}
#[test]
fn unsupported_rotation_has_a_typed_refusal() {
    let transport = Transport::new(vec![Err(HttpError::WebDriver {
        error: "unknown command".into(),
        message: "secret-bearing provider detail".into(),
    })]);
    let error = set(
        &transport,
        "/session/example",
        RemoteOrientation::Portrait,
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap_err();
    assert_eq!(error.code, "orientation_unsupported");
    assert!(!error.message.contains("secret"));
}

#[test]
fn expired_post_response_is_a_timeout_even_when_mutation_may_have_succeeded() {
    struct DeadlineTransport(bool);
    impl ClassicTransport for DeadlineTransport {
        fn request(
            &self,
            method: &str,
            path: &str,
            body: Option<&Value>,
            timeout: Duration,
        ) -> Result<Value, HttpError> {
            assert_eq!(method, "POST");
            assert_eq!(path, "/session/example/orientation");
            assert_eq!(body, Some(&json!({"orientation":"LANDSCAPE"})));
            std::thread::sleep(timeout + Duration::from_millis(10));
            if self.0 {
                Ok(json!({"value":null}))
            } else {
                Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into())
            }
        }
    }
    for late_success in [false, true] {
        let error = set(
            &DeadlineTransport(late_success),
            "/session/example",
            RemoteOrientation::Landscape,
            Instant::now() + Duration::from_millis(20),
        )
        .unwrap_err();
        assert_eq!(error.code, "orientation_timeout");
        assert!(error.message.contains("may already have rotated"));
    }
}

#[test]
fn immediate_post_transport_failure_remains_unverified() {
    let transport = Transport::new(vec![Err(HttpError::Transport("private provider detail".into()))]);
    let error = set(
        &transport,
        "/session/example",
        RemoteOrientation::Landscape,
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap_err();
    assert_eq!(error.code, "orientation_unverified");
    assert!(error.message.contains("may already have rotated"));
    assert!(!error.message.contains("private"));
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
}

#[test]
fn allocation_support_discovery_never_measures_or_applies_temporary_page_orientation() {
    for (reply, expected) in [
        (Ok(json!({"value":"PORTRAIT"})), OrientationSupport::Supported),
        (Ok(json!({"value":"LANDSCAPE"})), OrientationSupport::Supported),
        (Ok(json!({"value":"invalid"})), OrientationSupport::Unverified),
        (
            Err(HttpError::WebDriver {
                error: "unknown command".into(),
                message: "unsupported".into(),
            }),
            OrientationSupport::Unsupported,
        ),
        (
            Err(HttpError::WebDriver {
                error: "unknown error".into(),
                message: "private transport detail".into(),
            }),
            OrientationSupport::Unverified,
        ),
    ] {
        let transport = Transport::new(vec![reply]);
        assert_eq!(
            probe_support(&transport, "/session/example"),
            RemoteOrientationState {
                support: expected,
                applied: None
            }
        );
        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], ("GET".into(), "/session/example/orientation".into(), None));
        assert_eq!(*transport.timeouts.lock().unwrap(), vec![Duration::from_secs(3)]);
    }
}
