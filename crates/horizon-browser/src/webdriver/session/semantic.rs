//! Semantic DOM actions for Firefox and Safari `WebDriver` sessions.

use serde_json::{Value, json};

use crate::semantic::{
    bounded_control_value, check_script_error, parse_target_rect, scan_expression, scroll_expression,
    target_rect_expression,
};
use crate::session::BrowserEventSender;
use crate::{
    AgentAction, BackendKind, BrowserButton, BrowserControlAction, BrowserControlFailure, BrowserControlValue,
    BrowserInput, BrowserModifiers, BrowserSnapshot,
};

use super::{Driver, webdriver_value};

impl Driver {
    pub(super) fn execute_agent_action(
        &mut self,
        request: &AgentAction,
        event_tx: &BrowserEventSender,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        if let Some(command) = request.action.to_command() {
            let failure_code = if matches!(command, crate::session::BrowserCommand::Input(_)) {
                "input_failed"
            } else {
                "navigation_failed"
            };
            return self
                .run_command(command, event_tx, false)
                .map(|_| BrowserControlValue::Accepted)
                .map_err(|error| BrowserControlFailure::new(failure_code, error));
        }
        match &request.action {
            BrowserControlAction::Snapshot { max_nodes } => self.semantic_snapshot(*max_nodes),
            BrowserControlAction::Query { selector, max_results } => self.semantic_query(selector, *max_results),
            BrowserControlAction::Click { target, count } => self.semantic_click(target, *count, event_tx),
            BrowserControlAction::Fill { target, value } => self.semantic_fill(target, value, event_tx),
            BrowserControlAction::Scroll {
                target,
                delta_x,
                delta_y,
            } => self.semantic_scroll(target.as_ref(), *delta_x, *delta_y),
            BrowserControlAction::Evaluate { expression } => self.semantic_evaluate(expression),
            BrowserControlAction::Network { operation, options } => {
                self.network_action(request, *operation, options.clone(), event_tx)
            }
            BrowserControlAction::Navigate { .. }
            | BrowserControlAction::Reload
            | BrowserControlAction::Back
            | BrowserControlAction::Forward
            | BrowserControlAction::Input { .. } => Err(BrowserControlFailure::new(
                "invalid_action_state",
                "command action was not dispatched",
            )),
        }
    }

    fn semantic_snapshot(&mut self, max_nodes: u32) -> Result<BrowserControlValue, BrowserControlFailure> {
        let value = self.evaluate_json(&scan_expression(None, max_nodes))?;
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

    fn semantic_query(
        &mut self,
        selector: &str,
        max_results: u32,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let value = self.evaluate_json(&scan_expression(Some(selector), max_results))?;
        let (generation, revision, nodes) = self.semantic.register_nodes(value)?;
        Ok(BrowserControlValue::Nodes {
            generation,
            revision,
            nodes,
        })
    }

    fn semantic_click(
        &mut self,
        target: &crate::BrowserTarget,
        count: u32,
        event_tx: &BrowserEventSender,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let selector = self.semantic.resolve(target)?;
        let value = self.evaluate_json(&target_rect_expression(&selector, false))?;
        let (x, y) = parse_target_rect(&value)?;
        self.perform_click(x, y, count, event_tx)
            .map_err(|error| BrowserControlFailure::new("input_failed", error))?;
        Ok(BrowserControlValue::Accepted)
    }

    fn perform_click(&mut self, x: f64, y: f64, count: u32, event_tx: &BrowserEventSender) -> Result<(), String> {
        self.pending_classic_history_start = None;
        if self.safari.is_some() {
            return self.perform_safari_click(x, y, count, event_tx);
        }
        let mut payload = self
            .actions
            .click_payload(x, y, BrowserButton::Left, count, BrowserModifiers::none());
        let result = if self.config.browser.backend == BackendKind::FirefoxBidi {
            payload["context"] = json!(self.context_id);
            self.call_bidi("input.performActions", &payload, event_tx).map(|_| ())
        } else {
            self.classic_post("actions", &payload).map(|_| ())
        };
        if let Err(error) = &result {
            tracing::warn!("WebDriver input failed: {error}");
        }
        if !self.retain_frame_during_navigation {
            self.scroll_state_refresh_at = std::time::Instant::now();
            self.frames.demand();
        }
        result
    }

    fn semantic_fill(
        &mut self,
        target: &crate::BrowserTarget,
        value: &str,
        event_tx: &BrowserEventSender,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let selector = self.semantic.resolve(target)?;
        let result = self.evaluate_json(&target_rect_expression(&selector, true))?;
        let _ = parse_target_rect(&result)?;
        self.perform_input(
            BrowserInput::InsertText {
                text: value.to_string(),
            },
            event_tx,
        )
        .map_err(|error| BrowserControlFailure::new("input_failed", error))?;
        if self.safari.is_some() {
            self.flush_safari_input(event_tx)
                .map_err(|error| BrowserControlFailure::new("input_failed", error))?;
        }
        Ok(BrowserControlValue::Accepted)
    }

    fn semantic_scroll(
        &mut self,
        target: Option<&crate::BrowserTarget>,
        delta_x: f64,
        delta_y: f64,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let selector = target.map(|target| self.semantic.resolve(target)).transpose()?;
        let value = self.evaluate_json(&scroll_expression(selector.as_deref(), delta_x, delta_y))?;
        check_script_error(&value)?;
        self.frames.demand();
        Ok(BrowserControlValue::Json { value })
    }

    fn semantic_evaluate(&self, expression: &str) -> Result<BrowserControlValue, BrowserControlFailure> {
        let value = self.evaluate_json(expression)?;
        Ok(BrowserControlValue::Json { value })
    }

    fn evaluate_json(&self, expression: &str) -> Result<Value, BrowserControlFailure> {
        let response = self
            .classic_post(
                "execute/sync",
                &json!({ "script": format!("return ({expression});"), "args": [] }),
            )
            .map_err(|error| BrowserControlFailure::new("javascript_error", error))?;
        let value = webdriver_value(&response)
            .cloned()
            .ok_or_else(|| BrowserControlFailure::new("invalid_result", "WebDriver returned no script value"))?;
        bounded_control_value(value)
    }
}
