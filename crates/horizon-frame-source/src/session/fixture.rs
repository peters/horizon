//! A session without a program or a worker thread, so a UI can test the input and
//! the messages it sends to a frame source program.
use std::{
    sync::mpsc,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use super::{Event, Session, Shared, State};
use crate::InputEvent;
use crate::control::Outgoing;
use horizon_vnc::Status;

/// What a fixture session was asked to send, in the order it was sent.
pub struct SentFrameInput(mpsc::Receiver<Event>);

impl SentFrameInput {
    /// Everything sent since the last call, in order: the input events, and no
    /// other message.
    pub fn drain(&mut self) -> Vec<InputEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.0.try_recv() {
            if let Event::Send(Outgoing::Input(input)) = event {
                events.push(input);
            }
        }
        events
    }

    /// Every size that the viewer asked the program to draw at, in order.
    pub fn drain_sizes(&mut self) -> Vec<[u32; 2]> {
        let mut sizes = Vec::new();
        while let Ok(event) = self.0.try_recv() {
            if let Event::Size(size) = event {
                sizes.push(size);
            }
        }
        sizes
    }
}

impl Session {
    /// A session that keeps one picture and sends to no program. A test reads what
    /// it was asked to send.
    #[must_use]
    pub fn fixture() -> (Self, SentFrameInput) {
        let (events, receiver) = mpsc::channel();
        let session = Self {
            shared: Arc::new(Shared {
                // The program of a fixture is connected at once: a UI test of the
                // input does not need a handshake.
                state: Mutex::new(State {
                    status: Some(Status::Connected),
                    ..State::default()
                }),
                taken: std::sync::Condvar::new(),
                wake: Box::new(|_wake| {}),
                latest_full: Mutex::new(None),
            }),
            live: Arc::new(AtomicBool::new(true)),
            events,
            thread: None,
        };
        (session, SentFrameInput(receiver))
    }
}
