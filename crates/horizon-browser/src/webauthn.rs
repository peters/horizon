//! Optional bridge that forwards a page's `WebAuthn` request to an external
//! authenticator command.
//!
//! A page script replaces `navigator.credentials.get/create` for `publicKey`
//! requests and posts them to the driver over an engine channel (a CDP
//! binding or a `BiDi` script channel). The driver resolves the caller's
//! origin from the browser, builds `clientDataJSON` itself, runs the
//! command and delivers the reply into the calling realm. See
//! `docs/architecture/browser-webauthn-bridge.md` for the trust boundary.

mod command;
mod origin;
mod request;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

pub(crate) use origin::CallerOrigin;
use request::{PageOptions, Prepared};

use crate::frames::FrameSlot;

/// Page messages larger than this are dropped before parsing.
pub(crate) const MAX_PAGE_MESSAGE_BYTES: usize = 192 * 1024;
const PAGE_FUNCTION: &str = include_str!("webauthn/page.js");
/// A page that starts and aborts requests in a loop must not flood the
/// person's authenticator with prompts.
const MAX_STARTS_PER_WINDOW: usize = 6;
const START_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    Get,
    Create,
}

impl Operation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Create => "create",
        }
    }
}

/// The `DOMException` (or `TypeError`) a page receives. Messages are fixed
/// so a page cannot tell a refusal, a missing credential and a denial apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PageError {
    pub(crate) name: &'static str,
    pub(crate) message: &'static str,
}

impl PageError {
    pub(crate) fn not_allowed() -> Self {
        Self {
            name: "NotAllowedError",
            message: "The operation either timed out or was not allowed.",
        }
    }

    pub(crate) fn insecure_origin() -> Self {
        Self {
            name: "SecurityError",
            message: "This origin cannot use the passkey bridge.",
        }
    }

    pub(crate) fn foreign_rp_id() -> Self {
        Self {
            name: "SecurityError",
            message: "The relying party ID is not a registrable suffix of this origin.",
        }
    }

    pub(crate) fn type_error() -> Self {
        Self {
            name: "TypeError",
            message: "The publicKey options are not valid.",
        }
    }

    fn excluded() -> Self {
        Self {
            name: "InvalidStateError",
            message: "The authenticator already holds a credential for this account.",
        }
    }

    fn busy() -> Self {
        Self {
            name: "NotAllowedError",
            message: "Another passkey request is waiting.",
        }
    }

