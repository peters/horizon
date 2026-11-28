//! The start of the bridge of one conversation: the request that a frame queues
//! on the worker, the answer of the worker, and the notice of a start that failed.

use horizon_core::agent_bridge::{Bridge, BridgeEvent, SessionChoice};

use super::{Phase, START_FAILURE, Session, Start};

impl Session {
    /// Takes the request of one start, and makes every start in flight stale.
    pub(in super::super) fn begin_start(&mut self) -> Start {
        self.generation = self.generation.saturating_add(1);
        self.phase = Phase::Starting;
        self.limit_noticed = false;
        let session = match &self.resume_id {
            Some(session_id) => SessionChoice::Resume(session_id.clone()),
            None => SessionChoice::New(horizon_core::new_local_id()),
        };
        self.resume_watch = matches!(session, SessionChoice::Resume(_));
        Start {
            generation: self.generation,
            session,
        }
    }

    /// Keeps the bridge of a start that answers this generation and sends the
    /// messages that waited for it. It answers the bridge of a stale answer,
    /// which the caller stops, and `None` for a stale answer that failed.
    pub(in super::super) fn adopt(
        &mut self,
        generation: u64,
        outcome: Option<Result<Bridge, horizon_core::agent_bridge::BridgeError>>,
    ) -> Option<Bridge> {
        if generation != self.generation {
            // The person stopped this conversation, or its workspace closed,
            // while the start ran: the bridge of that start is not ours, and the
            // messages that wait belong to the start that replaced it.
            return outcome.and_then(Result::ok);
        }
        match outcome {
            Some(Ok(bridge)) => {
                // A live bridge is never replaced by a second start of one
                // workspace: the bridge that runs keeps the conversation, and the
                // bridge of this start goes back to the caller for its stop.
                if self.bridge.is_some() {
                    return Some(bridge);
                }
                self.bridge = Some(bridge);
                self.phase = Phase::Running;
                self.send_waiting();
                None
            }
            Some(Err(error)) => {
                self.fail(&failure_detail(&error));
                None
            }
            None => {
                self.fail("the start did not answer");
                None
            }
        }
    }

    /// Sends the messages that waited to the bridge of this conversation.
    fn send_waiting(&mut self) {
        for text in std::mem::take(&mut self.waiting) {
            let Some(bridge) = self.bridge.as_ref() else {
                return;
            };
            if bridge.send(text).is_err() {
                // The program ended before this message reached it. Its events
                // still hold the end of the program, and a poll of a later frame
                // reads them and names why it ended, so the bridge stays until
                // then.
                return;
            }
        }
    }

    /// Ends this conversation with the notice of a program that did not start.
    fn fail(&mut self, detail: &str) {
        self.phase = Phase::Ended;
        self.waiting.clear();
        // The message of that start ends with it, so the field offers the send
        // control again.
        self.conversation.apply(BridgeEvent::TurnEnded {
            failed: false,
            duration_ms: None,
            cost_usd: None,
        });
        self.conversation.notice(&format!("{START_FAILURE}: {detail}"));
        if self.resume_watch {
            // The stored session of this start did not continue either, so no
            // later start resumes it: the next message starts a new session.
            self.forget_resumed_session();
        }
    }
}

/// The words of the operating system under the error of a start.
fn failure_detail(error: &horizon_core::agent_bridge::BridgeError) -> String {
    match std::error::Error::source(error) {
        Some(source) => source.to_string(),
        None => error.to_string(),
    }
}
