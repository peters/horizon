//! The coordination file work of the browser request pumps. Claims, audits,
//! manifest stamps and results take coordination locks and sync their files to
//! disk, so one worker thread does that work, in the order it was asked for. A
//! slow disk, or a lock that an MCP server or a driver holds, then never stalls
//! a frame. What the UI does with an outcome runs on a later frame, in the same
//! order.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use super::HorizonApp;

/// How long an exiting Horizon waits for the coordination work it already
/// asked for, such as the results of closes it settled: long enough for a
/// healthy disk, bounded so a stalled one cannot hold the exit.
const EXIT_FLUSH: Duration = Duration::from_secs(2);

/// What the UI does with an outcome.
type Then = Box<dyn FnOnce(&mut HorizonApp) + Send>;
type Job = Box<dyn FnOnce() -> Option<Then> + Send>;

pub(super) struct HostIo {
    jobs: Option<Sender<Job>>,
    /// Says that the worker ran every job and ended.
    drained: Option<Receiver<()>>,
    finish: Sender<Then>,
    finished: Receiver<Then>,
    /// Jobs asked for whose outcome is not yet queued for the UI.
    outstanding: Arc<AtomicUsize>,
    wake: Arc<OnceLock<egui::Context>>,
    /// Until when an exit waits for the jobs asked for.
    exit_deadline: Option<Instant>,
}

impl Default for HostIo {
    fn default() -> Self {
        let (finish, finished) = channel();
        Self {
            jobs: None,
            drained: None,
            finish,
            finished,
            outstanding: Arc::default(),
            wake: Arc::default(),
            exit_deadline: None,
        }
    }
}

impl Drop for HostIo {
    fn drop(&mut self) {
        self.jobs = None;
        if let Some(drained) = self.drained.take() {
            let _ = drained.recv_timeout(EXIT_FLUSH);
        }
    }
}

impl HostIo {
    /// The context a finished job wakes, so its outcome applies without input.
    pub(super) fn wake_with(&self, ctx: &egui::Context) {
        let _ = self.wake.get_or_init(|| ctx.clone());
    }

    /// Does `work` after every job asked for before it; nothing waits for it.
    pub(super) fn write(&mut self, work: impl FnOnce() + Send + 'static) {
        self.submit(Box::new(move || {
            let _ = settle(work);
            None
        }));
    }

    /// Does `work` after every job asked for before it, then `apply` on the UI
    /// thread with its outcome, or with `None` when `work` panicked, so the UI
    /// always learns that the work ended.
    pub(super) fn then<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> T + Send + 'static,
        apply: impl FnOnce(&mut HorizonApp, Option<T>) + Send + 'static,
    ) {
        self.submit(Box::new(move || {
            let outcome = settle(work);
            Some(Box::new(move |app: &mut HorizonApp| apply(app, outcome)) as Then)
        }));
    }

    /// Whether a job asked for has not reached the UI yet.
    pub(super) fn busy(&self) -> bool {
        self.outstanding.load(Ordering::Acquire) > 0
    }

    fn submit(&mut self, job: Job) {
        self.outstanding.fetch_add(1, Ordering::AcqRel);
        let job = match &self.jobs {
            Some(jobs) => match jobs.send(job) {
                Ok(()) => return,
                Err(returned) => returned.0,
            },
            None => job,
        };
        match self.start() {
            Ok((jobs, drained)) => {
                if let Err(returned) = jobs.send(job) {
                    self.run_here(returned.0);
                }
                self.jobs = Some(jobs);
                self.drained = Some(drained);
            }
            // Without a worker the job still runs, as it did before there was one.
            Err(error) => {
                tracing::warn!(%error, "could not start the browser coordination worker");
                self.run_here(job);
            }
        }
    }

    fn start(&self) -> std::io::Result<(Sender<Job>, Receiver<()>)> {
        let (jobs, received) = channel::<Job>();
        let (done, drained) = channel();
        let finish = self.finish.clone();
        let outstanding = Arc::clone(&self.outstanding);
        let wake = Arc::clone(&self.wake);
        std::thread::Builder::new()
            .name("browser-coordination".into())
            .spawn(move || {
                for job in received {
                    if let Some(then) = job() {
                        let _ = finish.send(then);
                    }
                    // Queued before it counts as done, so an idle worker has nothing left to apply.
                    outstanding.fetch_sub(1, Ordering::AcqRel);
                    if let Some(ctx) = wake.get() {
                        ctx.request_repaint();
                    }
                }
                let _ = done.send(());
            })?;
        Ok((jobs, drained))
    }

    fn run_here(&self, job: Job) {
        if let Some(then) = job() {
            let _ = self.finish.send(then);
        }
        self.outstanding.fetch_sub(1, Ordering::AcqRel);
    }

    fn take_finished(&self) -> Vec<Then> {
        self.finished.try_iter().collect()
    }
}

