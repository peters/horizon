//! Chromium `CDP` side of the WebAuthn bridge.
//!
//! The page script posts through a `Runtime.addBinding` binding. The
//! binding call names the execution context; the host maps that context to
//! its frame and reads the frame's origin, and every ancestor's, from
//! `Page.getFrameTree`. Only the page session's main world is served, so an
//! out-of-process (cross-site) iframe never reaches the command.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::cdp::{CdpEvent, CdpLink};
use crate::frames::FrameSlot;
use crate::webauthn::{Bridge, CallerOrigin, PageError, PageMessage};

use super::{BrowserEventSender, DriverState};

#[derive(Debug)]
pub(super) struct CdpPasskeys {
    bridge: Bridge<u64>,
    /// Main-world execution contexts of the page session, by id: frame id.
    contexts: HashMap<u64, String>,
}

impl CdpPasskeys {
    pub(super) fn from_config(config: &crate::BrowserSessionConfig) -> Option<Self> {
        Bridge::from_config(config).map(|bridge| Self {
            bridge,
            contexts: HashMap::new(),
        })
    }
}

impl DriverState {
    /// Page-session setup for the bridge. The binding survives a
    /// cross-process navigation only while the Runtime domain is enabled,
    /// so a configured bridge enables it at attach.
    pub(super) fn passkey_setup_commands(&self) -> Vec<(&'static str, Value)> {
        let Some(passkeys) = self.passkeys.as_ref() else {
            return Vec::new();
        };
        vec![
            ("Runtime.addBinding", json!({ "name": passkeys.bridge.binding() })),
            (
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": passkeys.bridge.binding_source(), "runImmediately": true }),
            ),
            ("Runtime.enable", json!({})),
        ]
    }

    /// Forget contexts and cancel a running request of a replaced session.
    pub(super) fn reset_passkeys(&mut self) {
        if let Some(passkeys) = self.passkeys.as_mut() {
            passkeys.contexts.clear();
            passkeys.bridge.cancel_for(|_| true);
        }
    }

    /// Returns whether the event was a bridge binding call.
    pub(super) fn handle_passkey_event(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        event: &CdpEvent<'_>,
    ) -> bool {
        if self.passkeys.is_none() || event.session_id.is_none() || event.session_id != self.session_id.as_deref() {
            return false;
        }
        match event.method {
            "Runtime.bindingCalled" => {
                let Some(passkeys) = self.passkeys.as_ref() else {
                    return false;
                };
                if event.params.get("name").and_then(Value::as_str) != Some(passkeys.bridge.binding()) {
                    return false;
                }
                let context = event.params.get("executionContextId").and_then(Value::as_u64);
                let payload = event.params.get("payload").and_then(Value::as_str);
                if let (Some(context), Some(message)) = (context, payload.and_then(PageMessage::parse)) {
                    self.handle_passkey_message(link, event_tx, frame_slot, context, message);
                }
                true
            }
            "Runtime.executionContextCreated" => {
                self.note_passkey_context(event.params);
                false
            }
            "Runtime.executionContextDestroyed" => {
                if let Some(context) = event.params.get("executionContextId").and_then(Value::as_u64) {
                    self.forget_passkey_contexts(event_tx, |id| id == context);
                }
                false
            }
            "Runtime.executionContextsCleared" => {
                self.forget_passkey_contexts(event_tx, |_| true);
                false
            }
            _ => false,
        }
    }

    pub(super) fn tick_passkeys(&mut self, link: &mut CdpLink, event_tx: &BrowserEventSender) {
        let Some((context, reply)) = self.passkeys.as_mut().and_then(|passkeys| passkeys.bridge.poll()) else {
            return;
        };
        self.deliver_passkey_reply(link, context, &reply);
        event_tx.wake_ui();
    }

    fn handle_passkey_message(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        context: u64,
        message: PageMessage,
    ) {
        match message {
            PageMessage::Abort { id } => {
                if self
                    .passkeys
                    .as_mut()
                    .is_some_and(|passkeys| passkeys.bridge.abort(&context, id))
                {
                    event_tx.wake_ui();
                }
            }
            PageMessage::Request { id, op, options } => {
                let origin = self.passkey_caller_origin(link, event_tx, frame_slot, context);
                let agent = self.owner_seen.clone();
                let Some(passkeys) = self.passkeys.as_mut() else {
                    return;
                };
                if let Some(reply) = passkeys
                    .bridge
                    .start(context, id, op, &options, origin, agent.as_deref())
                {
                    self.deliver_passkey_reply(link, context, &reply);
                }
                event_tx.wake_ui();
            }
        }
    }

