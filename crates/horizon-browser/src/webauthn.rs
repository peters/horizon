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
/// Requests wait here until the driver loop handles them, outside event
/// handling, so a page that floods the channel cannot nest protocol
/// round trips. A flood beyond this bound is dropped.
const MAX_QUEUED_MESSAGES: usize = 16;

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
struct Admitted<T> {
    ticket: u64,
    target: T,
    page_id: u64,
    withdrawn: Option<Withdrawal>,
}

/// Why an admitted request ended before its command started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Withdrawal {
    /// The page aborted it and already rejected the promise.
    Aborted,
    /// Its realm navigated or went away; the page gets `NotAllowedError`.
    Ended,
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
    /// Requests whose caller origin is being read. The read drains browser
    /// events, so a nested request sees the panel busy while one of them is
    /// live, and an abort or a navigation of the caller in that time
    /// withdraws it. A withdrawn entry no longer blocks a new request; it
    /// waits here until its own [`Self::start`] call consumes it.
    admitted: Vec<Admitted<T>>,
    next_ticket: u64,
    inbox: VecDeque<(T, PageMessage)>,
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
            admitted: Vec::new(),
            next_ticket: 0,
            inbox: VecDeque::new(),
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

    /// Queue a request or malformed message for the driver loop. Returns
    /// the busy reply for the page when the queue is full.
    pub(crate) fn enqueue(&mut self, target: T, message: PageMessage) -> Option<String> {
        if self.inbox.len() >= MAX_QUEUED_MESSAGES {
            let id = match message {
                PageMessage::Request { id, .. } | PageMessage::Malformed { id } | PageMessage::Abort { id } => id,
            };
            return Some(error_reply(id, PageError::busy()));
        }
        self.inbox.push_back((target, message));
        None
    }

    /// The next queued page message.
    pub(crate) fn next_message(&mut self) -> Option<(T, PageMessage)> {
        self.inbox.pop_front()
    }

    /// Admit a request before the engine reads its caller origin. Returns
    /// the ticket that the engine passes to [`Self::start`], or a reply for
    /// the page when another request is live or the panel reached its start
    /// limit.
    pub(crate) fn admit(&mut self, target: T, page_id: u64) -> Result<u64, String> {
        let live = self.admitted.iter().any(|admitted| admitted.withdrawn.is_none());
        if self.active.is_some() || live {
            return Err(error_reply(page_id, PageError::busy()));
        }
        let now = Instant::now();
        while self
            .recent_starts
            .front()
            .is_some_and(|started| now.duration_since(*started) >= START_WINDOW)
        {
            self.recent_starts.pop_front();
        }
        if self.recent_starts.len() >= MAX_STARTS_PER_WINDOW {
            return Err(error_reply(page_id, PageError::too_many()));
        }
        self.next_ticket += 1;
        self.admitted.push(Admitted {
            ticket: self.next_ticket,
            target,
            page_id,
            withdrawn: None,
        });
        Ok(self.next_ticket)
    }

    /// Start the admitted request once the engine resolved its caller
    /// origin. Returns a reply for the page when the request ends at once;
    /// a request withdrawn by an abort ends without a reply, and one ended
    /// by its realm gets `NotAllowedError`. Neither starts a command.
    pub(crate) fn start(
        &mut self,
        ticket: u64,
        operation: Operation,
        options: &PageOptions,
        origin: Result<CallerOrigin, PageError>,
        agent: Option<&str>,
    ) -> Option<(T, String)> {
        let position = self.admitted.iter().position(|admitted| admitted.ticket == ticket)?;
        let Admitted {
            target,
            page_id,
            withdrawn,
            ..
        } = self.admitted.remove(position);
        match withdrawn {
            Some(Withdrawal::Aborted) => return None,
            Some(Withdrawal::Ended) => return Some((target, error_reply(page_id, PageError::not_allowed()))),
            None => {}
        }
        let prepared = match origin.and_then(|origin| request::prepare(operation, options, &origin, agent)) {
            Ok(prepared) => prepared,
            Err(error) => return Some((target, error_reply(page_id, error))),
        };
        if self.active.is_some() {
            return Some((target, error_reply(page_id, PageError::busy())));
        }
        self.recent_starts.push_back(Instant::now());
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
        self.inbox.retain(|(queued, message)| {
            !(queued == target && matches!(message, PageMessage::Request { id, .. } if *id == page_id))
        });
        self.withdraw_where(Withdrawal::Aborted, |admitted| {
            admitted.target == *target && admitted.page_id == page_id
        });
        self.cancel_where(|active| active.target == *target && active.page_id == page_id)
            .is_some()
    }

    /// The realm that sent requests navigated or went away: kill the
    /// running command and drop its queued requests. Returns a
    /// `NotAllowedError` reply for each, which still settles the promises
    /// when a navigation stops before it commits.
    pub(crate) fn cancel_for(&mut self, matches: impl Fn(&T) -> bool) -> Vec<(T, String)> {
        self.withdraw_where(Withdrawal::Ended, |admitted| matches(&admitted.target));
        let mut replies = Vec::new();
        self.inbox.retain(|(target, message)| {
            let keep = !matches(target);
            if !keep && let PageMessage::Request { id, .. } | PageMessage::Malformed { id } = message {
                replies.push((target.clone(), error_reply(*id, PageError::not_allowed())));
            }
            keep
        });
        if let Some(ended) = self.cancel_where(|active| matches(&active.target)) {
            replies.push((ended.target, error_reply(ended.page_id, PageError::not_allowed())));
        }
        replies
    }

    fn withdraw_where(&mut self, reason: Withdrawal, matches: impl Fn(&Admitted<T>) -> bool) {
        for admitted in self.admitted.iter_mut().filter(|admitted| matches(admitted)) {
            admitted.withdrawn.get_or_insert(reason);
        }
    }

    fn cancel_where(&mut self, matches: impl Fn(&Active<T>) -> bool) -> Option<Active<T>> {
        if !self.active.as_ref().is_some_and(matches) {
            return None;
        }
        self.frame_slot.set_passkey_pending(false);
        self.active.take()
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

    #[test]
    fn refusals_answer_at_once_and_one_request_runs_at_a_time() {
        let mut bridge = bridge(vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()]).expect("bridge");
        let PageMessage::Request { id, op, options } = get_request() else {
            unreachable!("request");
        };
        let ticket = bridge.admit(1, id).expect("admitted");
        let (target, refused) = bridge
            .start(ticket, op, &options, Err(PageError::insecure_origin()), None)
            .expect("refused at once");
        assert_eq!(target, 1);
        assert!(refused.contains("SecurityError"));
        assert!(!bridge.frame_slot.passkey_pending());
        let origin = CallerOrigin::parse("https://example.com");
        assert!(
            bridge.start(ticket, op, &options, origin.clone(), None).is_none(),
            "a used ticket starts nothing"
        );
        assert!(!bridge.frame_slot.passkey_pending());

        let ticket = bridge.admit(1, id).expect("admitted");
        let nested = bridge.admit(2, 8).expect_err("busy while the origin is read");
        assert!(nested.contains("Another passkey request is waiting"));
        assert!(bridge.start(ticket, op, &options, origin.clone(), None).is_none());
        assert!(bridge.frame_slot.passkey_pending());
        let busy = bridge.admit(2, 8).expect_err("busy");
        assert!(busy.contains("Another passkey request is waiting"));

        assert!(!bridge.abort(&2, id), "another realm cannot abort the request");
        assert!(bridge.abort(&1, id));
        assert!(!bridge.frame_slot.passkey_pending());
        assert!(bridge.poll().is_none());

        for _ in 1..MAX_STARTS_PER_WINDOW {
            let ticket = bridge.admit(1, id).expect("admitted");
            assert!(bridge.start(ticket, op, &options, origin.clone(), None).is_none());
            assert!(bridge.abort(&1, id));
        }
        let limited = bridge.admit(1, id).expect_err("rate limited");
        assert!(limited.contains("Too many passkey requests"));
    }

    #[test]
    fn an_abort_or_navigation_during_the_origin_read_withdraws_the_request() {
        let mut bridge = bridge(vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()]).expect("bridge");
        let PageMessage::Request { id, op, options } = get_request() else {
            unreachable!("request");
        };
        let origin = CallerOrigin::parse("https://example.com");
        let ticket = bridge.admit(1, id).expect("admitted");
        assert!(!bridge.abort(&2, id), "another realm cannot withdraw it");
        bridge.abort(&1, id);
        assert!(bridge.start(ticket, op, &options, origin.clone(), None).is_none());
        assert!(
            !bridge.frame_slot.passkey_pending(),
            "a withdrawn request starts no command"
        );

        let ticket = bridge.admit(1, id).expect("admitted");
        assert!(bridge.cancel_for(|target| *target == 1).is_empty());
        let (target, ended) = bridge
            .start(ticket, op, &options, origin.clone(), None)
            .expect("a navigation still settles the promise");
        assert_eq!(target, 1);
        assert!(ended.contains("NotAllowedError"));
        assert!(!bridge.frame_slot.passkey_pending());

        let ticket = bridge.admit(1, id).expect("the panel is free again");
        assert!(bridge.start(ticket, op, &options, origin, None).is_none());
        assert!(bridge.frame_slot.passkey_pending());
        let (target, ended) = bridge.cancel_for(|target| *target == 1).pop().expect("reply");
        assert_eq!(target, 1);
        assert!(ended.contains("NotAllowedError"));
        assert!(!bridge.frame_slot.passkey_pending());
    }

    #[test]
    fn queued_requests_are_bounded_and_an_abort_removes_its_request() {
        let mut bridge = bridge(vec!["authenticator".into()]).expect("bridge");
        for page_id in 0..MAX_QUEUED_MESSAGES as u64 {
            assert!(bridge.enqueue(1, PageMessage::Malformed { id: page_id }).is_none());
        }
        let busy = bridge
            .enqueue(1, PageMessage::Malformed { id: 99 })
            .expect("a flood gets a busy reply");
        assert!(busy.contains("\"id\":99") && busy.contains("NotAllowedError"));
        while bridge.next_message().is_some() {}
        assert!(bridge.enqueue(1, get_request()).is_none());
        assert!(bridge.enqueue(2, get_request()).is_none());
        bridge.abort(&1, 7);
        assert!(matches!(
            bridge.next_message(),
            Some((2, PageMessage::Request { id: 7, .. }))
        ));
        assert!(bridge.next_message().is_none());

        assert!(bridge.enqueue(3, get_request()).is_none());
        let ended = bridge.cancel_for(|target| *target == 3);
        assert_eq!(ended.len(), 1, "a navigation settles a queued request");
        assert!(ended[0].1.contains("NotAllowedError"));
        assert!(bridge.next_message().is_none(), "and drops it");
    }

    #[test]
    fn a_retry_after_an_abort_during_the_origin_read_is_admitted() {
        let mut bridge = bridge(vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()]).expect("bridge");
        let PageMessage::Request { id, op, options } = get_request() else {
            unreachable!("request");
        };
        let origin = CallerOrigin::parse("https://example.com");
        let first = bridge.admit(1, id).expect("admitted");
        bridge.abort(&1, id);
        let retry = bridge
            .admit(1, id + 1)
            .expect("an aborted request no longer blocks a retry");
        assert!(bridge.start(retry, op, &options, origin.clone(), None).is_none());
        assert!(bridge.frame_slot.passkey_pending(), "the retry runs");
        assert!(
            bridge.start(first, op, &options, origin, None).is_none(),
            "the aborted request ends without a reply"
        );
        assert!(bridge.frame_slot.passkey_pending(), "and leaves the retry running");
    }

    #[test]
    fn a_failing_command_becomes_a_not_allowed_reply_for_the_same_realm() {
        let mut bridge =
            bridge(vec!["/bin/sh".into(), "-c".into(), "cat >/dev/null; echo nope".into()]).expect("bridge");
        let PageMessage::Request { id, op, options } = get_request() else {
            unreachable!("request");
        };
        let ticket = bridge.admit(5, id).expect("admitted");
        assert!(
            bridge
                .start(ticket, op, &options, CallerOrigin::parse("https://example.com"), None)
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
