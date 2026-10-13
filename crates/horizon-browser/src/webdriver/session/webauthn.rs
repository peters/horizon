//! Firefox `WebDriver` `BiDi` side of the `WebAuthn` bridge.
//!
//! A preload script posts through a `script.channel`. Each `script.message`
//! names its source realm and browsing context; the host reads the realm's
//! origin, and the origin of every ancestor context, from `script.getRealms`
//! and `browsingContext.getTree`.

use std::collections::VecDeque;

use serde_json::{Value, json};

use crate::session::BrowserEventSender;
use crate::webauthn::{Bridge, CallerOrigin, PageError, PageMessage};

use super::Driver;

/// The realm that sent a request and its browsing context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Source {
    realm: String,
    context: String,
}

#[derive(Debug)]
pub(super) struct BidiPasskeys {
    bridge: Bridge<Source>,
    channel: String,
    /// Replies decided during event handling. A reply is a blocking
    /// `script.callFunction`, so the driver loop sends them.
    outbox: VecDeque<(Source, String)>,
}

/// Queued page messages handled in one pass of the driver loop.
const MESSAGES_PER_TICK: usize = 4;
const MAX_QUEUED_REPLIES: usize = 32;

impl BidiPasskeys {
    fn queue_reply(&mut self, reply: (Source, String)) {
        if self.outbox.len() < MAX_QUEUED_REPLIES {
            self.outbox.push_back(reply);
        }
    }

    pub(super) fn from_config(config: &crate::BrowserSessionConfig) -> Option<Self> {
        Bridge::from_config(config).map(|bridge| Self {
            bridge,
            channel: format!("horizon-passkey-{}", uuid::Uuid::new_v4().simple()),
            outbox: VecDeque::new(),
        })
    }
}

const MAX_CONTEXT_DEPTH: usize = 64;

impl Driver {
    /// Install the page script in this panel's top-level context and its
    /// frames before the first navigation. A failure leaves the bridge off.
    pub(super) fn install_passkey_bridge(&mut self, events: &BrowserEventSender) {
        if !self.firefox_bidi() || self.host.is_remote() {
            self.passkeys = None;
        }
        let (Some(passkeys), Some(context)) = (self.passkeys.as_ref(), self.context_id.clone()) else {
            return;
        };
        let channel = json!({ "type": "channel", "value": { "channel": passkeys.channel, "ownership": "none" } });
        let preload = json!({
            "functionDeclaration": passkeys.bridge.page_function(),
            "arguments": [channel],
            "contexts": [context],
        });
        let subscribe = json!({ "events": ["script.message"], "contexts": [context] });
        let installed = self
            .call_bidi("session.subscribe", &subscribe, events)
            .and_then(|_| self.call_bidi("script.addPreloadScript", &preload, events));
        if let Err(error) = installed {
            tracing::warn!(target: "browser", "Firefox passkey bridge is unavailable: {error}");
            let _ = events.send(crate::session::BrowserEvent::Warning(
                "The passkey bridge could not be installed in this Firefox panel".to_string(),
            ));
            self.passkeys = None;
        }
    }

    /// Returns whether the event was a bridge message.
    pub(super) fn handle_passkey_bidi_event(&mut self, event: &Value, events: &BrowserEventSender) -> bool {
        let Some(passkeys) = self.passkeys.as_mut() else {
            return false;
        };
        let method = event.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = event.get("params").unwrap_or(&Value::Null);
        if matches!(
            method,
            "browsingContext.navigationStarted" | "browsingContext.contextDestroyed"
        ) {
            // The old realm gets the reply until a navigation commits; a
            // navigation that stops before it commits still settles it.
            if let Some(context) = params.get("context").and_then(Value::as_str) {
                let replies = passkeys.bridge.cancel_for(|source| source.context == context);
                let ended = !replies.is_empty();
                for reply in replies {
                    passkeys.queue_reply(reply);
                }
                if ended {
                    self.note_passkey_change(events);
                }
            }
            return false;
        }
        if method != "script.message" || params.get("channel").and_then(Value::as_str) != Some(&passkeys.channel) {
            return false;
        }
        let source = Source {
            realm: string_at(params, "/source/realm"),
            context: string_at(params, "/source/context"),
        };
        let message = params
            .pointer("/data/value")
            .and_then(Value::as_str)
            .and_then(PageMessage::parse);
        if let Some(message) = message
            && !source.realm.is_empty()
            && !source.context.is_empty()
        {
            // An abort only marks state. A request needs `BiDi` round trips,
            // whose replies carry more events, so it waits for the loop.
            if matches!(message, PageMessage::Abort { .. }) {
                self.handle_passkey_message(&source, message, events);
            } else if let Some(passkeys) = self.passkeys.as_mut()
                && let Some(reply) = passkeys.bridge.enqueue(source.clone(), message)
            {
                passkeys.queue_reply((source, reply));
            }
        }
        true
    }

    pub(super) fn tick_passkeys(&mut self, events: &BrowserEventSender) {
        while let Some((source, reply)) = self.passkeys.as_mut().and_then(|passkeys| passkeys.outbox.pop_front()) {
            self.deliver_passkey_reply(&source, &reply, events);
        }
        for _ in 0..MESSAGES_PER_TICK {
            let Some((source, message)) = self
                .passkeys
                .as_mut()
                .and_then(|passkeys| passkeys.bridge.next_message())
            else {
                break;
            };
            self.handle_passkey_message(&source, message, events);
        }
        let Some((source, reply)) = self.passkeys.as_mut().and_then(|passkeys| passkeys.bridge.poll()) else {
            return;
        };
        self.deliver_passkey_reply(&source, &reply, events);
        self.note_passkey_change(events);
    }

