//! Semantic scans and trusted input in Chromium child execution contexts.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::cdp::CdpLink;
use crate::frames::FrameSlot;
use crate::input::BrowserInputCdpExt;
use crate::semantic::{
    FrameTarget, MAX_SEMANTIC_FRAMES, append_frame_scan, bounded_control_value, frame_fill_expression,
    parse_target_rect, scan_expression, scan_node_limit_reached, target_rect_expression,
};
use crate::session::{BrowserEventSender, DriverState};
use crate::{BrowserControlFailure, BrowserControlValue};

use super::{pointer_press, pointer_release};

impl DriverState {
    pub(in crate::session) fn frame_scan(
        &mut self,
        link: &mut CdpLink,
        events: &BrowserEventSender,
        slot: &Arc<FrameSlot>,
        selector: Option<&str>,
        max_nodes: u32,
    ) -> Result<Value, BrowserControlFailure> {
        self.frame_scan_within(link, events, slot, selector, max_nodes, super::super::CALL_TIMEOUT)
    }

    pub(in crate::session) fn frame_scan_within(
        &mut self,
        link: &mut CdpLink,
        events: &BrowserEventSender,
        slot: &Arc<FrameSlot>,
        selector: Option<&str>,
        max_nodes: u32,
        timeout: Duration,
    ) -> Result<Value, BrowserControlFailure> {
        let deadline = Instant::now() + timeout;
        let revision = self.semantic.scan_revision();
        let expression = scan_expression(selector, max_nodes);
        let mut scan = self.evaluate_json_within(link, events, slot, &expression, remaining(deadline)?)?;
        crate::semantic::clear_scan_frames(&mut scan)?;
        if self.semantic.scan_revision() != revision {
            return Err(stale());
        }
        if scan_node_limit_reached(&scan, max_nodes) {
            return Ok(scan);
        }
        let page = self.session_id.clone().ok_or_else(stale)?;
        let mut sessions: Vec<_> = self.clipboard.iframe_sessions.iter().cloned().collect();
        sessions.push(page.clone());
        if sessions.len() > MAX_SEMANTIC_FRAMES {
            return Err(frame_limit());
        }
        for session in &sessions {
            self.queue_runtime_enable(link, session);
        }
        // Drain outstanding enables without re-enabling already observed worlds.
        while self
            .runtime_enable_inflight
            .values()
            .any(|session| sessions.contains(session))
        {
            remaining(deadline)?;
            if self.stop_requested.load(std::sync::atomic::Ordering::Acquire) {
                return Err(stale());
            }
            match link.read_one() {
                Ok(Some(message)) => self.handle_message(link, events, slot, message),
                Ok(None) => {}
                Err(_) => return Err(frame_unavailable()),
            }
        }
        if sessions
            .iter()
            .any(|session| !self.runtime_enable_requested.contains(session))
        {
            return Err(frame_unavailable());
        }
        let mut targets = self.clipboard.evaluation_targets(&page);
        targets.sort();
        if targets.len() > MAX_SEMANTIC_FRAMES {
            return Err(frame_limit());
        }
        for (session, context) in targets {
            if scan_node_limit_reached(&scan, max_nodes) {
                break;
            }
            let Some(context) = context else {
                continue;
            };
            let frame = FrameTarget::Cdp { session, context };
            let value = self.frame_value(
                link,
                events,
                slot,
                &frame,
                &format!("window === window.top ? null : ({expression})"),
                deadline,
            )?;
            if !value.is_null() {
                append_frame_scan(&mut scan, value, &frame, max_nodes)?;
            }
        }
        if self.semantic.scan_revision() != revision {
            return Err(stale());
        }
        Ok(scan)
    }

    fn frame_call(
        &mut self,
        link: &mut CdpLink,
        events: &BrowserEventSender,
        slot: &Arc<FrameSlot>,
        command: (&str, &str, &Value),
        deadline: Instant,
    ) -> Result<Value, BrowserControlFailure> {
        let (session, method, params) = command;
        let stop = Arc::clone(&self.stop_requested);
        let outcome = link.call_and_drain_until(remaining(deadline)?, method, params, Some(session), || {
            stop.load(std::sync::atomic::Ordering::Acquire)
        });
        for message in outcome.drained {
            self.handle_message(link, events, slot, message);
        }
        outcome.result.map_err(|_| frame_unavailable())
    }

