//! Exact child ownership and bounded automation-service teardown.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const STDERR_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

#[cfg(unix)]
use super::child_id;
use super::{ProcessChild, kill_and_reap, poll_and_cleanup_exited_tree, spawn_process, take_stderr};

/// Exact loopback automation-service process (`geckodriver` or
/// `safaridriver`). The service owns the browser session it creates; normal
/// teardown deletes that session before this child is terminated and reaped.
pub(crate) struct ServiceProcess {
    child: Arc<Mutex<ProcessChild>>,
    control: ChromeProcessControl,
    /// The child that ends the driver and the browser that it started when this
    /// process is gone, whatever ended it.
    #[cfg(unix)]
    watch: Option<super::watch::BrowserWatch>,
    exit_status: Option<std::process::ExitStatus>,
    stderr_tail: Arc<Mutex<String>>,
    stderr_reader: Option<JoinHandle<()>>,
    stderr_done: Option<Receiver<()>>,
    stderr_complete: bool,
    label: &'static str,
}

/// Exact process-tree handle retained outside the driver so an application-exit
/// deadline can terminate and reap the browser even if its driver is stuck in
/// teardown. Retaining the native child, rather than only its PID, avoids
/// targeting a later process that reused the number.
pub(crate) trait ProcessLifecycle: Send + Sync {
    fn is_reaped(&self) -> bool;
    fn terminate(&self, timeout: Duration) -> bool;
}

#[derive(Clone, Default)]
pub(crate) struct ChromeProcessControl {
    inner: Arc<Mutex<ChromeProcessControlState>>,
}

#[derive(Default)]
struct ChromeProcessControlState {
    child: Option<Arc<Mutex<ProcessChild>>>,
    delegate: Option<Arc<dyn ProcessLifecycle>>,
    force_requested: bool,
    registration_settled: bool,
}

impl ChromeProcessControl {
    /// Returns whether the caller must forward a pending force request after
    /// releasing any lifecycle lock it holds.
    #[must_use]
    pub(crate) fn delegate(&self, lifecycle: Arc<dyn ProcessLifecycle>) -> bool {
        let mut state = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.delegate = Some(lifecycle);
        state.force_requested
    }

    pub(super) fn register(&self, child: &Arc<Mutex<ProcessChild>>) {
        let force_requested = {
            let mut state = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            state.child = Some(Arc::clone(child));
            state.registration_settled = true;
            state.force_requested
        };
        if force_requested {
            let _ = self.terminate(Duration::from_secs(3));
        }
    }

    pub(crate) fn mark_registration_settled(&self) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .registration_settled = true;
    }

    pub(super) fn clear(&self, child: &Arc<Mutex<ProcessChild>>) {
        let mut state = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_registered_child(&mut state, child);
    }

    fn clear_until(&self, child: &Arc<Mutex<ProcessChild>>, deadline: Instant) -> bool {
        let Some(mut state) = lock_until(&self.inner, deadline) else {
            return false;
        };
        clear_registered_child(&mut state, child);
        true
    }

    /// Whether no registered child remains alive. A driver completion signal
    /// is not sufficient: its final kill/reap can fail while this retained
    /// exact handle still owns a live browser process.
    pub(crate) fn is_reaped(&self) -> bool {
        let child = {
            let state = match self.inner.try_lock() {
                Ok(state) => state,
                Err(TryLockError::Poisoned(error)) => error.into_inner(),
                Err(TryLockError::WouldBlock) => return false,
            };
            if let Some(delegate) = state.delegate.clone() {
                drop(state);
                return delegate.is_reaped();
            }
            state.child.clone()
        };
        let Some(child) = child else {
            return true;
        };
        let reaped = {
            let mut child_guard = match child.try_lock() {
                Ok(child) => child,
                Err(TryLockError::Poisoned(error)) => error.into_inner(),
                Err(TryLockError::WouldBlock) => return false,
            };
            match poll_and_cleanup_exited_tree(&mut child_guard) {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(error) => {
                    tracing::warn!(pid = child_guard.id(), "failed to poll browser child: {error}");
                    false
                }
            }
        };
        if reaped {
            self.clear(&child);
        }
        reaped
    }

    /// Force the exact registered browser child to exit and reap it within
    /// the supplied deadline. A force requested just before spawn is
    /// remembered; registration immediately terminates that child.
    pub(crate) fn terminate(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let child = loop {
            let Some(mut state) = lock_until(&self.inner, deadline) else {
                tracing::warn!("timed out waiting for browser process-control state");
                return false;
            };
            state.force_requested = true;
            if let Some(delegate) = state.delegate.clone() {
                drop(state);
                return delegate.terminate(deadline.saturating_duration_since(Instant::now()));
            }
            if let Some(child) = state.child.clone() {
                break child;
            }
            if state.registration_settled {
                return true;
            }
            drop(state);
            if Instant::now() >= deadline {
                tracing::warn!("browser process registration did not settle before the shutdown deadline");
                return false;
            }
            std::thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(10)),
            );
        };
        let Some(mut child_guard) = lock_until(&child, deadline) else {
            tracing::warn!("timed out waiting for the exact browser child handle");
            return false;
        };
        let pid = child_guard.id();
        let reaped = match kill_and_reap(&mut child_guard, deadline.saturating_duration_since(Instant::now())) {
            Ok(Some(_)) => true,
            Ok(None) => {
                tracing::warn!(pid, "browser did not exit within the forced shutdown deadline");
                false
            }
            Err(error) => {
                tracing::warn!(pid, "failed to force-terminate browser: {error}");
                false
            }
        };
        drop(child_guard);
        if reaped && !self.clear_until(&child, deadline) {
            tracing::warn!(pid, "timed out clearing the reaped browser child handle");
            return false;
        }
        reaped
    }
}

