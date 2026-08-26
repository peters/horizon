//! Panel-side session handle, event wake-up, and committed URL state.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};

use super::{BrowserCommand, BrowserEvent, BrowserSession, BrowserShutdownSignal};
use crate::frames::FrameSlot;
use crate::{BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, BrowserAuditStatus, new_action_id};

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

    pub(crate) fn wake_ui(&self) {
        self.wake.wake();
    }
}

impl BrowserSession {
    #[must_use]
    pub fn send(&self, command: BrowserCommand) -> bool {
        let is_stop = matches!(command, BrowserCommand::Stop);
        if is_stop {
            self.stop_requested.store(true, Ordering::Release);
        }
        let actor = if matches!(command, BrowserCommand::SetViewport { .. } | BrowserCommand::Stop) {
            BrowserAuditActor::System
        } else {
            BrowserAuditActor::User
        };
        let action = BrowserAuditAction::from_command(&command);
        let accepted = self.command_tx.send(command) || is_stop;
        if (!accepted || is_stop)
            && let Some(coordination) = &self.coordination
        {
            let status = if accepted {
                BrowserAuditStatus::Dispatched
            } else {
                BrowserAuditStatus::Rejected
            };
            let entry = BrowserAuditEntry::new(new_action_id(), actor, status, action);
            if let Err(error) = coordination.record_action(&self.panel_local_id, &entry) {
                tracing::warn!(target: "browser", "failed to append browser action audit: {error}");
            }
        }
        accepted
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
        let _ = self.send(BrowserCommand::Stop);
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
    use std::time::Duration;

    use super::*;

    #[derive(Debug, Default)]
    struct RecordingCoordination {
        entries: Mutex<Vec<BrowserAuditEntry>>,
    }

    impl crate::BrowserCoordination for RecordingCoordination {
        fn prepare(&self, _panel_local_id: &str, _timeout: Duration) -> bool {
            true
        }

        fn initialize(&self, _panel_local_id: &str, _state: &crate::CoordinationState) -> std::io::Result<()> {
            Ok(())
        }

        fn update(&self, _panel_local_id: &str, _state: &crate::CoordinationState) -> std::io::Result<()> {
            Ok(())
        }

        fn set_user_active(&self, _panel_local_id: &str, _active: bool) -> std::io::Result<()> {
            Ok(())
        }

        fn signals(&self, _panel_local_id: &str) -> std::io::Result<crate::CoordinationSignals> {
            Ok(crate::CoordinationSignals::default())
        }

        fn acknowledge_handoff(&self, _panel_local_id: &str, _request_id: &str) -> std::io::Result<bool> {
            Ok(false)
        }

        fn record_action(&self, _panel_local_id: &str, entry: &BrowserAuditEntry) -> std::io::Result<()> {
            self.entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(entry.clone());
            Ok(())
        }

        fn remove(&self, _panel_local_id: &str, _timeout: Duration) -> bool {
            true
        }
    }

    fn session_for_audit(
        coordination: Arc<RecordingCoordination>,
    ) -> (BrowserSession, crate::session::CommandReceiver) {
        let frame_slot = Arc::new(FrameSlot::new());
        let (command_tx, command_rx) = crate::session::command_queue::channel(Arc::clone(&frame_slot));
        let (_completion_tx, completion_rx) = mpsc::channel();
        (
            BrowserSession {
                command_tx,
                stop_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                frame_slot,
                event_rx: mpsc::channel().1,
                completion_rx,
                event_wake: BrowserEventWake::default(),
                committed_url: CommittedUrl::default(),
                process_control: crate::process::ChromeProcessControl::default(),
                panel_local_id: "panel".to_string(),
                coordination: Some(coordination),
            },
            command_rx,
        )
    }

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

    #[test]
    fn stop_is_audited_even_when_the_atomic_flag_ends_the_driver_loop() {
        let coordination = Arc::new(RecordingCoordination::default());
        let (session, receiver) = session_for_audit(Arc::clone(&coordination));

        assert!(session.send(BrowserCommand::Stop));

        let batch = receiver.drain(1);
        assert_eq!(batch.commands.len(), 1);
        let entries = coordination
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].actor, BrowserAuditActor::System);
        assert_eq!(entries[0].status, BrowserAuditStatus::Dispatched);
        assert_eq!(entries[0].action, BrowserAuditAction::Stop);
    }
}
