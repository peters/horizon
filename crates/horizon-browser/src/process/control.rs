//! Exact child ownership and bounded automation-service teardown.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use super::kill_and_reap;

/// Exact loopback automation-service process (`geckodriver` or
/// `safaridriver`). The service owns the browser session it creates; normal
/// teardown deletes that session before this child is terminated and reaped.
pub(crate) struct ServiceProcess {
    child: Arc<Mutex<Child>>,
    control: ChromeProcessControl,
    exit_status: Option<std::process::ExitStatus>,
    stderr_tail: Arc<Mutex<String>>,
    label: &'static str,
}

/// Exact child handle retained outside the driver so an application-exit
/// deadline can terminate and reap the browser even if its driver is stuck in
/// teardown. Retaining `Child`, rather than only its PID, avoids targeting a
/// later process that reused the number.
#[derive(Clone, Default)]
pub(crate) struct ChromeProcessControl {
    inner: Arc<Mutex<ChromeProcessControlState>>,
}

#[derive(Default)]
struct ChromeProcessControlState {
    child: Option<Arc<Mutex<Child>>>,
    force_requested: bool,
    registration_settled: bool,
}

impl ChromeProcessControl {
    pub(super) fn register(&self, child: &Arc<Mutex<Child>>) {
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

    pub(super) fn clear(&self, child: &Arc<Mutex<Child>>) {
        let mut state = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_registered_child(&mut state, child);
    }

    fn clear_until(&self, child: &Arc<Mutex<Child>>, deadline: Instant) -> bool {
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
            match child_guard.try_wait() {
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
        let mut child = command.spawn()?;
        let Some(stderr) = child.stderr.take() else {
            let _ = kill_and_reap(&mut child, Duration::from_secs(3));
            return Err(std::io::Error::other(format!("failed to capture {label} stderr")));
        };
        let tail = Arc::new(Mutex::new(String::new()));
        let reader_tail = Arc::clone(&tail);
        let thread_name = format!("{label}-stderr");
        if let Err(error) = std::thread::Builder::new().name(thread_name).spawn(move || {
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
        }) {
            let _ = kill_and_reap(&mut child, Duration::from_secs(3));
            return Err(error);
        }
        let child = Arc::new(Mutex::new(child));
        control.register(&child);
        Ok(Self {
            child,
            control,
            exit_status: None,
            stderr_tail: tail,
            label,
        })
    }

    #[must_use]
    pub(crate) fn child_status(&mut self) -> Option<std::process::ExitStatus> {
        if self.exit_status.is_some() {
            return self.exit_status;
        }
        let status = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .try_wait()
            .ok()
            .flatten();
        self.exit_status = status;
        status
    }

    #[must_use]
    pub(crate) fn kill(&mut self) -> bool {
        if self.child_status().is_some() {
            return true;
        }
        let mut child = self.child.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let pid = child.id();
        match kill_and_reap(&mut child, Duration::from_secs(3)) {
            Ok(Some(status)) => {
                self.exit_status = Some(status);
                self.control.clear(&self.child);
                true
            }
            Ok(None) => {
                tracing::warn!(
                    pid,
                    service = self.label,
                    "automation service did not exit before its deadline"
                );
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

    pub(crate) fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Drop for ServiceProcess {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

fn clear_registered_child(state: &mut ChromeProcessControlState, child: &Arc<Mutex<Child>>) {
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