impl ServiceProcess {
    pub(crate) fn spawn(
        command: &Path,
        args: &[String],
        control: ChromeProcessControl,
        label: &'static str,
    ) -> std::io::Result<Self> {
        let mut command = Command::new(command);
        command.args(args).stdout(Stdio::null()).stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = spawn_process(command)?;
        let Some(stderr) = take_stderr(&mut child) else {
            let _ = kill_and_reap(&mut child, Duration::from_secs(3));
            return Err(std::io::Error::other(format!("failed to capture {label} stderr")));
        };
        let tail = Arc::new(Mutex::new(String::new()));
        let reader_tail = Arc::clone(&tail);
        let (done_tx, done_rx) = mpsc::channel();
        let thread_name = format!("{label}-stderr");
        let reader = match std::thread::Builder::new().name(thread_name).spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else {
                    break;
                };
                let mut tail = reader_tail.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                tail.push_str(&line);
                tail.push('\n');
                if tail.len() > 8 * 1024 {
                    let cut = tail.floor_char_boundary(4 * 1024);
                    tail.drain(..cut);
                }
            }
            // Send only after the mutex write. Classification waits for this.
            let _ = done_tx.send(());
        }) {
            Ok(reader) => reader,
            Err(error) => {
                let _ = kill_and_reap(&mut child, Duration::from_secs(3));
                return Err(error);
            }
        };
        let child = Arc::new(Mutex::new(child));
        control.register(&child);
        #[cfg(unix)]
        let watch = super::start_watch(child_id(&child));
        Ok(Self {
            child,
            control,
            #[cfg(unix)]
            watch,
            exit_status: None,
            stderr_tail: tail,
            stderr_reader: Some(reader),
            stderr_done: Some(done_rx),
            stderr_complete: false,
            label,
        })
    }

    /// Wait until the reader has stored every line it could read, then join it.
    /// A pipe that stays open does not finish this wait: the reader is detached
    /// and [`Self::stderr_is_complete`] stays false so startup does not treat a
    /// partial tail as proof that `--allow-system-access` was accepted.
    fn drain_stderr_reader(&mut self) {
        let Some(handle) = self.stderr_reader.take() else {
            return;
        };
        let finished = self
            .stderr_done
            .take()
            .is_some_and(|done| done.recv_timeout(STDERR_DRAIN_TIMEOUT).is_ok());
        if finished {
            let _ = handle.join();
            self.stderr_complete = true;
        }
    }

    #[must_use]
    pub(crate) fn child_status(&mut self) -> Option<std::process::ExitStatus> {
        if self.exit_status.is_some() {
            return self.exit_status;
        }
        let status = {
            let mut child = self.child.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            poll_and_cleanup_exited_tree(&mut child).ok().flatten()
        };
        if status.is_some() {
            self.exit_status = status;
            self.drain_stderr_reader();
        }
        self.exit_status
    }

    #[must_use]
    pub(crate) fn kill(&mut self) -> bool {
        if self.child_status().is_some() {
            self.control.clear(&self.child);
            return true;
        }
        let mut child = self.child.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let pid = child.id();
        match kill_and_reap(&mut child, Duration::from_secs(3)) {
            Ok(Some(status)) => {
                drop(child);
                self.exit_status = Some(status);
                self.stop_watch();
                self.drain_stderr_reader();
                self.control.clear(&self.child);
                true
            }
            Ok(None) => {
                tracing::warn!(
                    pid,
                    service = self.label,
                    "automation service did not exit before its deadline"
                );
                drop(child);
                // The driver may still run. Its watch is the only thing that ends
                // it when this process dies, so the watch outlives this value
                // instead of being stopped with it.
                self.abandon_watch();
                false
            }
            Err(error) => {
                tracing::warn!(
                    pid,
                    service = self.label,
                    "failed to terminate automation service: {error}"
                );
                false
            }
        }
    }

    /// Ends the watch of this driver and of the browser that it started. The
    /// driver is gone or going, so the watch has nothing left to do.
    fn stop_watch(&mut self) {
        #[cfg(unix)]
        if let Some(mut watch) = self.watch.take() {
            watch.stop();
        }
    }

    /// Keeps the watch of a driver that the caller could not end, so the watch
    /// still ends that driver when this process dies.
    fn abandon_watch(&mut self) {
        #[cfg(unix)]
        if let Some(watch) = self.watch.take() {
            super::watch::BrowserWatch::abandon(watch);
        }
    }

    /// Lets the watch act as it does when this process dies. A test uses it to be
    /// that death without killing itself.
    #[cfg(all(test, unix))]
    pub(crate) fn release_watch(&mut self) {
        if let Some(watch) = self.watch.take() {
            watch.release();
        }
    }

    pub(crate) fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub(crate) fn stderr_is_complete(&self) -> bool {
        self.stderr_complete
    }
}

