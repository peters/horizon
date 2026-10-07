//! Child-frame scans and native input through the Firefox `BiDi` companion.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::semantic::{
    FrameTarget, MAX_SEMANTIC_FRAMES, append_frame_scan, bounded_control_value, frame_fill_expression,
    parse_target_rect, scan_expression, target_rect_expression,
};
use crate::session::BrowserEventSender;
use crate::{BrowserButton, BrowserControlFailure, BrowserControlValue, BrowserInput, BrowserModifiers};

use super::super::Driver;

impl Driver {
    pub(in crate::webdriver::session) fn frame_scan(
        &mut self,
        selector: Option<&str>,
        max_nodes: u32,
        events: &BrowserEventSender,
    ) -> Result<Value, BrowserControlFailure> {
        let deadline = Instant::now() + super::super::COMMAND_TIMEOUT;
        let expression = scan_expression(selector, max_nodes);
        let mut scan = self.guarded_semantic_scan(&expression, Some(remaining(deadline)?))?;
        let generation = self.semantic.generation();
        crate::semantic::clear_scan_frames(&mut scan)?;
        if !self.firefox_bidi() {
            return Ok(scan);
        }
        // The tree is rooted at this panel, never another page of a shared session.
        let root = self.context_id.clone().ok_or_else(stale)?;
        let tree = self.frame_command("browsingContext.getTree", &json!({"root":root}), events, deadline)?;
        let contexts = child_contexts(&tree, &root)?;
        for context in contexts {
            if scan["nodes"]
                .as_array()
                .is_some_and(|nodes| nodes.len() >= max_nodes as usize)
            {
                break;
            }
            let result = self.frame_command(
                "script.evaluate",
                &json!({
                    "expression":format!("JSON.stringify(({expression}))"), "target":{"context":context},
                    "awaitPromise":true,
                }),
                events,
                deadline,
            )?;
            let realm = result
                .get("realm")
                .and_then(Value::as_str)
                .ok_or_else(stale)?
                .to_owned();
            let child = json_result(&result)?;
            append_frame_scan(&mut scan, child, &FrameTarget::Bidi { context, realm }, max_nodes)?;
        }
        if self.semantic.generation() != generation {
            return Err(stale());
        }
        Ok(scan)
    }

    fn frame_command(
        &mut self,
        method: &str,
        params: &Value,
        events: &BrowserEventSender,
        deadline: Instant,
    ) -> Result<Value, BrowserControlFailure> {
        let link = self.bidi.as_mut().ok_or_else(stale)?;
        let outcome = link.call(remaining(deadline)?, method, params);
        self.host.record_bidi_result(method, params, &outcome.result);
        for event in outcome.events {
            self.handle_bidi_event(&event, events);
        }
        remaining(deadline)?;
        outcome.result.map_err(|_| stale())
    }

    fn frame_value(
        &mut self,
        frame: &FrameTarget,
        expression: &str,
        events: &BrowserEventSender,
    ) -> Result<Value, BrowserControlFailure> {
        let FrameTarget::Bidi { realm, .. } = frame else {
            return Err(stale());
        };
        let result = self
            .call_bidi(
                "script.evaluate",
                &json!({
                    "expression":format!("JSON.stringify(({expression}))"), "target":{"realm":realm},
                    "awaitPromise":true,
                }),
                events,
            )
            .map_err(|_| stale())?;
        if !self.semantic.bidi_frame_is_current(frame) {
            return Err(stale());
        }
        json_result(&result)
    }

    pub(in crate::webdriver::session) fn frame_fill(
        &mut self,
        frame: &FrameTarget,
        selector: &str,
        value: &str,
        events: &BrowserEventSender,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let result = self.frame_value(frame, &frame_fill_expression(selector), events)?;
        parse_target_rect(&result)?;
        let FrameTarget::Bidi { context, .. } = frame else {
            return Err(stale());
        };
        let mut payload = self
            .actions
            .payload(BrowserInput::InsertText { text: value.to_owned() });
        payload["context"] = json!(context);
        self.call_bidi("input.performActions", &payload, events)
            .map_err(|_| BrowserControlFailure::new("input_failed", "child-frame text input failed"))?;
        self.frames.demand();
        Ok(BrowserControlValue::Accepted)
    }