    fn handle_passkey_message(&mut self, source: &Source, message: PageMessage, events: &BrowserEventSender) {
        match message {
            PageMessage::Abort { id } => {
                if self
                    .passkeys
                    .as_mut()
                    .is_some_and(|passkeys| passkeys.bridge.abort(source, id))
                {
                    self.note_passkey_change(events);
                }
            }
            PageMessage::Malformed { id } => {
                self.deliver_passkey_reply(source, &crate::webauthn::malformed_reply(id), events);
            }
            PageMessage::Request { id, op, options } => {
                let Some(passkeys) = self.passkeys.as_mut() else {
                    return;
                };
                let ticket = match passkeys.bridge.admit(source.clone(), id) {
                    Ok(ticket) => ticket,
                    Err(reply) => {
                        self.deliver_passkey_reply(source, &reply, events);
                        return;
                    }
                };
                // The realm reads drain events. An abort or a navigation of
                // the caller in that time withdraws the admitted request.
                let origin = self.passkey_caller_origin(source, events);
                let agent = self.owner_seen.clone();
                let Some(passkeys) = self.passkeys.as_mut() else {
                    return;
                };
                if let Some((source, reply)) = passkeys.bridge.start(ticket, op, &options, origin, agent.as_deref()) {
                    self.deliver_passkey_reply(&source, &reply, events);
                }
                self.note_passkey_change(events);
            }
        }
    }

    /// The waiting state changed: repaint the chip and publish it to the
    /// coordination manifest.
    fn note_passkey_change(&mut self, events: &BrowserEventSender) {
        self.coordination_dirty = true;
        events.wake_ui();
    }

    /// The origin of the source realm, when the source context and every
    /// ancestor up to this panel's top-level context share that origin.
    fn passkey_caller_origin(
        &mut self,
        source: &Source,
        events: &BrowserEventSender,
    ) -> Result<CallerOrigin, PageError> {
        let top = self.context_id.clone().ok_or_else(PageError::not_allowed)?;
        let origin = self.window_realm_origin(&source.context, Some(&source.realm), events)?;
        let tree = self
            .call_bidi("browsingContext.getTree", &json!({ "root": top }), events)
            .map_err(|error| refused(&error))?;
        let ancestors = ancestors(&tree, &source.context).ok_or_else(PageError::not_allowed)?;
        for ancestor in ancestors {
            if self.window_realm_origin(&ancestor, None, events)? != origin {
                return Err(PageError::not_allowed());
            }
        }
        CallerOrigin::parse(&origin)
    }

    /// The origin of the page realm of `context`: the named realm, or the
    /// only window realm outside a sandbox.
    fn window_realm_origin(
        &mut self,
        context: &str,
        realm: Option<&str>,
        events: &BrowserEventSender,
    ) -> Result<String, PageError> {
        let realms = self
            .call_bidi(
                "script.getRealms",
                &json!({ "context": context, "type": "window" }),
                events,
            )
            .map_err(|error| refused(&error))?;
        let mut page_realms = realms
            .get("realms")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|candidate| candidate.get("sandbox").is_none_or(Value::is_null))
            .filter(|candidate| {
                realm.is_none_or(|realm| candidate.get("realm").and_then(Value::as_str) == Some(realm))
            });
        let found = page_realms.next().ok_or_else(PageError::not_allowed)?;
        if page_realms.next().is_some() {
            return Err(PageError::not_allowed());
        }
        found
            .get("origin")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(PageError::not_allowed)
    }

    fn deliver_passkey_reply(&mut self, source: &Source, reply: &str, events: &BrowserEventSender) {
        let Some(passkeys) = self.passkeys.as_ref() else {
            return;
        };
        let params = json!({
            "functionDeclaration": passkeys.bridge.deliver_function(),
            "target": { "realm": source.realm },
            "arguments": [{ "type": "string", "value": reply }],
            "awaitPromise": false,
            "resultOwnership": "none",
        });
        if let Err(error) = self.call_bidi("script.callFunction", &params, events) {
            tracing::debug!(target: "browser", "passkey reply was not delivered: {error}");
        }
    }
}

/// The contexts from `context`'s parent up to the tree root, or `None`
/// when `context` is not in the tree.
fn ancestors(tree: &Value, context: &str) -> Option<Vec<String>> {
    fn search(node: &Value, context: &str, path: &mut Vec<String>, depth: usize) -> bool {
        let Some(id) = node.get("context").and_then(Value::as_str) else {
            return false;
        };
        if id == context {
            return true;
        }
        if depth >= MAX_CONTEXT_DEPTH {
            return false;
        }
        path.push(id.to_string());
        let found = node
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|child| search(child, context, path, depth + 1));
        if !found {
            path.pop();
        }
        found
    }
    let root = tree.get("contexts").and_then(Value::as_array)?.first()?;
    let mut path = Vec::new();
    search(root, context, &mut path, 0).then(|| {
        path.reverse();
        path
    })
}

fn string_at(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn refused(error: &str) -> PageError {
    tracing::warn!(target: "browser", "passkey request could not read the caller's realm: {error}");
    PageError::not_allowed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestors_walk_from_the_parent_to_the_top_level_context() {
        let tree = json!({"contexts": [{
            "context": "top",
            "children": [
                {"context": "a", "children": [{"context": "a1", "children": []}]},
                {"context": "b", "children": []}
            ]
        }]});
        assert_eq!(ancestors(&tree, "top"), Some(Vec::new()));
        assert_eq!(ancestors(&tree, "b"), Some(vec!["top".to_string()]));
        assert_eq!(ancestors(&tree, "a1"), Some(vec!["a".to_string(), "top".to_string()]));
        assert_eq!(ancestors(&tree, "elsewhere"), None);
    }
}
