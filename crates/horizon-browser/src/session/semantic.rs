//! Semantic DOM actions executed on the Chromium driver's bound page.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::frames::FrameSlot;
use crate::input::BrowserInputCdpExt;
use crate::semantic::{
    bounded_control_value, check_script_error, parse_target_rect, scan_expression, scroll_expression,
    target_rect_expression, wait_scan_expression,
};
use crate::{
    AgentAction, BrowserButton, BrowserControlAction, BrowserControlFailure, BrowserControlValue, BrowserInput,
    BrowserModifiers, BrowserSnapshot,
};

use super::{BrowserEventSender, DriverState};

impl DriverState {
    pub(super) fn execute_agent_action(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        request: &AgentAction,
    ) -> (Result<BrowserControlValue, BrowserControlFailure>, bool) {
        if let Some(command) = request.action.to_command() {
            return match self.dispatch_command(link, event_tx, frame_slot, command, false) {
                Ok(stop) => (Ok(BrowserControlValue::Accepted), stop),
                Err(error) => (Err(error), false),
            };
        }
        let result = match &request.action {
            BrowserControlAction::Snapshot { max_nodes } => {
                self.semantic_snapshot(link, event_tx, frame_slot, *max_nodes)
            }
            BrowserControlAction::Query { selector, max_results } => {
                self.semantic_query(link, event_tx, frame_slot, selector, *max_results)
            }
            BrowserControlAction::WaitForSelector { .. } => Err(BrowserControlFailure::new(
                "invalid_action_state",
                "selector waits are observed from the driver loop",
            )),
            BrowserControlAction::Click { target, count } => {
                self.semantic_click(link, event_tx, frame_slot, target, *count)
            }
            BrowserControlAction::Fill { target, value } => {
                self.semantic_fill(link, event_tx, frame_slot, target, value)
            }
            BrowserControlAction::Scroll {
                target,
                delta_x,
                delta_y,
            } => self.semantic_scroll(link, event_tx, frame_slot, target.as_ref(), *delta_x, *delta_y),
            BrowserControlAction::Evaluate { expression } => {
                self.semantic_evaluate(link, event_tx, frame_slot, expression)
            }
            BrowserControlAction::Network { operation, options } => {
                self.network_action(link, event_tx, frame_slot, request, *operation, options.clone())
            }
            BrowserControlAction::Navigate { .. }
            | BrowserControlAction::Reload
            | BrowserControlAction::Back
            | BrowserControlAction::Forward
            | BrowserControlAction::Input { .. } => Ok(BrowserControlValue::Accepted),
        };
        (result, false)
    }

    fn semantic_snapshot(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        max_nodes: u32,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let value = self.evaluate_json(link, event_tx, frame_slot, &scan_expression(None, max_nodes))?;
        let (generation, revision, nodes) = self.semantic.register_nodes(value)?;
        Ok(BrowserControlValue::Snapshot {
            snapshot: BrowserSnapshot {
                generation,
                revision,
                url: self.url.clone(),
                title: self.title.clone(),
                nodes,
            },
        })
    }

    pub(super) fn semantic_query(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        selector: &str,
        max_results: u32,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        self.semantic_query_within(link, event_tx, frame_slot, selector, max_results, super::CALL_TIMEOUT)
    }

    /// A selector scan that does not register references: for judging a wait
    /// condition without disturbing refs a concurrent snapshot handed out.
    /// Returns the page generation, the peeked nodes, and the raw scan.
    pub(super) fn semantic_peek_within(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        selector: &str,
        max_results: u32,
        timeout: std::time::Duration,
    ) -> Result<
        (
            u64,
            Vec<crate::BrowserNode>,
            Option<crate::semantic::ScanSummary>,
            Value,
        ),
        BrowserControlFailure,
    > {
        let value = self.evaluate_json_within(
            link,
            event_tx,
            frame_slot,
            &wait_scan_expression(selector, max_results),
            timeout,
        )?;
        let peeked = self.semantic.peek_nodes(&value)?;
        Ok((peeked.generation, peeked.nodes, peeked.summary, value))
    }

    /// Register a previously peeked scan as the current references.
    pub(super) fn semantic_register_scan(
        &mut self,
        value: Value,
    ) -> Result<(u64, u64, Vec<crate::BrowserNode>), BrowserControlFailure> {
        self.semantic.register_nodes(value)
    }

