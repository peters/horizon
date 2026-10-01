use super::*;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Mutex;

struct Transport {
    replies: Mutex<VecDeque<Result<Value, HttpError>>>,
    calls: Mutex<Vec<(String, String, Option<Value>)>>,
}
impl Transport {
    fn new(replies: Vec<Result<Value, HttpError>>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            calls: Mutex::new(Vec::new()),
        }
    }
}
impl ClassicTransport for Transport {
    fn request(&self, method: &str, path: &str, body: Option<&Value>, _: Duration) -> Result<Value, HttpError> {
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
    for (reply, support, applied) in [
        (
            Ok(json!({"value":"LANDSCAPE"})),
            OrientationSupport::Supported,
            Some(RemoteOrientation::Landscape),
        ),
        (Ok(json!({"value":"invalid"})), OrientationSupport::Unverified, None),
        (
            Err(HttpError::WebDriver {
                error: "unknown command".into(),
                message: "unsupported".into(),
            }),
            OrientationSupport::Unsupported,
            None,
        ),
        (
            Err(HttpError::WebDriver {
                error: "unknown error".into(),
                message: "temporary".into(),
            }),
            OrientationSupport::Unverified,
            None,
        ),
    ] {
        let transport = Transport::new(vec![reply]);
        assert_eq!(
            probe(&transport, "/session/example"),
            RemoteOrientationState { support, applied }
        );
        assert_eq!(transport.calls.lock().unwrap().len(), 1);
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
fn expired_action_and_cancelled_start_do_not_send_mutation() {
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
    assert_eq!(
        verify_start(&transport, "/session/example", RemoteOrientation::Landscape, || true)
            .unwrap_err()
            .code,
        "browser_unavailable"
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
fn ignored_start_orientation_is_a_mismatch_without_a_runtime_mutation() {
    let transport = Transport::new(vec![Ok(json!({"value":"PORTRAIT"}))]);
    let error = verify_start_until(
        &transport,
        "/session/example",
        RemoteOrientation::Landscape,
        || false,
        Instant::now() + Duration::from_millis(20),
    )
    .unwrap_err();
    assert_eq!(error.code, "remote_orientation_mismatch");
    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "GET");
}

#[test]
fn start_observation_failures_remain_unverified() {
    for replies in [
        vec![Err(HttpError::WebDriver {
            error: "unknown error".into(),
            message: "private detail".into(),
        })],
        vec![Ok(json!({"value":"invalid"}))],
        vec![Ok(json!({"value":"LANDSCAPE"})), Ok(json!({"value":null}))],
        vec![Ok(json!({"value":"LANDSCAPE"})), Ok(page(0, 0, &Value::Null))],
    ] {
        let transport = Transport::new(replies);
        let error = verify_start_until(
            &transport,
            "/session/example",
            RemoteOrientation::Landscape,
            || false,
            Instant::now() + Duration::from_millis(20),
        )
        .unwrap_err();
        assert_eq!(error.code, "orientation_unverified");
        assert!(!error.message.contains("private detail"));
    }
}

#[test]
fn cancellation_after_observation_preserves_browser_unavailable() {
    let transport = Transport::new(vec![
        Ok(json!({"value":"LANDSCAPE"})),
        Ok(page(900, 600, &json!("landscape"))),
    ]);
    let error = verify_start_until(
        &transport,
        "/session/example",
        RemoteOrientation::Landscape,
        || !transport.calls.lock().unwrap().is_empty(),
        Instant::now() + Duration::from_millis(20),
    )
    .unwrap_err();
    assert_eq!(error.code, "browser_unavailable");
}