    fn frame_value(
        &mut self,
        link: &mut CdpLink,
        events: &BrowserEventSender,
        slot: &Arc<FrameSlot>,
        frame: &FrameTarget,
        expression: &str,
        deadline: Instant,
    ) -> Result<Value, BrowserControlFailure> {
        let (session, context) = self.cdp_frame(frame)?;
        let result = self.frame_call(
            link,
            events,
            slot,
            (
                &session,
                "Runtime.evaluate",
                &json!({
                    "expression": expression, "contextId": context, "returnByValue": true,
                    "awaitPromise": true, "userGesture": true,
                }),
            ),
            deadline,
        );
        self.cdp_frame(frame)?;
        let result = result?;
        if result.get("exceptionDetails").is_some() {
            return Err(stale());
        }
        bounded_control_value(result.pointer("/result/value").cloned().unwrap_or(Value::Null))
    }

    fn cdp_frame(&self, frame: &FrameTarget) -> Result<(String, u64), BrowserControlFailure> {
        let FrameTarget::Cdp { session, context } = frame else {
            return Err(stale());
        };
        let page = self.session_id.as_deref().ok_or_else(stale)?;
        if !self
            .clipboard
            .evaluation_targets(page)
            .contains(&(session.clone(), Some(*context)))
        {
            return Err(stale());
        }
        Ok((session.clone(), *context))
    }

    pub(in crate::session) fn frame_fill(
        &mut self,
        link: &mut CdpLink,
        events: &BrowserEventSender,
        slot: &Arc<FrameSlot>,
        frame: &FrameTarget,
        selector: &str,
        value: &str,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let deadline = Instant::now() + super::super::CALL_TIMEOUT;
        let result = self.frame_value(link, events, slot, frame, &frame_fill_expression(selector), deadline)?;
        parse_target_rect(&result)?;
        let (session, _) = self.cdp_frame(frame)?;
        // Focus crosses the iframe boundary; the browser inserts native text.
        self.frame_call(
            link,
            events,
            slot,
            (&session, "Input.insertText", &json!({"text":value})),
            deadline,
        )?;
        Ok(BrowserControlValue::Accepted)
    }

    pub(in crate::session) fn frame_click(
        &mut self,
        link: &mut CdpLink,
        events: &BrowserEventSender,
        slot: &Arc<FrameSlot>,
        frame: &FrameTarget,
        selector: &str,
        count: u32,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let deadline = Instant::now() + super::super::CALL_TIMEOUT;
        let result = self.frame_value(
            link,
            events,
            slot,
            frame,
            &target_rect_expression(selector, false),
            deadline,
        )?;
        parse_target_rect(&result)?;
        let (session, context) = self.cdp_frame(frame)?;
        let expression = format!(
            "document.querySelector({})",
            serde_json::to_string(selector).map_err(|_| stale())?
        );
        let result = self.frame_call(
            link,
            events,
            slot,
            (
                &session,
                "Runtime.evaluate",
                &json!({
                    "expression":expression, "contextId":context, "returnByValue":false,
                }),
            ),
            deadline,
        )?;
        let object = result
            .pointer("/result/objectId")
            .and_then(Value::as_str)
            .ok_or_else(stale)?
            .to_owned();
        let result = (|| {
            let quads = self.frame_call(
                link,
                events,
                slot,
                (&session, "DOM.getContentQuads", &json!({"objectId":object})),
                deadline,
            )?;
            let (x, y) = quad_center(&quads)?;
            for click in 1..=count {
                self.cdp_frame(frame)?;
                for input in [pointer_press(x, y, click), pointer_release(x, y, click)] {
                    let (method, params) = input.cdp();
                    self.frame_call(link, events, slot, (&session, method, &params), deadline)?;
                }
            }
            Ok(BrowserControlValue::Accepted)
        })();
        let _ = self.frame_call(
            link,
            events,
            slot,
            (&session, "Runtime.releaseObject", &json!({"objectId":object})),
            Instant::now() + Duration::from_secs(1),
        );
        result
    }
}

