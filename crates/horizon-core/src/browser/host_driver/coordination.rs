//! The agent coordination of a driver on a browser host.
//!
//! The manifest that an agent writes lives on the computer of the person, not on
//! the browser host. The driver of the host therefore does not read it: this
//! module carries the same facts over the link of the host driver. What the
//! driver learns about its own page (its address, its title, the target and
//! whether it came back across a lost link) is sent to this computer, which
//! writes it into the manifest. What the manifest says about the agent (its
//! identity, a handoff request, claimed actions) arrives in
//! [`HostDriverSignals`] and is what the driver's page loop runs.
//!
//! The trait is the one of a local session, so an agent tool cannot tell a
//! Firefox panel of the host from a local one: the same action, the same result,
//! the same handoff.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, mpsc};

use horizon_browser::host_driver::{HostDriverCoordination, HostDriverReply, HostDriverSignals};
use horizon_browser::{
    AgentAction, AgentActionResult, BrowserAuditEntry, BrowserCoordination, CoordinationOwnership, CoordinationSignals,
    CoordinationState, HandoffRequest,
};

/// A queue of replies for the link, shared with the driver thread of the page.
///
/// The page loop of the driver thread must never wait for the link, so every
/// message it produces for this computer is left here and the thread that owns
/// the link writes it in its own pass.
pub(super) type Outbox = mpsc::Sender<HostDriverReply>;

/// Coordination over the link of a browser host.
pub(super) struct WireCoordination {
    outbox: Outbox,
    signals: Mutex<CoordinationSignals>,
    /// Actions that this computer asked for as one operation each, which the
    /// next pass of the page loop claims like an action of the manifest.
    direct: Mutex<Vec<AgentAction>>,
    /// Handoff requests whose acknowledgement this computer already received,
    /// so one request does not travel on every pass.
    acknowledged: Mutex<BTreeSet<String>>,
}

impl std::fmt::Debug for WireCoordination {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WireCoordination")
            .field("signals", &self.signals)
            .finish_non_exhaustive()
    }
}

impl WireCoordination {
    pub(super) fn new(outbox: Outbox) -> Arc<Self> {
        Arc::new(Self {
            outbox,
            signals: Mutex::new(CoordinationSignals::default()),
            direct: Mutex::new(Vec::new()),
            acknowledged: Mutex::new(BTreeSet::new()),
        })
    }

    /// Take the signals of this computer for the next pass of the page loop.
    pub(super) fn accept_signals(&self, signals: HostDriverSignals) {
        let mut current = lock(&self.signals);
        current.owner = signals.owner;
        current.handoff = signals.handoff.map(|handoff| HandoffRequest {
            request_id: handoff.request_id,
            reason: handoff.reason,
        });
        current.actions = signals.actions;
    }

    /// Queue one operation that this computer sent as its own message, so the
    /// page loop runs it exactly as it runs an action of the manifest.
    pub(super) fn queue_action(&self, action_id: String, actor: String, action: horizon_browser::BrowserControlAction) {
        lock(&self.direct).push(AgentAction {
            action_id,
            actor,
            requested_at_millis: now_millis(),
            action,
        });
    }

    /// Send one reply that the page loop produced.
    fn send(&self, reply: HostDriverReply) {
        // A link that ended drops the receiver; the session keeps the page for
        // the next link, so a lost message is not an error of the session.
        let _ = self.outbox.send(reply);
    }
}

impl BrowserCoordination for WireCoordination {
    fn prepare(&self, _panel_local_id: &str, _timeout: std::time::Duration) -> bool {
        // The stale state of a previous session is a file of this computer, not
        // of the host, and this session owns its own end of the link.
        true
    }

    fn initialize(&self, _panel_local_id: &str, state: &CoordinationState) -> std::io::Result<()> {
        self.send(HostDriverReply::Coordination(Box::new(convert(state, None))));
        Ok(())
    }

    fn update(&self, _panel_local_id: &str, state: &CoordinationState) -> std::io::Result<()> {
        self.send(HostDriverReply::Coordination(Box::new(convert(state, None))));
        Ok(())
    }

    fn set_user_active(&self, _panel_local_id: &str, _active: bool) -> std::io::Result<()> {
        // The person of this computer is the one whose activity matters, and
        // that computer knows it: the panel sends it as a signal instead.
        Ok(())
    }

    fn signals(&self, _panel_local_id: &str) -> std::io::Result<CoordinationSignals> {
        let mut stored = lock(&self.signals);
        let mut signals = stored.clone();
        // An action is claimed once: a host that returned the same request
        // twice would run it twice on the page.
        stored.actions.clear();
        drop(stored);
        let direct = std::mem::take(&mut *lock(&self.direct));
        if !direct.is_empty() {
            signals.actions.extend(direct);
        }
        Ok(signals)
    }

    fn observe_ownership(&self, _panel_local_id: &str) -> std::io::Result<CoordinationOwnership> {
        let signals = lock(&self.signals);
        Ok(CoordinationOwnership {
            owner: signals.owner.clone(),
            handoff: signals.handoff.clone(),
        })
    }

    fn acknowledge_handoff(&self, _panel_local_id: &str, request_id: &str) -> std::io::Result<bool> {
        if !lock(&self.acknowledged).insert(request_id.to_string()) {
            return Ok(true);
        }
        self.send(HostDriverReply::HandoffAck {
            request_id: request_id.to_string(),
        });
        Ok(true)
    }

    fn record_action(&self, _panel_local_id: &str, entry: &BrowserAuditEntry) -> std::io::Result<()> {
        self.send(HostDriverReply::Audit(Box::new(entry.clone())));
        Ok(())
    }

