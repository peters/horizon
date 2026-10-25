//! Key presses as one `WebDriver` key-action sequence per `press` action.

use std::time::Instant;

use serde_json::{Value, json};

use crate::session::BrowserEventSender;
use crate::webdriver::actions::ActionState;
use crate::{BrowserControlFailure, BrowserControlValue, BrowserKey, BrowserModifiers};

use super::super::Driver;

impl Driver {
    /// Send every press of the action in one key-action sequence: `BiDi`
    /// `input.performActions` on local Firefox, classic Perform Actions on
    /// Safari and remote targets. One request keeps a large count within the
    /// action bound, and Safari's text coalescing cannot drop the modifiers.
    /// A failed request releases the input state, so no key stays held.
    pub(in crate::webdriver::session) fn semantic_press(
        &mut self,
        key: BrowserKey,
        modifiers: BrowserModifiers,
        count: u32,
        event_tx: &BrowserEventSender,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let inputs = (0..count).flat_map(|_| crate::semantic_keys::press_inputs(key, modifiers));
        self.pending_classic_history_start = None;
        let result = if self.firefox_bidi() {
            let context = self.press_context(event_tx);
            let mut payload = self.actions.key_sequence_payload(inputs);
            payload["context"] = json!(context);
            let result = self.call_bidi("input.performActions", &payload, event_tx).map(|_| ());
            if result.is_err() {
                let context = json!({ "context": context });
                let _ = self.call_bidi("input.releaseActions", &context, event_tx);
                self.actions = ActionState::default();
            }
            result
        } else if self.safari.is_some() {
            self.flush_safari_input(event_tx)
                .and_then(|()| self.perform_safari_keys(inputs, event_tx))
        } else {
            let payload = self.actions.key_sequence_payload(inputs);
            let result = self.classic_post("actions", &payload).map(|_| ());
            if result.is_err() {
                let _ = self.classic_delete("actions");
                self.actions = ActionState::default();
            }
            result
        };
        if !self.retain_frame_during_navigation {
            self.scrollbar.refresh_at = Instant::now();
            self.frames.demand();
        }
        result
            .map(|()| BrowserControlValue::Accepted)
            .map_err(|error| BrowserControlFailure::new("input_failed", error))
    }

    /// The browsing context that owns the document focus. `BiDi` key actions
    /// name one context, and key events sent to the top document do not reach
    /// a focused child frame, so a press follows the focus a verified frame
    /// click left. A parent document names its child frame as the active
    /// element, so only a document that focuses a real element is selected.
    fn press_context(&mut self, events: &BrowserEventSender) -> String {
        let top = self.context_id.clone().unwrap_or_default();
        let mut candidates = vec![top.clone()];
        candidates.extend(self.semantic.bidi_scan_contexts().iter().cloned());
        candidates.truncate(crate::semantic::MAX_SEMANTIC_FRAMES + 1);
        for context in candidates {
            if self.context_focuses_element(&context, events) {
                return context;
            }
        }
        top
    }

    fn context_focuses_element(&mut self, context: &str, events: &BrowserEventSender) -> bool {
        let result = self.call_bidi(
            "script.evaluate",
            &json!({
                "expression": format!(
                    "(async () => JSON.stringify(await ({})))()",
                    crate::semantic::focused_document_expression()
                ),
                "target": {"context": context},
                "awaitPromise": true,
            }),
            events,
        );
        result.is_ok_and(|result| {
            super::frames::json_result(&result).is_ok_and(|value| {
                value.get("focused") == Some(&Value::Bool(true))
                    && value.get("activeIsFrame") != Some(&Value::Bool(true))
            })
        })
    }
}
