//! The events of the bridge of one conversation: the pass that reads them into the
//! entries of the conversation, and the end of a program that did not continue.

use horizon_core::agent_bridge::BridgeEvent;

use super::Session;
use super::{LIMIT_NOTICE, Phase, RESUME_FAILURE_NOTICE};

impl Session {
    /// Reads every event of the bridge of this conversation. It never waits.
    pub(in super::super) fn poll(&mut self) {
        let events = match &self.bridge {
            Some(bridge) => bridge.poll(),
            None => return,
        };
        if events.is_empty() {
            return;
        }
        let mut ended = false;
        let mut resume_failed = false;
        let mut started = false;
        for event in events {
            match &event {
                BridgeEvent::SessionStarted { .. } => started = true,
                BridgeEvent::Limit {
                    using_overage: true, ..
                } if !self.limit_noticed => {
                    self.limit_noticed = true;
                    self.conversation.notice(LIMIT_NOTICE);
                }
                BridgeEvent::TurnEnded { failed, .. } => {
                    if *failed {
                        resume_failed |= self.resume_watch;
                    } else {
                        self.resume_watch = false;
                    }
                }
                BridgeEvent::Ended { .. } => {
                    ended = true;
                    resume_failed |= self.resume_watch;
                }
                _ => {}
            }
            self.conversation.apply(event);
        }
        if started {
            // The session id is text of the program, so the dock keeps the value
            // that the conversation cut: one line of `MAX_SUMMARY_CHARS`
            // characters, which the next start passes as one argument.
            let session_id = self.conversation.snapshot().session_id.clone();
            if session_id.as_deref().is_some_and(|id| !id.is_empty()) {
                self.resume_id = session_id;
            }
        }
        if resume_failed {
            self.forget_resumed_session();
        }
        if ended {
            self.bridge = None;
            self.phase = Phase::Ended;
        }
    }

    /// Forgets the stored session that the program did not continue, and says so
    /// in one notice. The entries of the conversation stay, and the next message
    /// starts a new session.
    pub(super) fn forget_resumed_session(&mut self) {
        self.resume_watch = false;
        self.resume_id = None;
        self.conversation.notice(RESUME_FAILURE_NOTICE);
    }
}
