//! Serialize cancellation with the platform handler's process-spawn boundary.
use super::{Error, Result};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct State {
    cancelled: crate::cloud_runtime::Cancellation,
    dispatch: Mutex<()>,
}

/// Cancellation shared by the sign-in card and its browser-opening worker.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<State>);

impl Cancellation {
    /// Cancels the attempt after any in-progress browser dispatch returns.
    pub fn cancel(&self) {
        let _dispatch = self
            .0
            .dispatch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.0.cancelled.cancel();
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.is_cancelled()
    }

    pub(super) fn check(&self) -> Result<()> {
        self.0.cancelled.check().map_err(|_| Error::Declined)
    }

    /// The opener must dispatch the platform handler, without waiting for browser completion.
    pub(super) fn dispatch(&self, open: impl FnOnce() -> std::io::Result<()>) -> Result<()> {
        let _dispatch = self
            .0
            .dispatch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.check()?;
        open().map_err(Error::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc::channel, time::Duration};

    #[test]
    fn completed_cancellation_prevents_browser_dispatch() {
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            cancel.dispatch(|| panic!("cancelled browser dispatch")),
            Err(Error::Declined)
        ));
    }

    #[test]
    fn cancellation_joins_an_in_progress_dispatch_before_it_completes() {
        let cancel = Cancellation::default();
        let (opened, opening) = channel();
        let (release, released) = channel();
        let dispatch = cancel.clone();
        let worker = std::thread::spawn(move || {
            dispatch.dispatch(|| {
                opened.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(())
            })
        });
        opening.recv_timeout(Duration::from_secs(5)).unwrap();
        let (done, completed) = channel();
        let cancellation = cancel.clone();
        let cancelling = std::thread::spawn(move || {
            cancellation.cancel();
            done.send(()).unwrap();
        });
        assert!(completed.recv_timeout(Duration::from_millis(50)).is_err());
        release.send(()).unwrap();
        worker.join().unwrap().unwrap();
        completed.recv_timeout(Duration::from_secs(5)).unwrap();
        cancelling.join().unwrap();
        assert!(cancel.is_cancelled());
        assert!(matches!(
            cancel.dispatch(|| panic!("late browser dispatch")),
            Err(Error::Declined)
        ));
    }
}
