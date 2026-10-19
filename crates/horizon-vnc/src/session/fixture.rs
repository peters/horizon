//! A connected session without a VNC server or a worker thread, so a UI can
//! test what it does with an image and with the input it sends.
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use std::time::Instant;

use tokio::sync::mpsc;

use super::{Image, Session, Shared, State, Status, StreamEvidence};
use crate::{Frame, InputEvent, Region, ViewOptions};

/// The input that a fixture session was asked to send.
pub struct SentInput(mpsc::UnboundedReceiver<InputEvent>);

impl SentInput {
    /// Everything sent since the last call, in order.
    pub fn drain(&mut self) -> Vec<InputEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.0.try_recv() {
            events.push(event);
        }
        events
    }
}

impl Session {
    /// A connected session that holds one untaken `image` made with
    /// `produced_with`, and `latest_full` as its full desktop.
    #[must_use]
    pub fn fixture(
        latest_full: Frame,
        image: Frame,
        produced_with: ViewOptions,
        keep_server_text: fn(&str) -> bool,
    ) -> (Self, SentInput) {
        let (input, receiver) = mpsc::unbounded_channel();
        let desktop = latest_full.size();
        let changed = Region::whole(image.size());
        let session = Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    stream: StreamEvidence {
                        sequence: 1,
                        last_frame: Some(Instant::now()),
                    },
                    image: Some(Image {
                        frame: image,
                        produced_with,
                        changed,
                    }),
                    status: Some(Status::Connected),
                    received_frame_sequence: 1,
                    visible: true,
                    options: produced_with,
                    desktop: Some(desktop),
                    server_name: None,
                    server_text: None,
                }),
                wake: Box::new(|_| {}),
                keep_server_text,
            }),
            latest_full: Arc::new(Mutex::new(Some(Arc::new(latest_full)))),
            live: Arc::new(AtomicBool::new(true)),
            input,
            stop: None,
            thread: None,
        };
        (session, SentInput(receiver))
    }

    /// Give the session `text` as if the server had sent it as clipboard
    /// text. Returns whether the session kept it.
    #[must_use]
    pub fn push_server_text(&self, text: &str) -> bool {
        self.shared.publish_text(text)
    }
}
