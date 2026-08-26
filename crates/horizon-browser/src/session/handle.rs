//! Panel-side session handle, event wake-up, and committed URL state.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};

use super::{BrowserCommand, BrowserEvent, BrowserSession, BrowserShutdownSignal};
use crate::frames::FrameSlot;

/// Latest URL that the driver has observed the browser commit. This survives
/// the event receiver being dropped during shutdown so the panel can persist
/// the driver's final state after teardown completes.
#[derive(Clone, Default)]
pub struct CommittedUrl(Arc<Mutex<Option<String>>>);

impl CommittedUrl {
    pub fn publish(&self, url: &str) {
        *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(url.to_string());
    }

    #[must_use]
    pub fn snapshot(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

/// UI callback invoked after the driver queues an event. Keeping this generic
/// avoids coupling the browser engine to egui while still allowing a native
/// event loop to wake immediately from its idle backoff.
pub type BrowserEventWaker = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Clone, Default)]
pub(crate) struct BrowserEventWake {
    pub(crate) callback: Arc<Mutex<Option<BrowserEventWaker>>>,
}

impl BrowserEventWake {
    pub(super) fn is_set(&self) -> bool {
        self.callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    pub(super) fn set(&self, callback: BrowserEventWaker) {
        *self.callback.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(callback);
    }

    fn wake(&self) {
        let callback = self
            .callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(callback) = callback {
            callback();
        }
    }
}

pub(crate) struct BrowserEventSender {
    pub(crate) tx: mpsc::Sender<BrowserEvent>,
    pub(crate) wake: BrowserEventWake,
    pub(crate) committed_url: CommittedUrl,
}

impl BrowserEventSender {
    pub(crate) fn send(&self, event: BrowserEvent) -> Result<(), mpsc::SendError<BrowserEvent>> {
        if let BrowserEvent::UrlChanged(url) = &event {
            self.committed_url.publish(url);
        }
        self.tx.send(event)?;
        self.wake.wake();
        Ok(())
    }
}

impl BrowserSession {
    #[must_use]
    pub fn send(&self, command: BrowserCommand) -> bool {
        if matches!(command, BrowserCommand::Stop) {
            self.stop_requested.store(true, Ordering::Release);
        }
        self.command_tx.send(command)
    }

    pub fn set_event_waker(&self, callback: BrowserEventWaker) {
        self.event_wake.set(callback);
    }

    #[must_use]
    pub fn needs_event_waker(&self) -> bool {
        !self.event_wake.is_set()
    }

    /// Send `Stop` and return the teardown-completion signal. The receiver
    /// resolves once the driver has closed its protocol connection, killed
    /// the browser, and removed the manifest.
    #[must_use]
    pub fn shutdown_signal(self) -> BrowserShutdownSignal {
        self.stop_requested.store(true, Ordering::Release);
        let _ = self.command_tx.send(BrowserCommand::Stop);
        self.frame_slot.release_notification();
        BrowserShutdownSignal::running(
            self.completion_rx,
            self.process_control,
            self.panel_local_id,
            self.coordination,
        )
    }

    /// Return the existing teardown-completion signal after the driver has
    /// already announced `Stopped`; no second stop request is necessary.
    #[must_use]
    pub fn completion_signal(self) -> BrowserShutdownSignal {
        self.frame_slot.release_notification();
        BrowserShutdownSignal::running(
            self.completion_rx,
            self.process_control,
            self.panel_local_id,
            self.coordination,
        )
    }

    #[must_use]
    pub fn committed_url(&self) -> CommittedUrl {
        self.committed_url.clone()
    }
}

pub(crate) fn publish_frame(event_tx: &BrowserEventSender, frame_slot: &FrameSlot, seq: u64) {
    if frame_slot.claim_notification() && event_tx.send(BrowserEvent::Frame { seq }).is_err() {
        frame_slot.release_notification();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn queued_browser_events_wake_the_registered_ui_callback() {
        let wake_count = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&wake_count);
        let wake = BrowserEventWake::default();
        wake.set(Arc::new(move || {
            counted.fetch_add(1, Ordering::Relaxed);
        }));
        let (tx, rx) = mpsc::channel();
        let sender = BrowserEventSender {
            tx,
            wake,
            committed_url: CommittedUrl::default(),
        };

        assert!(sender.send(BrowserEvent::Ready).is_ok());
        assert_eq!(rx.recv(), Ok(BrowserEvent::Ready));
        assert_eq!(wake_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn committed_url_survives_a_dropped_event_receiver() {
        let committed_url = CommittedUrl::default();
        let (tx, rx) = mpsc::channel();
        let sender = BrowserEventSender {
            tx,
            wake: BrowserEventWake::default(),
            committed_url: committed_url.clone(),
        };
        drop(rx);

        assert!(
            sender
                .send(BrowserEvent::UrlChanged("https://example.com/final".to_string()))
                .is_err()
        );
        assert_eq!(committed_url.snapshot().as_deref(), Some("https://example.com/final"));
    }
}
