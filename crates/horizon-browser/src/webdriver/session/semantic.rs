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

const DOCUMENT_IDENTITY_EXPRESSION: &str =
    "JSON.stringify([String(location.href), Number(globalThis.performance?.timeOrigin || 0)])";

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
            BrowserControlAction::WaitForSelector { .. } => Err(BrowserControlFailure::new(
                "invalid_action_state",
                "selector waits are observed from the driver loop",
            )),
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

    pub(super) fn semantic_query(
        &mut self,
        selector: &str,
        max_results: u32,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let value = self.evaluate_json(&scan_expression(Some(selector), max_results))?;
        self.register_query(value)
    }

    /// A selector scan that does not register references: for judging a wait
    /// condition without disturbing refs a concurrent snapshot handed out.
    pub(super) fn semantic_peek_within(
        &mut self,
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
        let value = self.evaluate_json_within(&scan_expression(Some(selector), max_results), Some(timeout))?;
        let peeked = self.semantic.peek_nodes(&value)?;
        self.record_classic_document_identity(peeked.document_identity);
        Ok((self.semantic.generation(), peeked.nodes, peeked.summary, value))
    }

    /// Register a previously peeked scan as the current references.
    pub(super) fn semantic_register_scan(
        &mut self,
        value: Value,
    ) -> Result<(u64, u64, Vec<crate::BrowserNode>), BrowserControlFailure> {
        self.semantic.register_nodes(value)
    }

    fn register_query(&mut self, value: Value) -> Result<BrowserControlValue, BrowserControlFailure> {
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
            self.scrollbar.refresh_at = std::time::Instant::now();
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
        self.evaluate_json_within(expression, None)
    }

    fn evaluate_json_within(
        &self,
        expression: &str,
        timeout: Option<std::time::Duration>,
    ) -> Result<Value, BrowserControlFailure> {
        let body = json!({ "script": format!("return ({expression});"), "args": [] });
        let response = match timeout {
            Some(timeout) => self.classic_navigation_post_within("execute/sync", &body, timeout),
            None => self.classic_post("execute/sync", &body),
        }
        .map_err(|error| BrowserControlFailure::new("javascript_error", error))?;
        let value = webdriver_value(&response)
            .cloned()
            .ok_or_else(|| BrowserControlFailure::new("invalid_result", "WebDriver returned no script value"))?;
        bounded_control_value(value)
    }

    pub(super) fn initialize_classic_document_identity(&mut self) {
        if self.config.browser.backend == BackendKind::SafariWebDriver {
            let _ = self.refresh_classic_document_identity_within(std::time::Duration::from_secs(1));
        }
    }

    pub(super) fn refresh_classic_document_identity_within(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<bool, BrowserControlFailure> {
        if self.config.browser.backend != BackendKind::SafariWebDriver {
            return Ok(false);
        }
        let value = self.evaluate_json_within(DOCUMENT_IDENTITY_EXPRESSION, Some(timeout))?;
        let identity = value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| BrowserControlFailure::new("invalid_result", "WebDriver returned no document identity"))?;
        Ok(self.record_classic_document_identity(Some(identity)))
    }

    fn record_classic_document_identity(&mut self, identity: Option<String>) -> bool {
        if self.config.browser.backend != BackendKind::SafariWebDriver {
            return false;
        }
        let Some(identity) = identity else {
            return false;
        };
        let changed = self
            .classic_document_identity
            .replace(identity.clone())
            .is_some_and(|previous| previous != identity);
        if changed {
            self.semantic.invalidate();
            self.advance_generation();
        }
        changed
    }
}
