//! The one worker thread of the dock: the start and the stop of a bridge, in the
//! order that the frames ask for them.
//!
//! A start opens a process, and a stop closes the standard input, waits and ends
//! the child by its process id when the child ignores both steps. Both can wait,
//! so neither runs on the thread of a frame. The worker is separate from the
//! coordination worker of [`super::super::browser_host_io`], because a stop that
//! waits leaves a browser job no room on that one.
//!
//! The jobs run in the order that the UI sends them, and each outcome reaches
//! the UI on a later frame, in the same order. A job that panics never ends the
//! worker or the jobs queued after it. The worker ends when the app drops it, or
//! when the exit closes it with [`Worker::flush`].

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// What the UI does with an outcome, on a later frame.
pub(super) type Apply = Box<dyn FnOnce(&mut super::OrchestratorDock) + Send>;
type Job = Box<dyn FnOnce() -> Option<Apply> + Send>;

/// The worker of the dock, and the outcomes that it sends back.
pub(super) struct Worker {
    jobs: Option<Sender<Job>>,
    /// Says that the worker ran every job and ended.
    drained: Option<Receiver<()>>,
    finish: Sender<Apply>,
    finished: Receiver<Apply>,
    /// The context that a finished job wakes, so its outcome applies without
    /// input.
    wake: std::sync::Arc<std::sync::OnceLock<egui::Context>>,
    /// Whether a test says that no thread of the dock starts.
    #[cfg(test)]
    without_thread: bool,
}

impl Default for Worker {
    fn default() -> Self {
        let (finish, finished) = channel();
        Self {
            jobs: None,
            drained: None,
            finish,
            finished,
            wake: std::sync::Arc::default(),
            #[cfg(test)]
            without_thread: false,
        }
    }
}

impl Worker {
    /// The context that a finished job wakes.
    pub(super) fn wake_with(&self, ctx: &egui::Context) {
        let _ = self.wake.get_or_init(|| ctx.clone());
    }

    /// Does `work` on the worker, after every job asked for before it, and
    /// answers `apply` on a later frame with its outcome, or with `None` when
    /// the work panicked, so the UI always learns that the work ended.
    ///
    /// The answer says whether a worker took the job. A job that no worker took
    /// runs nowhere, so the caller applies the failure itself rather than leave
    /// its state waiting for an answer that never comes.
    #[must_use]
    pub(super) fn then<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> T + Send + 'static,
        apply: impl FnOnce(&mut super::OrchestratorDock, Option<T>) + Send + 'static,
    ) -> bool {
        self.submit(Box::new(move || {
            let outcome = settle(work);
            Some(Box::new(move |dock: &mut super::OrchestratorDock| apply(dock, outcome)) as Apply)
        }))
    }

    /// Does `work` on the worker, after every job asked for before it. Nothing
    /// waits for it and it answers no outcome. The answer says whether a worker
    /// took the job.
    #[must_use]
    pub(super) fn write(&mut self, work: impl FnOnce() + Send + 'static) -> bool {
        self.submit(Box::new(move || {
            let _ = settle(work);
            None
        }))
    }

    /// Says that no thread of the dock starts, so a test reads what a job that no
    /// worker takes does.
    #[cfg(test)]
    pub(super) fn hold_no_worker(&mut self) {
        self.without_thread = true;
    }

    /// Lets a thread of the dock take the jobs of this worker again.
    #[cfg(test)]
    pub(super) fn take_jobs_again(&mut self) {
        self.without_thread = false;
    }

    /// Takes the outcomes of the jobs that finished, in their order. A frame
    /// applies them and never waits for one.
    pub(super) fn take_finished(&self) -> Vec<Apply> {
        self.finished.try_iter().collect()
    }

    /// Closes the worker and waits, for at most `wait`, until it ran every job
    /// and ended. The exit calls it within its own bound, and the caller holds
    /// an exception of the UI thread rule.
    pub(super) fn flush(&mut self, wait: Duration) {
        self.jobs = None;
        if let Some(drained) = self.drained.take() {
            let _ = drained.recv_timeout(wait);
        }
    }

    fn submit(&mut self, mut job: Job) -> bool {
        loop {
            if let Some(jobs) = self.worker() {
                match jobs.send(job) {
                    Ok(()) => return true,
                    Err(returned) => {
                        job = returned.0;
                        self.jobs = None;
                    }
                }
            } else {
                // No thread starts now, and a frame must not run the job: the
                // caller applies its failure, and the next job tries a worker
                // again.
                tracing::warn!("no worker takes a job of the orchestrator dock");
                return false;
            }
        }
    }

    /// The worker that takes the jobs of the dock, started when there is none.
    fn worker(&mut self) -> Option<Sender<Job>> {
        #[cfg(test)]
        if self.without_thread {
            return None;
        }
        if let Some(jobs) = &self.jobs {
            return Some(jobs.clone());
        }
        let (jobs, received) = channel::<Job>();
        let (done, drained) = channel();
        let finish = self.finish.clone();
        let wake = std::sync::Arc::clone(&self.wake);
        let started = std::thread::Builder::new()
            .name("orchestrator-dock".into())
            .spawn(move || {
                for job in received {
                    if let Some(apply) = job() {
                        let _ = finish.send(apply);
                    }
                    if let Some(ctx) = wake.get() {
                        ctx.request_repaint();
                    }
                }
                let _ = done.send(());
            });
        match started {
            Ok(_) => {
                self.drained = Some(drained);
                self.jobs = Some(jobs.clone());
                Some(jobs)
            }
            Err(error) => {
                tracing::warn!(%error, "the worker of the orchestrator dock does not start");
                None
            }
        }
    }
}

/// Runs `work`, keeping a panic inside it, so one failed job never ends the
/// worker or the jobs queued after it.
fn settle<T>(work: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .inspect_err(|_| tracing::error!("a job of the orchestrator dock panicked"))
        .ok()
}