    fn complete_action(&self, _panel_local_id: &str, result: &AgentActionResult) -> std::io::Result<()> {
        self.send(HostDriverReply::Action(
            horizon_browser::host_driver::HostDriverActionResult {
                action_id: result.action_id.clone(),
                result: result.clone(),
            },
        ));
        Ok(())
    }

    fn remove(&self, _panel_local_id: &str, _timeout: std::time::Duration) -> bool {
        // The manifest of this computer is removed by the panel that owns it.
        true
    }
}

/// The state of a page of the host, in the words of the wire.
fn convert(state: &CoordinationState, owner: Option<String>) -> HostDriverCoordination {
    HostDriverCoordination {
        owner,
        handoff: None,
        url: state.url.clone(),
        title: state.title.clone(),
        target_id: state.target_id.clone(),
        page_resumed: state.page_resumed,
        automation_disclosure: state.automation_disclosure,
        file_chooser: state.file_chooser,
        passkey_pending: state.passkey_pending,
    }
}

/// One handoff request of the wire, for a message that names its own fields.
#[cfg(test)]
#[must_use]
pub(super) fn handoff_of(request: &HandoffRequest) -> horizon_browser::host_driver::HostDriverHandoff {
    horizon_browser::host_driver::HostDriverHandoff {
        request_id: request.request_id.clone(),
        reason: request.reason.clone(),
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

fn lock<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::{WireCoordination, handoff_of};
    use horizon_browser::host_driver::{HostDriverReply, HostDriverSignals};
    use horizon_browser::{BrowserControlAction, BrowserCoordination, CoordinationState, HandoffRequest};

    fn wire() -> (std::sync::Arc<WireCoordination>, mpsc::Receiver<HostDriverReply>) {
        let (sender, receiver) = mpsc::channel();
        (WireCoordination::new(sender), receiver)
    }

    use std::sync::mpsc;

    #[test]
    fn the_page_of_the_host_travels_to_the_manifest_of_this_computer() {
        let (coordination, outbox) = wire();
        let state = CoordinationState {
            url: "https://example.test/".to_string(),
            title: "Example".to_string(),
            target_id: "target-1".to_string(),
            page_resumed: true,
            ..CoordinationState::default()
        };
        coordination.initialize("panel", &state).expect("initialize");
        let HostDriverReply::Coordination(message) = outbox.try_recv().expect("one message") else {
            panic!("the state of the page");
        };
        assert_eq!(message.url, "https://example.test/");
        assert_eq!(message.title, "Example");
        assert_eq!(message.target_id, "target-1");
        assert!(message.page_resumed);

        coordination.update("panel", &state).expect("update");
        assert!(outbox.try_recv().is_ok(), "an update travels as one message");
    }

    #[test]
    fn the_signals_of_this_computer_reach_the_page_loop_once() {
        let (coordination, _outbox) = wire();
        let action = horizon_browser::AgentAction {
            action_id: "a-1".to_string(),
            actor: "agent".to_string(),
            requested_at_millis: 1,
            action: BrowserControlAction::Reload,
        };
        coordination.accept_signals(HostDriverSignals {
            owner: Some("agent".to_string()),
            handoff: Some(handoff_of(&HandoffRequest {
                request_id: "h-1".to_string(),
                reason: "a code".to_string(),
            })),
            actions: vec![action.clone()],
        });
        let first = coordination.signals("panel").expect("signals");
        assert_eq!(first.owner.as_deref(), Some("agent"));
        assert_eq!(first.handoff.expect("a handoff").request_id, "h-1");
        assert_eq!(first.actions, vec![action], "the claimed action is handed over");
        let second = coordination.signals("panel").expect("signals");
        assert!(second.actions.is_empty(), "an action is claimed once");
    }

    #[test]
    fn one_operation_of_an_agent_tool_joins_the_actions_of_the_loop() {
        let (coordination, _outbox) = wire();
        coordination.queue_action(
            "a-2".to_string(),
            "agent".to_string(),
            BrowserControlAction::Snapshot { max_nodes: 5 },
        );
        let signals = coordination.signals("panel").expect("signals");
        assert_eq!(signals.actions.len(), 1);
        assert_eq!(signals.actions[0].action_id, "a-2");
        assert!(coordination.signals("panel").expect("again").actions.is_empty());
    }

    #[test]
    fn a_result_and_an_audit_record_of_the_host_travel_back() {
        let (coordination, outbox) = wire();
        let result =
            horizon_browser::AgentActionResult::completed("a-3", horizon_browser::BrowserControlValue::Accepted);
        coordination.complete_action("panel", &result).expect("a result");
        let HostDriverReply::Action(message) = outbox.try_recv().expect("one message") else {
            panic!("a result");
        };
        assert_eq!(message.action_id, "a-3");
        assert_eq!(message.result, result);

        let entry = horizon_browser::BrowserAuditEntry::new(
            "a-4".to_string(),
            horizon_browser::BrowserAuditActor::System,
            horizon_browser::BrowserAuditStatus::Completed,
            horizon_browser::BrowserAuditAction::Reload,
        );
        coordination.record_action("panel", &entry).expect("an audit record");
        assert!(matches!(outbox.try_recv(), Ok(HostDriverReply::Audit(_))));
    }

    #[test]
    fn a_handoff_request_is_acknowledged_once() {
        let (coordination, outbox) = wire();
        assert!(coordination.acknowledge_handoff("panel", "h-2").expect("acknowledged"));
        assert!(matches!(
            outbox.try_recv(),
            Ok(HostDriverReply::HandoffAck { request_id }) if request_id == "h-2"
        ));
        assert!(coordination.acknowledge_handoff("panel", "h-2").expect("acknowledged"));
        assert!(
            outbox.try_recv().is_err(),
            "a second acknowledgement of the same request travels nowhere"
        );
    }
}
