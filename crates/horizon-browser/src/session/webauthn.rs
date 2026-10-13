//! Chromium `CDP` side of the `WebAuthn` bridge.
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

/// Queued page messages handled in one pass of the driver loop.
const MESSAGES_PER_TICK: usize = 4;

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
    /// Install the bridge on a newly bound page session. Returns `false`
    /// when a setup command failed and the attach must stop.
    pub(super) fn install_passkey_bridge(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        session: &str,
    ) -> bool {
        for (method, params) in self.passkey_setup_commands() {
            if !self.setup_command(link, event_tx, frame_slot, method, &params, Some(session)) {
                return false;
            }
            if method == "Runtime.enable" {
                self.runtime_enable_requested.insert(session.to_string());
            }
        }
        true
    }

    /// Page-session setup for the bridge. The binding survives a
    /// cross-process navigation only while the Runtime domain is enabled,
    /// so a configured bridge enables it at attach.
    fn passkey_setup_commands(&self) -> Vec<(&'static str, Value)> {
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
            // The session is gone, so the reply cannot reach the page.
            if !passkeys.bridge.cancel_for(|_| true).is_empty() {
                self.manifest_dirty = true;
            }
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
                    // An abort only marks state. A request needs protocol
                    // round trips, so it waits for the driver loop.
                    if matches!(message, PageMessage::Abort { .. }) {
                        self.handle_passkey_message(link, event_tx, frame_slot, context, message);
                    } else if let Some(reply) = self
                        .passkeys
                        .as_mut()
                        .and_then(|passkeys| passkeys.bridge.enqueue(context, message))
                    {
                        self.deliver_passkey_reply(link, context, &reply);
                    }
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
            // A navigation of the calling frame to another document ends its
            // request, even when it stops before a new document replaces the
            // old one. Same-document navigations (fragment, history API) do
            // not.
            "Page.frameStartedNavigating" => {
                let same_document = event
                    .params
                    .get("navigationType")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind.to_ascii_lowercase().contains("samedocument"));
                if !same_document && let Some(frame) = event.params.get("frameId").and_then(Value::as_str) {
                    self.cancel_passkeys_in_frame(link, event_tx, frame);
                }
                false
            }
            _ => false,
        }
    }

    pub(super) fn tick_passkeys(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) {
        for _ in 0..MESSAGES_PER_TICK {
            let Some((context, message)) = self
                .passkeys
                .as_mut()
                .and_then(|passkeys| passkeys.bridge.next_message())
            else {
                break;
            };
            self.handle_passkey_message(link, event_tx, frame_slot, context, message);
        }
        let Some((context, reply)) = self.passkeys.as_mut().and_then(|passkeys| passkeys.bridge.poll()) else {
            return;
        };
        self.deliver_passkey_reply(link, context, &reply);
        self.note_passkey_change(event_tx);
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
                    self.note_passkey_change(event_tx);
                }
            }
            PageMessage::Malformed { id } => {
                self.deliver_passkey_reply(link, context, &crate::webauthn::malformed_reply(id));
            }
            PageMessage::Request { id, op, options } => {
                let Some(admission) = self
                    .passkeys
                    .as_mut()
                    .map(|passkeys| passkeys.bridge.admit(context, id))
                else {
                    return;
                };
                let ticket = match admission {
                    Ok(ticket) => ticket,
                    Err(reply) => {
                        self.deliver_passkey_reply(link, context, &reply);
                        return;
                    }
                };
                // The frame-tree read drains events. An abort or the end of
                // the context in that time withdraws the admitted request.
                let origin = self.passkey_caller_origin(link, event_tx, frame_slot, context);
                let agent = self.owner_seen.clone();
                let Some(passkeys) = self.passkeys.as_mut() else {
                    return;
                };
                if let Some((context, reply)) = passkeys.bridge.start(ticket, op, &options, origin, agent.as_deref()) {
                    self.deliver_passkey_reply(link, context, &reply);
                }
                self.note_passkey_change(event_tx);
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

    /// The waiting state changed: repaint the chip and publish it to the
    /// coordination manifest.
    fn note_passkey_change(&mut self, event_tx: &BrowserEventSender) {
        self.manifest_dirty = true;
        event_tx.wake_ui();
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

    /// The document of the calling frame stays until a navigation commits,
    /// so it gets the reply; a navigation that stops still settles it.
    fn cancel_passkeys_in_frame(&mut self, link: &mut CdpLink, event_tx: &BrowserEventSender, frame: &str) {
        let Some(passkeys) = self.passkeys.as_mut() else {
            return;
        };
        let contexts = &passkeys.contexts;
        let replies = passkeys
            .bridge
            .cancel_for(|context| contexts.get(context).is_some_and(|owner| owner == frame));
        for (context, reply) in &replies {
            self.deliver_passkey_reply(link, *context, reply);
        }
        if !replies.is_empty() {
            self.note_passkey_change(event_tx);
        }
    }

    fn forget_passkey_contexts(&mut self, event_tx: &BrowserEventSender, matches: impl Fn(u64) -> bool) {
        let Some(passkeys) = self.passkeys.as_mut() else {
            return;
        };
        passkeys.contexts.retain(|id, _| !matches(*id));
        // The context is gone, so the reply cannot reach the page.
        if !passkeys.bridge.cancel_for(|context| matches(*context)).is_empty() {
            self.note_passkey_change(event_tx);
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
