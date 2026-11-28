//! The lines of Horizon that go into the program of one conversation: the wake of a
//! press, and the queue that holds a line until the turn of the program ends.

use super::{Session, WAKE_NOTICE};

impl Session {
    /// Sends the one wake line of Horizon to the program of this conversation as
    /// a turn of its own, so the program learns that a press waits, and names
    /// that turn in the conversation. The turn is no message of the person, so
    /// it adds no [`Entry::Person`]. It answers whether the line went out.
    pub(in super::super) fn wake(&mut self, line: &str) -> bool {
        let Some(bridge) = self.bridge.take() else {
            return false;
        };
        if bridge.send(line.to_owned()).is_err() {
            // The program took no line, so its end arrives with the events of this
            // bridge: no notice claims a wake that did not go out.
            self.bridge = Some(bridge);
            return false;
        }
        self.bridge = Some(bridge);
        self.conversation.system_said(WAKE_NOTICE);
        true
    }
    /// Whether this conversation can take a line of Horizon now: its program runs,
    /// no turn of it runs, and no earlier line of Horizon waits.
    pub(in super::super) fn takes_a_system_line(&self) -> bool {
        !self.busy() && self.has_bridge() && self.system_waiting.is_empty()
    }

    /// Sends one line of Horizon into the bridge of this conversation now, and
    /// answers whether the program took it. The caller has asked
    /// [`Self::takes_a_system_line`]. The line is no message of the person, so it
    /// adds no [`Entry::Person`].
    pub(in super::super) fn send_system_line(&mut self, line: &str) -> bool {
        self.send_system_now(line)
    }

    /// Sends one line of Horizon to the program of this conversation as a turn of
    /// its own, and answers whether it went out now. A conversation that runs a turn
    /// keeps the line until that turn ends, so the words of Horizon never cut into
    /// the answer of the program; a conversation without a bridge holds it too.
    pub(in super::super) fn system_message(&mut self, line: &str) -> bool {
        if self.busy() || !self.has_bridge() {
            self.system_waiting.push(line.to_owned());
            return false;
        }
        self.send_system_now(line)
    }

    /// Sends the line of Horizon that waits, when no turn of this conversation
    /// runs. It answers whether a line went out.
    pub(in super::super) fn flush_system(&mut self) -> bool {
        if self.system_waiting.is_empty() || self.busy() || !self.has_bridge() {
            return false;
        }
        let line = self.system_waiting.remove(0);
        if self.send_system_now(&line) {
            return true;
        }
        self.system_waiting.insert(0, line);
        false
    }

    /// Sends one line of Horizon into the bridge that runs, and marks the
    /// conversation busy for it. The line is no message of the person, so it adds
    /// no [`Entry::Person`] and no notice: the words of Horizon that name such a
    /// turn are the words of its caller.
    fn send_system_now(&mut self, line: &str) -> bool {
        let Some(bridge) = self.bridge.take() else {
            return false;
        };
        if bridge.send(line.to_owned()).is_err() {
            // The program took no line, so its end arrives with the events of this
            // bridge: no state of this conversation claims a turn that did not start.
            self.bridge = Some(bridge);
            return false;
        }
        self.bridge = Some(bridge);
        self.conversation.turn_started();
        true
    }
}
