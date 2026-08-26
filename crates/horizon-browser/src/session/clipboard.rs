//! Frame-aware page-selection capture for the host clipboard bridge.

use std::collections::{HashMap, HashSet};

use crate::cdp::{CdpErrorInfo, CdpEvent, CdpLink};

use super::{BrowserEvent, BrowserEventSender, DriverState};

// Evaluate inside each frame's own default execution context. Reaching into
// `iframe.contentDocument` from the top document would fail for cross-origin
// frames, and returning early for an active iframe avoids copying a stale
// selection from an ancestor document.
const SELECTED_TEXT_EXPRESSION: &str = r#"(() => {
  if (!document.hasFocus()) return "";
  const active = document.activeElement;
  if (active?.tagName === "IFRAME") return "";
  if (active && active.type !== "password" && typeof active.value === "string"
      && Number.isInteger(active.selectionStart) && Number.isInteger(active.selectionEnd)) {
    return active.value.slice(active.selectionStart, active.selectionEnd);
  }
  return document.getSelection()?.toString() || "";
})()"#;

#[derive(Debug, Default)]
pub(super) struct ClipboardState {
    request_ids: HashSet<u64>,
    default_contexts: HashMap<String, HashSet<u64>>,
    iframe_sessions: HashSet<String>,
}

impl ClipboardState {
    fn evaluation_targets(&self, page_session: &str) -> Vec<(String, Option<u64>)> {
        let sessions = std::iter::once(page_session).chain(self.iframe_sessions.iter().map(String::as_str));
        let mut targets = Vec::new();
        for session in sessions {
            match self.default_contexts.get(session) {
                Some(contexts) if !contexts.is_empty() => {
                    targets.extend(contexts.iter().map(|context| (session.to_string(), Some(*context))));
                }
                _ => targets.push((session.to_string(), None)),
            }
        }
        targets
    }

    fn reset(&mut self) {
        self.request_ids.clear();
        self.default_contexts.clear();
        self.iframe_sessions.clear();
    }
}

impl DriverState {
    pub(super) fn request_clipboard_text(&mut self, link: &mut CdpLink) {
        let Some(page_session) = self.session_id.as_deref() else {
            return;
        };
        for (session, context_id) in self.clipboard.evaluation_targets(page_session) {
            let mut params = serde_json::json!({
                "expression": SELECTED_TEXT_EXPRESSION,
                "returnByValue": true,
            });
            if let Some(context_id) = context_id {
                params["contextId"] = serde_json::json!(context_id);
            }
            match link.send_request("Runtime.evaluate", &params, Some(session.as_str())) {
                Ok(request_id) => {
                    self.clipboard.request_ids.insert(request_id);
                }
                Err(error) => tracing::debug!(
                    target: "browser",
                    session,
                    "clipboard selection capture failed: {error}"
                ),
            }
        }
    }

    pub(super) fn handle_clipboard_response(
        &mut self,
        id: u64,
        result: Option<&serde_json::Value>,
        error: Option<&CdpErrorInfo>,
        event_tx: &BrowserEventSender,
    ) -> bool {
        if !self.clipboard.request_ids.remove(&id) {
            return false;
        }
        if let Some(error) = error {
            tracing::debug!(target: "browser", "clipboard selection capture rejected: {error}");
        } else if let Some(text) = clipboard_text_from_evaluation(result)
            && !text.is_empty()
        {
            let _ = event_tx.send(BrowserEvent::ClipboardText(text.to_string()));
        }
        true
    }

    pub(super) fn note_clipboard_execution_context(&mut self, event: &CdpEvent<'_>) {
        let Some(session) = event.session_id else {
            return;
        };
        let tracked_session =
            self.session_id.as_deref() == Some(session) || self.clipboard.iframe_sessions.contains(session);
        if !tracked_session {
            return;
        }
        match event.method {
            "Runtime.executionContextCreated" => {
                if let Some(context_id) = default_context_id(event.params) {
                    self.clipboard
                        .default_contexts
                        .entry(session.to_string())
                        .or_default()
                        .insert(context_id);
                }
            }
            "Runtime.executionContextDestroyed" => {
                if let Some(context_id) = event
                    .params
                    .get("executionContextId")
                    .and_then(serde_json::Value::as_u64)
                    && let Some(contexts) = self.clipboard.default_contexts.get_mut(session)
                {
                    contexts.remove(&context_id);
                }
            }
            "Runtime.executionContextsCleared" => {
                self.clipboard.default_contexts.remove(session);
            }
            _ => {}
        }
    }