/// Runs `work`, keeping a panic inside it, so one failed job never ends the
/// worker or the jobs queued after it.
fn settle<T>(work: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .inspect_err(|_| tracing::error!("a browser coordination job panicked"))
        .ok()
}

impl HorizonApp {
    /// Applies, in order, the outcomes of the coordination jobs that finished.
    pub(super) fn apply_browser_host_io(&mut self) -> bool {
        let finished = self.browser_create_host.io.take_finished();
        let applied = !finished.is_empty();
        for then in finished {
            then(self);
        }
        applied
    }

    /// Whether an exit still waits for coordination work, which it does for
    /// at most [`EXIT_FLUSH`] from the first call. Each outcome that landed
    /// is applied first, so a claimed request is answered, and a refusal or
    /// result it queues is written, before Horizon exits.
    pub(super) fn browser_host_io_holds_exit(&mut self) -> bool {
        // Read before applying: a job that ends in between has queued its
        // outcome, which the next call applies.
        let busy = self.browser_create_host.io.busy();
        self.apply_browser_host_io();
        let io = &mut self.browser_create_host.io;
        if !busy && !io.busy() {
            io.exit_deadline = None;
            return false;
        }
        let deadline = *io.exit_deadline.get_or_insert_with(|| Instant::now() + EXIT_FLUSH);
        Instant::now() < deadline
    }

    /// Waits on this thread, for at most [`EXIT_FLUSH`], until every
    /// coordination job ran and its outcome applied. The exit that runs
    /// without further frames uses it.
    pub(super) fn flush_browser_host_io(&mut self) {
        while self.browser_host_io_holds_exit() {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Applies outcomes until no coordination job is left. Tests use it where
    /// the UI would apply them on later frames.
    #[cfg(test)]
    pub(super) fn settle_browser_host_io(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let busy = self.browser_create_host.io.busy();
            self.apply_browser_host_io();
            if !busy && !self.browser_create_host.io.busy() {
                self.apply_browser_host_io();
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "a coordination job did not finish"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_idle(io: &HostIo) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while io.busy() {
            assert!(std::time::Instant::now() < deadline, "the worker did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn jobs_run_in_order_and_their_outcomes_apply_in_that_order() {
        let mut io = HostIo::default();
        let (seen, order) = channel();
        for index in 0..20 {
            let seen = seen.clone();
            io.then(
                move || {
                    seen.send(index).unwrap();
                    index
                },
                |_, _| {},
            );
        }
        wait_idle(&io);
        assert_eq!(order.try_iter().collect::<Vec<_>>(), (0..20).collect::<Vec<_>>());
        assert_eq!(io.take_finished().len(), 20);
    }

    #[test]
    fn a_job_that_does_not_return_never_holds_the_caller() {
        let mut io = HostIo::default();
        let (release, released) = channel::<()>();
        let started = std::time::Instant::now();
        io.write(move || {
            let _ = released.recv();
        });
        io.write(|| {});
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(io.busy());
        release.send(()).unwrap();
        wait_idle(&io);
    }

    #[test]
    fn a_job_that_panics_still_reports_and_the_worker_goes_on() {
        let mut io = HostIo::default();
        io.then(|| -> u8 { panic!("a fixture job fails") }, |_, _| {});
        io.then(|| 7_u8, |_, _| {});
        wait_idle(&io);
        assert_eq!(io.take_finished().len(), 2, "both outcomes reach the UI");
    }

    #[test]
    fn an_exit_waits_for_queued_work_and_what_its_outcomes_queue_within_a_bound() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let (release, released) = channel::<()>();
        let (seen, refused) = channel();
        app.browser_create_host.io.then(
            move || {
                let _ = released.recv();
            },
            move |app: &mut HorizonApp, _| {
                // An outcome that answers a claimed request queues a write.
                app.browser_create_host.io.write(move || seen.send(()).unwrap());
            },
        );
        assert!(app.browser_host_io_holds_exit(), "the claim still runs");
        release.send(()).unwrap();
        app.flush_browser_host_io();
        assert!(refused.try_recv().is_ok(), "the refusal was written before the exit");

        let (_hold, held) = channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = held.recv();
        });
        app.browser_create_host.io.exit_deadline = Some(Instant::now());
        assert!(
            !app.browser_host_io_holds_exit(),
            "a job that never ends holds the exit only until the bound"
        );
    }

    #[test]
    fn dropping_waits_for_the_work_already_asked_for() {
        let mut io = HostIo::default();
        let (seen, written) = channel();
        io.write(move || {
            std::thread::sleep(Duration::from_millis(50));
            seen.send(()).unwrap();
        });
        drop(io);
        assert!(written.try_recv().is_ok(), "the write ran before the drop returned");
    }
}