impl Drop for ServiceProcess {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

fn clear_registered_child(state: &mut ChromeProcessControlState, child: &Arc<Mutex<ProcessChild>>) {
    if state
        .child
        .as_ref()
        .is_some_and(|registered| Arc::ptr_eq(registered, child))
    {
        state.child = None;
    }
}

fn lock_until<T>(mutex: &Mutex<T>, deadline: Instant) -> Option<MutexGuard<'_, T>> {
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(error)) => return Some(error.into_inner()),
            Err(TryLockError::WouldBlock) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return None;
                }
                std::thread::sleep(remaining.min(Duration::from_millis(10)));
                if Instant::now() >= deadline {
                    return None;
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn exited_service_leader_does_not_leave_its_browser_process_group_alive() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("create temp dir: {error}"));
        let pid_file = root.path().join("descendant.pid");
        let args = vec![
            "-c".to_string(),
            "trap '' HUP; sleep 30 & printf '%s\\n' \"$!\" > \"$1.tmp\" && mv \"$1.tmp\" \"$1\"".to_string(),
            "horizon-service-process-test".to_string(),
            pid_file.display().to_string(),
        ];
        let mut process = ServiceProcess::spawn(
            Path::new("/bin/sh"),
            &args,
            ChromeProcessControl::default(),
            "test-driver",
        )
        .unwrap_or_else(|error| panic!("spawn service test process: {error}"));

        let deadline = Instant::now() + Duration::from_secs(2);
        while !pid_file.is_file() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let descendant_pid = std::fs::read_to_string(&pid_file)
            .unwrap_or_else(|error| panic!("read descendant pid: {error}"))
            .trim()
            .to_string();
        assert!(process_exists(&descendant_pid));

        while process.child_status().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(process.child_status().is_some());
        while process_exists(&descendant_pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!process_exists(&descendant_pid));
    }

    /// A driver that this process starts has a watch, and the watch ends the
    /// driver and the browser in its process group when this process dies without
    /// a cleanup: the kernel closes the pipe of the watch for a `SIGTERM`, a
    /// `SIGINT` and a `SIGKILL` of Horizon alike.
    #[cfg(unix)]
    #[test]
    fn a_driver_of_a_panel_ends_when_the_horizon_that_started_it_is_gone() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("create temp dir: {error}"));
        let pid_file = root.path().join("browser.pid");
        let args = vec![
            "-c".to_string(),
            "trap '' TERM; /bin/sleep 30 & printf '%s\\n' \"$!\" > \"$1\".tmp && \
             mv \"$1\".tmp \"$1\"; wait"
                .to_string(),
            "horizon-service-watch-test".to_string(),
            pid_file.display().to_string(),
        ];
        let mut process = ServiceProcess::spawn(
            Path::new("/bin/sh"),
            &args,
            ChromeProcessControl::default(),
            "test-driver",
        )
        .unwrap_or_else(|error| panic!("spawn service test process: {error}"));

        let deadline = Instant::now() + Duration::from_secs(5);
        while !pid_file.is_file() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let browser_pid = std::fs::read_to_string(&pid_file)
            .unwrap_or_else(|error| panic!("read browser pid: {error}"))
            .trim()
            .to_string();
        assert!(process_exists(&browser_pid));

        process.release_watch();
        while process_exists(&browser_pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !process_exists(&browser_pid),
            "the browser of a dead Horizon is gone with the driver"
        );
        let _ = process.kill();
    }

    #[test]
    fn exited_service_stderr_includes_the_final_line() {
        let message = "Found argument '--allow-system-access' which wasn't expected, or isn't valid in this context";
        let args = vec![
            "-c".to_string(),
            "printf '%s\\n' \"$1\" >&2; exit 64".to_string(),
            "horizon-stderr-test".to_string(),
            message.to_string(),
        ];
        let mut process = ServiceProcess::spawn(
            Path::new("/bin/sh"),
            &args,
            ChromeProcessControl::default(),
            "test-driver",
        )
        .unwrap_or_else(|error| panic!("spawn service test process: {error}"));
        let deadline = Instant::now() + Duration::from_secs(2);
        while process.child_status().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(process.child_status().is_some(), "service test process did not exit");
        assert!(
            process.stderr_is_complete(),
            "stderr reader did not finish before classification"
        );
        let stderr = process.stderr_tail();
        assert!(
            stderr.contains(message),
            "stderr after exit did not include the final line: {stderr}"
        );
    }

    /// The stand-in Horizon of [`a_horizon_that_a_signal_ends_leaves_no_driver_behind`]:
    /// it starts the real driver service with a stand-in driver program, and it
    /// waits to be ended by a signal. It runs as a child of the test, so the test
    /// can end it with `SIGTERM` and with `SIGKILL` the way a person ends Horizon.
    #[cfg(unix)]
    #[test]
    fn the_stand_in_horizon_of_the_signal_test() {
        let Some(root) = std::env::var_os("HORIZON_DRIVER_SIGNAL_STAND_IN") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let pid_file = root.join("driver.pid");
        let script = format!(
            "trap '' TERM\nprintf '%s' \"$$\" > {}\nwhile :; do sleep 1; done\n",
            pid_file.display()
        );
        let args = vec!["-c".to_string(), script, "stand-in-driver".to_string()];
        let _process = ServiceProcess::spawn(
            Path::new("/bin/sh"),
            &args,
            ChromeProcessControl::default(),
            "stand-in-driver",
        )
        .unwrap_or_else(|error| panic!("spawn the stand-in driver: {error}"));
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A Horizon that the system ends with a signal runs no exit cleanup, so the
    /// watch of a driver is the only thing that ends it. The stand-in Horizon is a
    /// real child of this test, it starts the real driver service, and its driver
    /// ignores `SIGTERM`, so only the watch of the driver can end it.
    #[cfg(unix)]
    #[test]
    fn a_horizon_that_a_signal_ends_leaves_no_driver_behind() {
        for signal in ["TERM", "KILL"] {
            let root = tempfile::tempdir().unwrap_or_else(|error| panic!("create temp dir: {error}"));
            let executable = std::env::current_exe().expect("the test binary");
            let mut horizon = Command::new(executable)
                .args([
                    "--exact",
                    "process::control::tests::the_stand_in_horizon_of_the_signal_test",
                    "--test-threads",
                    "1",
                ])
                .env("HORIZON_DRIVER_SIGNAL_STAND_IN", root.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap_or_else(|error| panic!("start the stand-in Horizon: {error}"));
            let driver = wait_for_pid(&root.path().join("driver.pid"));
            assert!(process_exists(&driver), "the stand-in driver runs");

            let status = Command::new("/bin/kill")
                .args(["-".to_string() + signal, horizon.id().to_string()])
                .status()
                .unwrap_or_else(|error| panic!("end the stand-in Horizon: {error}"));
            assert!(status.success(), "the signal reaches the stand-in Horizon");
            let _ = horizon.wait();

            assert!(
                gone_within(&driver, Duration::from_secs(8)),
                "the driver of a Horizon that ended with SIG{signal} stays: {driver}"
            );
        }
    }

    #[cfg(unix)]
    fn wait_for_pid(path: &Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(text) = std::fs::read_to_string(path)
                && !text.trim().is_empty()
            {
                return text.trim().to_owned();
            }
            assert!(Instant::now() < deadline, "the stand-in driver wrote its process id");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Whether `pid` is gone inside `bound`. The bound is a guard for a watch that
    /// does not act at all, not a speed of the machine: the watch sends `SIGTERM`
    /// and the surviving group gets `SIGKILL` a second later.
    #[cfg(unix)]
    fn gone_within(pid: &str, bound: Duration) -> bool {
        let deadline = Instant::now() + bound;
        while Instant::now() < deadline {
            if !process_exists(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[cfg(unix)]
    fn process_exists(pid: &str) -> bool {
        Command::new("/bin/kill")
            .args(["-0", "--", pid])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}