fn quad_center(value: &Value) -> Result<(f64, f64), BrowserControlFailure> {
    let quad = value
        .pointer("/quads/0")
        .and_then(Value::as_array)
        .filter(|quad| quad.len() == 8)
        .ok_or_else(stale)?;
    let points = quad
        .iter()
        .map(|value| value.as_f64().filter(|value| value.is_finite()).ok_or_else(stale))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((
        (points[0] + points[2] + points[4] + points[6]) / 4.0,
        (points[1] + points[3] + points[5] + points[7]) / 4.0,
    ))
}

fn remaining(deadline: Instant) -> Result<Duration, BrowserControlFailure> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|value| !value.is_zero())
        .ok_or_else(|| BrowserControlFailure::new("frame_timeout", "frame observation exceeded its original bound"))
}
fn stale() -> BrowserControlFailure {
    BrowserControlFailure::new("stale_reference", "the child document changed; take a fresh snapshot")
}
fn frame_unavailable() -> BrowserControlFailure {
    BrowserControlFailure::new("frame_unavailable", "the frame command failed; take a fresh snapshot")
}
fn frame_limit() -> BrowserControlFailure {
    BrowserControlFailure::new("frame_limit", "the page exceeds the semantic frame limit")
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdp::CdpEvent;
    use crate::session::{BrowserEventWake, BrowserSessionConfig, CommittedUrl};
    use std::net::TcpListener;
    use std::sync::{atomic::AtomicBool, mpsc};
    use tungstenite::Message;

    fn fixture(url: &str) -> (DriverState, BrowserEventSender, Arc<FrameSlot>) {
        let slot = Arc::new(FrameSlot::default());
        let config = BrowserSessionConfig {
            browser: crate::BrowserConfig::default(),
            panel_local_id: "frame-protocol".into(),
            initial_url: None,
            width: 800,
            height: 600,
            frame_slot: Arc::clone(&slot),
            coordination: None,
            capture_directory: None,
            video: Arc::default(),
            remote: None,
        };
        let mut state = DriverState::new(&config, url, None, Arc::new(AtomicBool::new(false)));
        state.session_id = Some("page".into());
        state.clipboard.iframe_sessions.insert("child".into());
        let events = BrowserEventSender {
            tx: mpsc::channel().0,
            wake: BrowserEventWake::default(),
            committed_url: CommittedUrl::default(),
        };
        (state, events, slot)
    }

    #[test]
    fn scans_enable_once_and_failed_evaluations_recheck_the_route() {
        for event in [
            None,
            Some("Target.detachedFromTarget"),
            Some("Runtime.executionContextDestroyed"),
            Some("Runtime.executionContextsCleared"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("ws://{}/", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut socket = tungstenite::accept(stream).unwrap();
                let mut commands = Vec::new();
                let mut pending = Vec::new();
                while let Ok(Message::Text(text)) = socket.read() {
                    let command: Value = serde_json::from_str(&text).unwrap();
                    commands.push(command.clone());
                    let session = command["sessionId"].clone();
                    let response = json!({"id":command["id"],"result":{"result":{"value":{"nodes":[]}}}});
                    if command["method"] == "Runtime.enable" {
                        pending.push(json!({"method":"Runtime.executionContextCreated","sessionId":session,"params":{"context":{"id":7,"auxData":{"isDefault":true}}}}));
                        pending.push(json!({"id":command["id"],"result":{}}));
                        continue;
                    }
                    if command["params"]["expression"] == "fail" {
                        if let Some(method) = event {
                            socket.send(Message::Text(json!({"method":method,"sessionId":"child","params":{"sessionId":"child","executionContextId":7}}).to_string().into())).unwrap();
                        }
                        socket
                            .send(Message::Text(
                                json!({"id":command["id"],"error":{"code":-32000,"message":"evaluation failed"}})
                                    .to_string()
                                    .into(),
                            ))
                            .unwrap();
                    } else {
                        socket.send(Message::Text(response.to_string().into())).unwrap();
                        for response in pending.drain(..) {
                            socket.send(Message::Text(response.to_string().into())).unwrap();
                        }
                    }
                }
                commands
            });
            let (mut state, events, slot) = fixture(&url);
            let mut link = CdpLink::connect_with_timeout(&url, Duration::from_millis(10)).unwrap();
            if event.is_none() {
                for _ in 0..2 {
                    state.frame_scan(&mut link, &events, &slot, None, 10).unwrap();
                    assert!(state.runtime_enable_inflight.is_empty());
                }
            } else {
                state.note_clipboard_execution_context(&CdpEvent {
                    method: "Runtime.executionContextCreated",
                    session_id: Some("child"),
                    params: &json!({"context":{"id":7,"auxData":{"isDefault":true}}}),
                });
            }
            let frame = FrameTarget::Cdp {
                session: "child".into(),
                context: 7,
            };
            let error = state
                .frame_value(
                    &mut link,
                    &events,
                    &slot,
                    &frame,
                    "fail",
                    Instant::now() + Duration::from_secs(2),
                )
                .unwrap_err();
            assert_eq!(
                error.code,
                if event.is_some() {
                    "stale_reference"
                } else {
                    "frame_unavailable"
                }
            );
            drop(link);
            let commands = server.join().unwrap();
            assert_eq!(
                commands
                    .iter()
                    .filter(|command| command["method"] == "Runtime.enable")
                    .count(),
                if event.is_none() { 2 } else { 0 }
            );
            if event.is_none() {
                assert_eq!(
                    commands
                        .iter()
                        .filter(|command| command["sessionId"] == "child" && command["method"] == "Runtime.evaluate")
                        .count(),
                    3
                );
            }
        }
    }
    #[test]
    fn multi_frame_scan_rejects_an_earlier_child_invalidated_during_a_later_scan() {
        for (event, affected) in [
            (None, "child"),
            (Some("Runtime.executionContextDestroyed"), "child"),
            (Some("Runtime.executionContextsCleared"), "child"),
            (Some("Target.detachedFromTarget"), "child"),
            (Some("Runtime.executionContextDestroyed"), "foreign"),
            (Some("Runtime.executionContextsCleared"), "foreign"),
            (Some("Target.detachedFromTarget"), "foreign"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("ws://{}/", listener.local_addr().unwrap());
            let worker = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut socket = tungstenite::accept(stream).unwrap();
                let mut sessions = Vec::new();
                while let Ok(Message::Text(text)) = socket.read() {
                    let command: Value = serde_json::from_str(&text).unwrap();
                    assert_eq!(command["method"], "Runtime.evaluate");
                    let session = command["sessionId"].as_str().unwrap_or("page");
                    sessions.push(session.to_owned());
                    if session == "later"
                        && let Some(method) = event
                    {
                        socket.send(Message::Text(json!({"method":method,"sessionId":affected,"params":{"sessionId":affected,"executionContextId":7}}).to_string().into())).unwrap();
                    }
                    let value = if session == "page" {
                        Value::Null
                    } else {
                        json!({"nodes":[{"selector":"#field"}]})
                    };
                    let response = json!({"id":command["id"],"result":{"result":{"value":if command["params"]["contextId"].is_null() { json!({"nodes":[]}) } else { value }}}});
                    socket.send(Message::Text(response.to_string().into())).unwrap();
                }
                sessions
            });
            let (mut state, events, slot) = fixture(&url);
            state.clipboard.iframe_sessions.insert("later".into());
            for session in ["page", "child", "later"] {
                state.runtime_enable_requested.insert(session.into());
                state.note_clipboard_execution_context(&CdpEvent {
                    method: "Runtime.executionContextCreated",
                    session_id: Some(session),
                    params: &json!({"context":{"id":7,"auxData":{"isDefault":true}}}),
                });
            }
            let (generation, _, nodes) = state
                .semantic
                .register_nodes(json!({"nodes":[{"selector":"#top"}]}))
                .unwrap();
            let mut link = CdpLink::connect_with_timeout(&url, Duration::from_millis(10)).unwrap();
            let result = state.frame_scan(&mut link, &events, &slot, None, 10);
            if event.is_some() && affected == "child" {
                assert_eq!(result.unwrap_err().code, "stale_reference");
            } else {
                assert_eq!(result.unwrap()["nodes"].as_array().unwrap().len(), 2);
            }
            assert_eq!(state.semantic.generation(), generation);
            assert_eq!(
                state
                    .semantic
                    .resolve(&crate::BrowserTarget::Ref {
                        reference: nodes[0].reference.clone()
                    })
                    .unwrap(),
                "#top"
            );
            drop(link);
            assert_eq!(worker.join().unwrap(), ["page", "child", "later", "page"]);
        }
    }
}
