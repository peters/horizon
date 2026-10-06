//! Native metadata from the temporary disclosure target.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::cdp::{CdpError, CdpLink};
use crate::disclosure::CHROMIUM_USER_AGENT_METADATA_EXPRESSION;

pub(super) fn read(link: &mut CdpLink, stop: &AtomicBool, session: &str, timeout: Duration) -> Result<Value, String> {
    link.call_and_drain_until(timeout, "Runtime.enable", &json!({}), Some(session), || {
        stop.load(Ordering::Acquire)
    })
    .result
    .map_err(|error| format!("Runtime.enable: {error}"))?;
    // Target creation returns before its navigation commits. The initial blank
    // document has no userAgentData; its context can also disappear mid-call.
    let deadline = Instant::now() + timeout;
    loop {
        if stop.load(Ordering::Acquire) {
            return Err("Runtime.evaluate: native metadata wait cancelled".into());
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err("Runtime.evaluate: native metadata was not ready before startup deadline".into());
        };
        let result = link
            .call_and_drain_until(
                remaining,
                "Runtime.evaluate",
                &json!({
                    "expression": CHROMIUM_USER_AGENT_METADATA_EXPRESSION,
                    "awaitPromise": true,
                    "returnByValue": true,
                }),
                Some(session),
                || stop.load(Ordering::Acquire),
            )
            .result;
        match result {
            Ok(value) if value.get("exceptionDetails").is_some() => {
                return Err("Runtime.evaluate: native metadata evaluation raised an exception".into());
            }
            Ok(value) if value.pointer("/result/value").is_some_and(Value::is_object) => return Ok(value),
            Ok(_) => {}
            Err(error) if navigation_replaced_context(&error) => {}
            Err(error) => return Err(format!("Runtime.evaluate: {error}")),
        }
        std::thread::sleep(Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn navigation_replaced_context(error: &CdpError) -> bool {
    matches!(error, CdpError::Response { code: -32000, message }
        if message.contains("Execution context was destroyed")
            || message.contains("Cannot find context with specified id"))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::thread;

    use tungstenite::Message;

    use super::*;

    fn fixture(answers: Vec<Value>, cancel: Option<Arc<AtomicBool>>) -> (CdpLink, thread::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("fixture connection");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("fixture timeout");
            let mut socket = tungstenite::accept(stream).expect("fixture handshake");
            let mut answers = VecDeque::from(answers);
            let mut evaluations = 0;
            while let Ok(Message::Text(text)) = socket.read() {
                let command: Value = serde_json::from_str(&text).expect("fixture command");
                assert_eq!(command["sessionId"], "disclosure-session");
                let result = if command["method"] == "Runtime.evaluate" {
                    assert_eq!(command["params"]["awaitPromise"], true);
                    evaluations += 1;
                    if let Some(stop) = &cancel {
                        stop.store(true, Ordering::Release);
                    }
                    answers
                        .pop_front()
                        .unwrap_or_else(|| json!({"result":{"type":"object","subtype":"null","value":null}}))
                } else {
                    assert_eq!(command["method"], "Runtime.enable");
                    json!({})
                };
                let response = if let Some(error) = result.get("fixture_error") {
                    json!({"id":command["id"],"error":error})
                } else {
                    json!({"id":command["id"],"result":result})
                };
                socket
                    .send(Message::Text(response.to_string().into()))
                    .expect("fixture response");
            }
            evaluations
        });
        (
            CdpLink::connect(&format!("ws://{address}/")).expect("fixture transport"),
            worker,
        )
    }

    #[test]
    fn waits_for_native_metadata_after_the_bootstrap_navigation() {
        let native = json!({"result":{"type":"object","value":{
            "platform":"Linux", "platformVersion":"6.8.0", "architecture":"x86",
            "model":"", "mobile":false, "brands":[], "fullVersionList":[]
        }}});
        let (mut link, worker) = fixture(
            vec![
                json!({"result":{"value":null}}),
                json!({"fixture_error":{"code":-32000,"message":"Execution context was destroyed."}}),
                native.clone(),
            ],
            None,
        );
        let result = read(
            &mut link,
            &AtomicBool::new(false),
            "disclosure-session",
            Duration::from_secs(1),
        );
        drop(link);
        let evaluations = worker.join().expect("fixture exit");
        assert_eq!(result.expect("native metadata"), native);
        assert_eq!(evaluations, 3);
    }

    #[test]
    fn missing_metadata_has_a_finite_startup_deadline() {
        let (mut link, worker) = fixture(Vec::new(), None);
        let result = read(
            &mut link,
            &AtomicBool::new(false),
            "disclosure-session",
            Duration::from_millis(50),
        );
        drop(link);
        worker.join().expect("fixture exit");
        assert!(result.is_err(), "an unavailable native identity must fail closed");
    }

    #[test]
    fn cancellation_interrupts_the_metadata_wait() {
        let stop = Arc::new(AtomicBool::new(false));
        let (mut link, worker) = fixture(Vec::new(), Some(Arc::clone(&stop)));
        let result = read(&mut link, &stop, "disclosure-session", Duration::from_secs(1));
        drop(link);
        worker.join().expect("fixture exit");
        assert!(result.is_err(), "a cancelled metadata read must not report success");
    }

    #[test]
    fn unrelated_protocol_refusals_are_not_retried() {
        let (mut link, worker) = fixture(
            vec![json!({"fixture_error":{"code":-32000,"message":"Access denied"}})],
            None,
        );
        let result = read(
            &mut link,
            &AtomicBool::new(false),
            "disclosure-session",
            Duration::from_secs(1),
        );
        drop(link);
        assert_eq!(worker.join().expect("fixture exit"), 1);
        assert!(result.expect_err("protocol refusal").contains("Access denied"));
    }
}