    /// The origin of `context` as the browser records it. The calling frame
    /// and each ancestor up to the main frame must share one origin.
    fn passkey_caller_origin(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        context: u64,
    ) -> Result<CallerOrigin, PageError> {
        let frame = self
            .passkeys
            .as_ref()
            .and_then(|passkeys| passkeys.contexts.get(&context).cloned())
            .ok_or_else(PageError::not_allowed)?;
        let session = self.session_id.clone().ok_or_else(PageError::not_allowed)?;
        let tree = self
            .call_and_ack(
                link,
                event_tx,
                frame_slot,
                "Page.getFrameTree",
                &json!({}),
                Some(&session),
            )
            .map_err(|error| {
                tracing::warn!(target: "browser", "passkey request could not read the frame tree: {error}");
                PageError::not_allowed()
            })?;
        let origin =
            same_origin_chain(&tree, &frame, self.main_frame_id.as_deref()).ok_or_else(PageError::not_allowed)?;
        CallerOrigin::parse(&origin)
    }

    fn deliver_passkey_reply(&self, link: &mut CdpLink, context: u64, reply: &str) {
        let (Some(passkeys), Some(session)) = (self.passkeys.as_ref(), self.session_id.as_deref()) else {
            return;
        };
        let params = json!({
            "functionDeclaration": passkeys.bridge.deliver_function(),
            "executionContextId": context,
            "arguments": [{ "value": reply }],
            "awaitPromise": false,
            "returnByValue": true,
        });
        if let Err(error) = link.send_request("Runtime.callFunctionOn", &params, Some(session)) {
            tracing::debug!(target: "browser", "passkey reply was not delivered: {error}");
        }
    }

    fn note_passkey_context(&mut self, params: &Value) {
        let Some(passkeys) = self.passkeys.as_mut() else {
            return;
        };
        let Some(context) = params.get("context") else {
            return;
        };
        let main_world = context.pointer("/auxData/isDefault").and_then(Value::as_bool) == Some(true);
        if let (true, Some(id), Some(frame)) = (
            main_world,
            context.get("id").and_then(Value::as_u64),
            context.pointer("/auxData/frameId").and_then(Value::as_str),
        ) {
            passkeys.contexts.insert(id, frame.to_string());
        }
    }

    fn forget_passkey_contexts(&mut self, event_tx: &BrowserEventSender, matches: impl Fn(u64) -> bool) {
        let Some(passkeys) = self.passkeys.as_mut() else {
            return;
        };
        passkeys.contexts.retain(|id, _| !matches(*id));
        if passkeys.bridge.cancel_for(|context| matches(*context)) {
            event_tx.wake_ui();
        }
    }
}

/// The calling frame's origin when it and every ancestor up to the main
/// frame report the same `securityOrigin`.
fn same_origin_chain(tree: &Value, frame: &str, main_frame: Option<&str>) -> Option<String> {
    let mut frames = HashMap::new();
    collect_frames(tree.get("frameTree")?, &mut frames, 0);
    let (_, origin) = frames.get(frame)?;
    let origin = (*origin).to_string();
    let mut current = frame;
    for _ in 0..=frames.len() {
        let (parent, frame_origin) = frames.get(current)?;
        if *frame_origin != origin {
            return None;
        }
        match parent {
            Some(parent) => current = parent,
            None => return (main_frame.is_none_or(|main| main == current)).then_some(origin),
        }
    }
    None
}

const MAX_FRAME_DEPTH: usize = 64;

fn collect_frames<'a>(node: &'a Value, frames: &mut HashMap<&'a str, (Option<&'a str>, &'a str)>, depth: usize) {
    if depth > MAX_FRAME_DEPTH {
        return;
    }
    let frame = node.get("frame");
    if let Some(id) = frame.and_then(|frame| frame.get("id")).and_then(Value::as_str) {
        let parent = frame.and_then(|frame| frame.get("parentId")).and_then(Value::as_str);
        let origin = frame
            .and_then(|frame| frame.get("securityOrigin"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        frames.insert(id, (parent, origin));
    }
    for child in node.get("childFrames").and_then(Value::as_array).into_iter().flatten() {
        collect_frames(child, frames, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Value {
        json!({"frameTree": {
            "frame": {"id": "main", "securityOrigin": "https://example.com"},
            "childFrames": [
                {"frame": {"id": "same", "parentId": "main", "securityOrigin": "https://example.com"},
                 "childFrames": [
                    {"frame": {"id": "nested", "parentId": "same", "securityOrigin": "https://example.com"}}
                 ]},
                {"frame": {"id": "sibling", "parentId": "main", "securityOrigin": "https://login.example.com"},
                 "childFrames": [
                    {"frame": {"id": "back", "parentId": "sibling", "securityOrigin": "https://example.com"}}
                 ]}
            ]
        }})
    }

    #[test]
    fn only_frames_whose_ancestors_share_their_origin_are_served() {
        let tree = tree();
        for served in ["main", "same", "nested"] {
            assert_eq!(
                same_origin_chain(&tree, served, Some("main")).as_deref(),
                Some("https://example.com"),
                "{served}"
            );
        }
        for refused in ["sibling", "back", "unknown"] {
            assert_eq!(same_origin_chain(&tree, refused, Some("main")), None, "{refused}");
        }
        assert_eq!(same_origin_chain(&tree, "main", Some("other")), None);
    }
}
