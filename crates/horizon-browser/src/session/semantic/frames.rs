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
        let generation = self.semantic.generation();
        let expression = scan_expression(selector, max_nodes);
        let mut scan = self.evaluate_json_within(link, events, slot, &expression, remaining(deadline)?)?;
        crate::semantic::clear_scan_frames(&mut scan)?;
        if self.semantic.generation() != generation {
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
        // Wait for enable replies so every attached frame has reported its worlds.
        for session in sessions {
            self.frame_call(link, events, slot, (&session, "Runtime.enable", &json!({})), deadline)?;
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
        if self.semantic.generation() != generation {
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
        outcome.result.map_err(|_| {
            BrowserControlFailure::new("frame_unavailable", "the frame command failed; take a fresh snapshot")
        })
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
        )?;
        self.cdp_frame(frame)?;
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
fn frame_limit() -> BrowserControlFailure {
    BrowserControlFailure::new("frame_limit", "the page exceeds the semantic frame limit")
}