    /// A selector query whose page evaluation may block for at most `timeout`.
    pub(super) fn semantic_query_within(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        selector: &str,
        max_results: u32,
        timeout: std::time::Duration,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let value = self.evaluate_json_within(
            link,
            event_tx,
            frame_slot,
            &scan_expression(Some(selector), max_results),
            timeout,
        )?;
        let (generation, revision, nodes) = self.semantic.register_nodes(value)?;
        Ok(BrowserControlValue::Nodes {
            generation,
            revision,
            nodes,
        })
    }

    fn semantic_click(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        target: &crate::BrowserTarget,
        count: u32,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let selector = self.semantic.resolve(target)?;
        let value = self.evaluate_json(link, event_tx, frame_slot, &target_rect_expression(&selector, false))?;
        let (x, y) = parse_target_rect(&value)?;
        self.interaction_started_at.get_or_insert_with(std::time::Instant::now);
        for click_count in 1..=count {
            self.send_semantic_input(link, event_tx, frame_slot, pointer_press(x, y, click_count))?;
            self.send_semantic_input(link, event_tx, frame_slot, pointer_release(x, y, click_count))?;
        }
        Ok(BrowserControlValue::Accepted)
    }

    fn semantic_fill(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        target: &crate::BrowserTarget,
        value: &str,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let selector = self.semantic.resolve(target)?;
        let result = self.evaluate_json(link, event_tx, frame_slot, &target_rect_expression(&selector, true))?;
        let _ = parse_target_rect(&result)?;
        self.interaction_started_at.get_or_insert_with(std::time::Instant::now);
        self.send_semantic_input(
            link,
            event_tx,
            frame_slot,
            BrowserInput::InsertText {
                text: value.to_string(),
            },
        )?;
        Ok(BrowserControlValue::Accepted)
    }

    fn semantic_scroll(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        target: Option<&crate::BrowserTarget>,
        delta_x: f64,
        delta_y: f64,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let selector = target.map(|target| self.semantic.resolve(target)).transpose()?;
        self.interaction_started_at.get_or_insert_with(std::time::Instant::now);
        let value = self.evaluate_json(
            link,
            event_tx,
            frame_slot,
            &scroll_expression(selector.as_deref(), delta_x, delta_y),
        )?;
        check_script_error(&value)?;
        Ok(BrowserControlValue::Json { value })
    }

    fn semantic_evaluate(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        expression: &str,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let value = self.evaluate_json(link, event_tx, frame_slot, expression)?;
        Ok(BrowserControlValue::Json { value })
    }

    fn evaluate_json(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        expression: &str,
    ) -> Result<Value, BrowserControlFailure> {
        self.evaluate_json_within(link, event_tx, frame_slot, expression, super::CALL_TIMEOUT)
    }

    fn evaluate_json_within(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        expression: &str,
        timeout: std::time::Duration,
    ) -> Result<Value, BrowserControlFailure> {
        let result = self
            .send_page_command_within(
                link,
                event_tx,
                frame_slot,
                "Runtime.evaluate",
                &json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                    "userGesture": true,
                }),
                timeout,
            )
            .map_err(|error| BrowserControlFailure::new("protocol_error", error.to_string()))?;
        if let Some(exception) = result.get("exceptionDetails") {
            let message = exception
                .pointer("/exception/description")
                .or_else(|| exception.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("JavaScript evaluation failed");
            return Err(BrowserControlFailure::new("javascript_error", message));
        }
        let remote = result
            .get("result")
            .ok_or_else(|| BrowserControlFailure::new("invalid_result", "CDP returned no evaluation result"))?;
        let value = remote.get("value").cloned().unwrap_or(Value::Null);
        bounded_control_value(value)
    }

    fn send_semantic_input(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        input: BrowserInput,
    ) -> Result<(), BrowserControlFailure> {
        let (method, params) = input.cdp();
        self.send_page_command(link, event_tx, frame_slot, method, &params)
            .map(|_| ())
            .map_err(|error| BrowserControlFailure::new("input_failed", error.to_string()))
    }
}

fn pointer_press(x: f64, y: f64, click_count: u32) -> BrowserInput {
    BrowserInput::MousePress {
        x,
        y,
        button: BrowserButton::Left,
        click_count,
        buttons: 1,
        modifiers: BrowserModifiers::none(),
    }
}

fn pointer_release(x: f64, y: f64, click_count: u32) -> BrowserInput {
    BrowserInput::MouseRelease {
        x,
        y,
        button: BrowserButton::Left,
        click_count,
        buttons: 0,
        modifiers: BrowserModifiers::none(),
    }
}