    /// Record an auto-attached out-of-process iframe and enable its Runtime
    /// domain so subsequent selection capture can target its default contexts.
    /// Returns whether this was an iframe attachment consumed by this path.
    pub(super) fn note_clipboard_target_attachment(&mut self, link: &mut CdpLink, event: &CdpEvent<'_>) -> bool {
        if attached_target_type(event.params) != Some("iframe") {
            return false;
        }
        let Some(session) = target_event_session_id(event.params, event.session_id) else {
            return true;
        };
        if self.clipboard.iframe_sessions.insert(session.to_string()) {
            if let Err(error) = link.send_request(
                "Target.setAutoAttach",
                &serde_json::json!({
                    "autoAttach": true,
                    "waitForDebuggerOnStart": false,
                    "flatten": true,
                }),
                Some(session),
            ) {
                tracing::debug!(target: "browser", session, "failed to auto-attach nested iframe targets: {error}");
            }
            if let Err(error) = link.send_request("Runtime.enable", &serde_json::json!({}), Some(session)) {
                tracing::debug!(target: "browser", session, "failed to enable iframe runtime: {error}");
            }
        }
        true
    }

    pub(super) fn note_clipboard_target_detachment(&mut self, event: &CdpEvent<'_>) {
        let Some(session) = target_event_session_id(event.params, event.session_id) else {
            return;
        };
        self.clipboard.iframe_sessions.remove(session);
        self.clipboard.default_contexts.remove(session);
    }

    pub(super) fn reset_clipboard_tracking(&mut self) {
        self.clipboard.reset();
    }
}

fn default_context_id(params: &serde_json::Value) -> Option<u64> {
    let context = params.get("context")?;
    context
        .pointer("/auxData/isDefault")?
        .as_bool()?
        .then(|| context.get("id")?.as_u64())?
}

fn attached_target_type(params: &serde_json::Value) -> Option<&str> {
    params.pointer("/targetInfo/type")?.as_str()
}

pub(super) fn target_event_session_id<'a>(
    params: &'a serde_json::Value,
    event_session: Option<&'a str>,
) -> Option<&'a str> {
    params
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .or(event_session)
}

fn clipboard_text_from_evaluation(result: Option<&serde_json::Value>) -> Option<&str> {
    result?.pointer("/result/value")?.as_str()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{ClipboardState, clipboard_text_from_evaluation, default_context_id, target_event_session_id};

    #[test]
    fn frame_targets_cover_page_contexts_and_out_of_process_iframe_sessions() {
        let mut state = ClipboardState::default();
        state.default_contexts.insert("page".to_string(), HashSet::from([7, 8]));
        state.iframe_sessions.insert("oopif".to_string());
        state.default_contexts.insert("oopif".to_string(), HashSet::from([11]));

        let targets = state.evaluation_targets("page").into_iter().collect::<HashSet<_>>();

        assert_eq!(
            targets,
            HashSet::from([
                ("page".to_string(), Some(7)),
                ("page".to_string(), Some(8)),
                ("oopif".to_string(), Some(11)),
            ])
        );
    }

    #[test]
    fn an_iframe_session_without_reported_contexts_uses_its_default_world() {
        let mut state = ClipboardState::default();
        state.iframe_sessions.insert("oopif".to_string());

        let targets = state.evaluation_targets("page").into_iter().collect::<HashSet<_>>();

        assert_eq!(
            targets,
            HashSet::from([("page".to_string(), None), ("oopif".to_string(), None)])
        );
    }

    #[test]
    fn only_default_execution_contexts_are_clipboard_candidates() {
        let default = serde_json::json!({
            "context": { "id": 7, "auxData": { "isDefault": true, "frameId": "child" } }
        });
        let isolated = serde_json::json!({
            "context": { "id": 8, "auxData": { "isDefault": false, "frameId": "child" } }
        });

        assert_eq!(default_context_id(&default), Some(7));
        assert_eq!(default_context_id(&isolated), None);
        assert_eq!(default_context_id(&serde_json::json!({})), None);
    }

    #[test]
    fn target_events_prefer_the_attached_or_detached_session_parameter() {
        let params = serde_json::json!({ "sessionId": "child" });

        assert_eq!(target_event_session_id(&params, Some("parent")), Some("child"));
        assert_eq!(
            target_event_session_id(&serde_json::json!({}), Some("parent")),
            Some("parent")
        );
    }

    #[test]
    fn clipboard_text_reads_runtime_evaluation_values_only() {
        let value = serde_json::json!({ "result": { "type": "string", "value": "selected" } });
        let exception = serde_json::json!({ "exceptionDetails": { "text": "failed" } });

        assert_eq!(clipboard_text_from_evaluation(Some(&value)), Some("selected"));
        assert_eq!(clipboard_text_from_evaluation(Some(&exception)), None);
        assert_eq!(clipboard_text_from_evaluation(None), None);
    }
}