    fn too_many() -> Self {
        Self {
            name: "NotAllowedError",
            message: "Too many passkey requests. Wait a minute and try again.",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PageMessage {
    Request {
        id: u64,
        op: Operation,
        #[serde(default)]
        options: Box<PageOptions>,
    },
    Abort {
        id: u64,
    },
    /// A request whose options do not parse. It gets a `TypeError` reply,
    /// so the page's promise settles.
    #[serde(skip)]
    Malformed {
        id: u64,
    },
}

impl PageMessage {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        #[derive(Deserialize)]
        struct Header {
            kind: String,
            id: u64,
        }
        if raw.len() > MAX_PAGE_MESSAGE_BYTES {
            return None;
        }
        serde_json::from_str(raw).ok().or_else(|| {
            serde_json::from_str::<Header>(raw)
                .ok()
                .filter(|header| header.kind == "request")
                .map(|header| Self::Malformed { id: header.id })
        })
    }
}

/// The reply for a request that the host could not read.
pub(crate) fn malformed_reply(page_id: u64) -> String {
    error_reply(page_id, PageError::type_error())
}

#[derive(Debug)]
struct Active<T> {
    target: T,
    page_id: u64,
    prepared: Prepared,
    run: command::CommandRun,
}

/// Per-session bridge state. `T` names the realm that receives the reply.
#[derive(Debug)]
pub(crate) struct Bridge<T> {
    argv: Vec<String>,
    deliver_key: String,
    binding: String,
    frame_slot: Arc<FrameSlot>,
    active: Option<Active<T>>,
    recent_starts: VecDeque<Instant>,
}

impl<T: Clone + PartialEq> Bridge<T> {
    /// `None` unless the browser config names an authenticator command.
    pub(crate) fn from_config(config: &crate::BrowserSessionConfig) -> Option<Self> {
        let argv = config.browser.webauthn_command.clone();
        if argv.first().is_none_or(String::is_empty) {
            return None;
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        Some(Self {
            argv,
            deliver_key: format!("__horizon_passkey_{}", &token[..16]),
            binding: format!("__horizon_passkey_post_{}", &token[16..]),
            frame_slot: Arc::clone(&config.frame_slot),
            active: None,
            recent_starts: VecDeque::new(),
        })
    }

    /// The CDP binding name the page script posts through.
    pub(crate) fn binding(&self) -> &str {
        &self.binding
    }

    /// `(send) => ...`: installs the bridge with `send` as the channel.
    pub(crate) fn page_function(&self) -> String {
        let key = serde_json::Value::String(self.deliver_key.clone()).to_string();
        PAGE_FUNCTION
            .replace("__DELIVER_KEY__", &key)
            .replace("__MAX_MESSAGE_BYTES__", &MAX_PAGE_MESSAGE_BYTES.to_string())
    }

    /// A CDP new-document source that takes the binding off the global
    /// object before page script runs and installs the bridge with it.
    pub(crate) fn binding_source(&self) -> String {
        let binding = serde_json::Value::String(self.binding.clone()).to_string();
        format!(
            "({function})((() => {{ const post = globalThis[{binding}]; try {{ delete globalThis[{binding}]; }} catch (_) {{}} return typeof post === \"function\" ? (message) => post(message) : undefined; }})());",
            function = self.page_function().trim_end(),
        )
    }

    /// `(reply) => ...`: hands one reply to the page script in its realm.
    pub(crate) fn deliver_function(&self) -> String {
        let key = serde_json::Value::String(self.deliver_key.clone()).to_string();
        format!(
            "(reply) => {{ const deliver = globalThis[{key}]; if (typeof deliver === \"function\") deliver(reply); }}"
        )
    }

    /// Start a request whose caller origin the engine resolved. Returns a
    /// reply for the page when the request ends at once.
    pub(crate) fn start(
        &mut self,
        target: T,
        page_id: u64,
        operation: Operation,
        options: &PageOptions,
        origin: Result<CallerOrigin, PageError>,
        agent: Option<&str>,
    ) -> Option<String> {
        if self.active.is_some() {
            return Some(error_reply(page_id, PageError::busy()));
        }
        let prepared = match origin.and_then(|origin| request::prepare(operation, options, &origin, agent)) {
            Ok(prepared) => prepared,
            Err(error) => return Some(error_reply(page_id, error)),
        };
        let now = Instant::now();
        while self
            .recent_starts
            .front()
            .is_some_and(|started| now.duration_since(*started) >= START_WINDOW)
        {
            self.recent_starts.pop_front();
        }
        if self.recent_starts.len() >= MAX_STARTS_PER_WINDOW {
            return Some(error_reply(page_id, PageError::too_many()));
        }
        self.recent_starts.push_back(now);
        let run = command::CommandRun::start(&self.argv, prepared.input.clone(), prepared.timeout);
        self.active = Some(Active {
            target,
            page_id,
            prepared,
            run,
        });
        self.frame_slot.set_passkey_pending(true);
        None
    }

    /// The page aborted request `page_id`: kill its command. No reply is
    /// sent; the page already rejected the promise.
    pub(crate) fn abort(&mut self, target: &T, page_id: u64) -> bool {
        self.cancel_where(|active| active.target == *target && active.page_id == page_id)
    }

    /// The realm that sent the active request is gone.
    pub(crate) fn cancel_for(&mut self, matches: impl Fn(&T) -> bool) -> bool {
        self.cancel_where(|active| matches(&active.target))
    }

    fn cancel_where(&mut self, matches: impl Fn(&Active<T>) -> bool) -> bool {
        if !self.active.as_ref().is_some_and(matches) {
            return false;
        }
        self.active = None;
        self.frame_slot.set_passkey_pending(false);
        true
    }

    /// A finished request and the reply for its realm.
    pub(crate) fn poll(&mut self) -> Option<(T, String)> {
        let outcome = self.active.as_ref()?.run.poll()?;
        let active = self.active.take()?;
        self.frame_slot.set_passkey_pending(false);
        let reply = match outcome {
            command::CommandOutcome::Exited(stdout) => {
                match request::credential_from_output(&active.prepared, &stdout) {
                    Ok(credential) => json!({ "id": active.page_id, "credential": credential }).to_string(),
                    Err(error) => error_reply(active.page_id, error),
                }
            }
            command::CommandOutcome::TimedOut => {
                tracing::info!(target: "browser", "passkey authenticator command timed out");
                error_reply(active.page_id, PageError::not_allowed())
            }
            command::CommandOutcome::Failed(reason) => {
                tracing::warn!(target: "browser", "passkey authenticator command failed: {reason}");
                error_reply(active.page_id, PageError::not_allowed())
            }
        };
        Some((active.target, reply))
    }
}

impl<T> Drop for Bridge<T> {
    fn drop(&mut self) {
        if self.active.take().is_some() {
            self.frame_slot.set_passkey_pending(false);
        }
    }
}

fn error_reply(page_id: u64, error: PageError) -> String {
    json!({ "id": page_id, "error": { "name": error.name, "message": error.message } }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge(argv: Vec<String>) -> Option<Bridge<u64>> {
        let mut config = crate::BrowserSessionConfig {
            browser: crate::BrowserConfig::default(),
            panel_local_id: "panel".into(),
            initial_url: None,
            width: 800,
            height: 600,
            frame_slot: Arc::new(FrameSlot::new()),
            coordination: None,
            capture_directory: None,
            video: Arc::new(crate::VideoCaptureHandle::default()),
            remote: None,
        };
        config.browser.webauthn_command = argv;
        Bridge::from_config(&config)
    }

    fn get_request() -> PageMessage {
        PageMessage::parse(r#"{"kind":"request","id":7,"op":"get","options":{"challenge":"AAECAwQFBgcICQoLDA0ODw"}}"#)
            .expect("message")
    }

    #[test]
    fn the_bridge_is_off_unless_a_command_is_configured() {
        assert!(bridge(Vec::new()).is_none());
        assert!(bridge(vec![String::new()]).is_none());
        let bridge = bridge(vec!["authenticator".into()]).expect("bridge");
        assert!(bridge.page_function().contains(&bridge.deliver_key));
        assert!(!bridge.page_function().contains("__DELIVER_KEY__"));
        assert!(!bridge.page_function().contains("__MAX_MESSAGE_BYTES__"));
        assert!(bridge.binding_source().contains(bridge.binding()));
    }

    #[test]
    fn page_messages_are_bounded_and_typed() {
        assert!(matches!(
            get_request(),
            PageMessage::Request {
                id: 7,
                op: Operation::Get,
                ..
            }
        ));
        assert!(matches!(
            PageMessage::parse(r#"{"kind":"abort","id":3}"#),
            Some(PageMessage::Abort { id: 3 })
        ));
        assert!(matches!(
            PageMessage::parse(r#"{"kind":"request","id":1,"op":"delete"}"#),
            Some(PageMessage::Malformed { id: 1 })
        ));
        assert!(matches!(
            PageMessage::parse(r#"{"kind":"request","id":2,"op":"create","options":{"algorithms":[null]}}"#),
            Some(PageMessage::Malformed { id: 2 })
        ));
        assert!(PageMessage::parse(r#"{"kind":"abort"}"#).is_none());
        assert!(PageMessage::parse(&" ".repeat(MAX_PAGE_MESSAGE_BYTES + 1)).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn refusals_answer_at_once_and_one_request_runs_at_a_time() {
        let mut bridge = bridge(vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()]).expect("bridge");
        let PageMessage::Request { id, op, options } = get_request() else {
            unreachable!("request");
        };
        let refused = bridge
            .start(1, id, op, &options, Err(PageError::insecure_origin()), None)
            .expect("refused at once");
        assert!(refused.contains("SecurityError"));
        assert!(!bridge.frame_slot.passkey_pending());

        let origin = CallerOrigin::parse("https://example.com");
        assert!(bridge.start(1, id, op, &options, origin.clone(), None).is_none());
        assert!(bridge.frame_slot.passkey_pending());
        let busy = bridge.start(2, 8, op, &options, origin.clone(), None).expect("busy");
        assert!(busy.contains("Another passkey request is waiting"));

        assert!(!bridge.abort(&2, id), "another realm cannot abort the request");
        assert!(bridge.abort(&1, id));
        assert!(!bridge.frame_slot.passkey_pending());
        assert!(bridge.poll().is_none());

        for _ in 1..MAX_STARTS_PER_WINDOW {
            assert!(bridge.start(1, id, op, &options, origin.clone(), None).is_none());
            assert!(bridge.abort(&1, id));
        }
        let limited = bridge.start(1, id, op, &options, origin, None).expect("rate limited");
        assert!(limited.contains("Too many passkey requests"));
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_command_becomes_a_not_allowed_reply_for_the_same_realm() {
        let mut bridge =
            bridge(vec!["/bin/sh".into(), "-c".into(), "cat >/dev/null; echo nope".into()]).expect("bridge");
        let PageMessage::Request { id, op, options } = get_request() else {
            unreachable!("request");
        };
        assert!(
            bridge
                .start(5, id, op, &options, CallerOrigin::parse("https://example.com"), None)
                .is_none()
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (target, reply) = loop {
            if let Some(done) = bridge.poll() {
                break done;
            }
            assert!(std::time::Instant::now() < deadline, "command did not finish");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(target, 5);
        let reply: serde_json::Value = serde_json::from_str(&reply).expect("reply");
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["error"]["name"], "NotAllowedError");
        assert!(!bridge.frame_slot.passkey_pending());
    }
}