    pub(in crate::webdriver::session) fn frame_click(
        &mut self,
        frame: &FrameTarget,
        selector: &str,
        count: u32,
        events: &BrowserEventSender,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let result = self.frame_value(frame, &target_rect_expression(selector, false), events)?;
        let (x, y) = parse_target_rect(&result)?;
        let FrameTarget::Bidi { context, realm } = frame else {
            return Err(stale());
        };
        let element = self.call_bidi("script.evaluate", &json!({
            "expression":format!("document.querySelector({})", serde_json::to_string(selector).map_err(|_| stale())?),
            "target":{"realm":realm}, "awaitPromise":false,
        }), events).map_err(|_| stale())?;
        if !self.semantic.bidi_frame_is_current(frame) {
            return Err(stale());
        }
        let shared = element
            .pointer("/result/sharedId")
            .and_then(Value::as_str)
            .ok_or_else(stale)?;
        let mut payload = self
            .actions
            .click_payload(x, y, BrowserButton::Left, count, BrowserModifiers::none());
        payload["context"] = json!(context);
        // Element origin lets the browser map nested and transformed frame coordinates.
        let pointer = payload["actions"]
            .as_array_mut()
            .and_then(|sources| sources.iter_mut().find(|source| source["type"] == "pointer"))
            .ok_or_else(stale)?;
        pointer["actions"][0]["origin"] = json!({"type":"element", "element":{"sharedId":shared}});
        pointer["actions"][0]["x"] = json!(0);
        pointer["actions"][0]["y"] = json!(0);
        self.call_bidi("input.performActions", &payload, events)
            .map_err(|_| BrowserControlFailure::new("input_failed", "child-frame pointer input failed"))?;
        self.frames.demand();
        Ok(BrowserControlValue::Accepted)
    }
}

fn json_result(result: &Value) -> Result<Value, BrowserControlFailure> {
    if result.get("type").and_then(Value::as_str) != Some("success") {
        return Err(stale());
    }
    let text = result
        .pointer("/result/value")
        .and_then(Value::as_str)
        .ok_or_else(stale)?;
    let value = serde_json::from_str(text)
        .map_err(|_| BrowserControlFailure::new("invalid_result", "frame returned invalid JSON"))?;
    bounded_control_value(value)
}

fn child_contexts(tree: &Value, root: &str) -> Result<Vec<String>, BrowserControlFailure> {
    let roots = tree.get("contexts").and_then(Value::as_array).ok_or_else(stale)?;
    let root = roots
        .iter()
        .find(|entry| entry["context"].as_str() == Some(root))
        .ok_or_else(stale)?;
    let mut pending = vec![root];
    let mut output = Vec::new();
    while let Some(entry) = pending.pop() {
        if let Some(children) = entry.get("children").and_then(Value::as_array) {
            for child in children.iter().rev() {
                output.push(
                    child
                        .get("context")
                        .and_then(Value::as_str)
                        .ok_or_else(stale)?
                        .to_owned(),
                );
                if output.len() >= MAX_SEMANTIC_FRAMES {
                    return Err(BrowserControlFailure::new(
                        "frame_limit",
                        "the page exceeds the semantic frame limit",
                    ));
                }
                pending.push(child);
            }
        }
    }
    Ok(output)
}
fn stale() -> BrowserControlFailure {
    BrowserControlFailure::new("stale_reference", "the child document changed; take a fresh snapshot")
}

fn remaining(deadline: Instant) -> Result<Duration, BrowserControlFailure> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|value| !value.is_zero())
        .ok_or_else(|| BrowserControlFailure::new("frame_timeout", "frame observation exceeded its original bound"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_is_scoped_to_the_bound_page_and_includes_nested_children() {
        let tree = json!({"contexts":[{"context":"other","children":[{"context":"foreign"}]},{"context":"page","children":[{"context":"child","children":[{"context":"nested","children":null}]}]}]});
        assert_eq!(child_contexts(&tree, "page").unwrap(), vec!["child", "nested"]);
        assert!(child_contexts(&tree, "missing").is_err());
    }

    #[test]
    fn excessive_frame_trees_and_script_failures_are_rejected() {
        let children: Vec<_> = (0..MAX_SEMANTIC_FRAMES)
            .map(|n| json!({"context":n.to_string(),"children":null}))
            .collect();
        let tree = json!({"contexts":[{"context":"page","children":children}]});
        assert_eq!(child_contexts(&tree, "page").unwrap_err().code, "frame_limit");
        assert_eq!(
            json_result(&json!({"type":"exception","exceptionDetails":{"text":"sensitive-page-error"}}))
                .unwrap_err()
                .code,
            "stale_reference"
        );
        assert!(remaining(Instant::now().checked_sub(Duration::from_secs(1)).unwrap()).is_err());
    }
}
