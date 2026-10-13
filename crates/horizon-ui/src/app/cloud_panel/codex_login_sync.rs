//! Keeps the `ChatGPT` login of Codex current while Horizon runs: every few minutes, off the
//! UI thread, it takes over any renewal a worker made on its own, renews the login before
//! the workers would, and gives the renewal to every cloud. See
//! [`horizon_core::cloud_runtime::codex_login::sync`].
use super::HorizonApp;
use horizon_core::cloud_runtime::{
    Cancellation,
    codex_login::{self, Delivered, Renewal},
};
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError, channel},
    time::{Duration, Instant},
};

/// How often the login is checked. Renewal starts with hours left, so this is ample, and
/// a cloud that already has the current login is asked again only every half hour.
const EVERY: Duration = Duration::from_mins(5);

#[derive(Default)]
pub(super) struct State {
    delivered: Delivered,
    /// A sync under way; it hands the deliveries back when it ends.
    job: Option<Receiver<Delivered>>,
    next: Option<Instant>,
}

impl State {
    /// Starts a sync when one is due and none runs. Returns when to look again.
    fn step(
        &mut self,
        root: Option<PathBuf>,
        now: Instant,
        start: impl FnOnce(PathBuf, Delivered) -> Receiver<Delivered>,
    ) -> Option<Duration> {
        if let Some(job) = &self.job {
            match job.try_recv() {
                Ok(delivered) => {
                    self.delivered = delivered;
                    self.job = None;
                }
                // The job wakes the UI when it ends.
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => self.job = None,
            }
        }
        let root = root?;
        if let Some(at) = self.next.filter(|at| now < *at) {
            return Some(at.saturating_duration_since(now));
        }
        self.next = Some(now + EVERY);
        self.job = Some(start(root, std::mem::take(&mut self.delivered)));
        Some(EVERY)
    }
}

impl HorizonApp {
    pub(super) fn keep_codex_login_current(&mut self, ctx: &egui::Context) {
        // UI tests resolve the developer's real Horizon home; they must never reach a worker.
        if cfg!(test) {
            return;
        }
        let root = self.cloud_prototype.root.clone();
        let wake = self
            .cloud_prototype
            .codex_login
            .step(root, Instant::now(), |root, delivered| {
                let (sender, receiver) = channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = sender.send(sync(&root, delivered));
                    ctx.request_repaint();
                });
                receiver
            });
        if let Some(wake) = wake {
            ctx.request_repaint_after(wake);
        }
    }
}

fn sync(root: &std::path::Path, mut delivered: Delivered) -> Delivered {
    match codex_login::sync(root, &mut delivered, &Cancellation::default()) {
        Ok(Renewal::SignInAgain) => {
            tracing::warn!("OpenAI refused the ChatGPT login for Codex; sign in again in Cloud settings");
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "could not keep the ChatGPT login for Codex current"),
    }
    delivered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sync_starts_when_due_and_never_twice_at_once() {
        let mut state = State::default();
        let root = Some(PathBuf::from("/synthetic/cloud"));
        let start = Instant::now();
        let started = std::cell::Cell::new(0);
        let (sender, receiver) = channel();
        let mut receiver = Some(receiver);
        let mut begin = |_: PathBuf, delivered: Delivered| {
            started.set(started.get() + 1);
            assert!(delivered.is_empty());
            receiver.take().unwrap_or_else(|| channel().1)
        };
        assert_eq!(state.step(root.clone(), start, &mut begin), Some(EVERY));
        // While it runs, no other starts, and the job itself wakes the UI.
        assert_eq!(state.step(root.clone(), start + EVERY * 2, &mut begin), None);
        let mut back = Delivered::new();
        back.insert("synthetic-cloud".into(), ("synthetic".into(), start));
        sender.send(back).unwrap();
        // Ended: the next is due only after the interval.
        let wait = state.step(root.clone(), start + Duration::from_secs(1), &mut begin);
        assert_eq!(wait, Some(EVERY.saturating_sub(Duration::from_secs(1))));
        assert_eq!(started.get(), 1);
        assert!(state.delivered.contains_key("synthetic-cloud"));
        // Without a cloud root nothing starts.
        let mut fresh = State::default();
        assert_eq!(fresh.step(None, start, &mut begin), None);
        assert_eq!(started.get(), 1);
    }
}
