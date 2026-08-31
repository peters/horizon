//! Deadline and cancellation control for deterministic browser jobs.

use std::future::{Future, pending};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

/// Why a deterministic plan stopped before reaching a terminal tool result.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStopReason {
    /// The runner received an explicit interrupt request.
    Cancelled,
    /// The deterministic job deadline elapsed.
    DeadlineExceeded,
}

impl ExecutionStopReason {
    /// Stable phase-neutral description of this stop condition.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Cancelled => "job cancelled by interrupt",
            Self::DeadlineExceeded => "job deadline exceeded",
        }
    }

    /// Description used only after a browser action has been dispatched.
    #[must_use]
    pub const fn in_flight_message(self) -> &'static str {
        match self {
            Self::Cancelled => "job cancelled by interrupt; an in-flight browser action may still complete",
            Self::DeadlineExceeded => "job deadline exceeded; an in-flight browser action may still complete",
        }
    }
}

/// One deadline represented for monotonic enforcement and durable diagnostics.
#[derive(Clone, Copy, Debug)]
pub struct JobDeadline {
    monotonic: tokio::time::Instant,
    unix_millis: u64,
}

impl JobDeadline {
    /// Start a deadline from the current monotonic and wall clocks.
    #[must_use]
    pub fn after(timeout: Duration) -> Self {
        let wall_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
        let now = tokio::time::Instant::now();
        let timeout_millis = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        Self {
            monotonic: now.checked_add(timeout).unwrap_or(now),
            unix_millis: wall_millis.saturating_add(timeout_millis),
        }
    }

    /// Absolute wall-clock form suitable for durable diagnostics.
    #[must_use]
    pub const fn unix_millis(self) -> u64 {
        self.unix_millis
    }
}

/// Idempotent cancellation sender retained by the CLI signal listener.
#[derive(Clone, Debug)]
pub struct CancellationHandle {
    sender: watch::Sender<bool>,
    cancelled: Arc<AtomicBool>,
}

impl CancellationHandle {
    /// Request cooperative cancellation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.sender.send_replace(true);
    }

    /// Create an independently awaitable cancellation observer.
    #[must_use]
    pub fn probe(&self) -> CancellationProbe {
        CancellationProbe {
            receiver: self.sender.subscribe(),
            cancelled: Arc::clone(&self.cancelled),
        }
    }
}

/// Cloneable cancellation state for detached preparation and finalization work.
#[derive(Clone, Debug)]
pub struct CancellationProbe {
    receiver: watch::Receiver<bool>,
    cancelled: Arc<AtomicBool>,
}

impl CancellationProbe {
    /// Return whether an interrupt has been observed.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Wait until cancellation is requested.
    pub async fn wait(&mut self) {
        cancellation_requested(&mut self.receiver).await;
    }
}

/// One deadline shared across preparation, MCP execution, and shutdown.
pub struct ExecutionControl {
    deadline: Option<JobDeadline>,
    cancellation: watch::Receiver<bool>,
}

impl ExecutionControl {
    pub(crate) fn unbounded() -> Self {
        Self::cancellable().0
    }

    /// Start cancellation observation without arming the execution deadline.
    #[must_use]
    pub fn cancellable() -> (Self, CancellationHandle) {
        let (sender, cancellation) = watch::channel(false);
        let cancelled = Arc::new(AtomicBool::new(false));
        (
            Self {
                deadline: None,
                cancellation,
            },
            CancellationHandle { sender, cancelled },
        )
    }

    /// Start a job deadline from the current monotonic and wall clocks.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        let (mut control, _cancellation) = Self::cancellable();
        let _deadline = control.start_timeout(timeout);
        control
    }

    /// Enforce an already-selected deadline and return its cancellation sender.
    #[must_use]
    pub fn until(deadline: JobDeadline) -> (Self, CancellationHandle) {
        let (mut control, cancellation) = Self::cancellable();
        control.deadline = Some(deadline);
        (control, cancellation)
    }

    /// Arm the execution deadline after plan input and validation complete.
    #[must_use]
    pub fn start_timeout(&mut self, timeout: Duration) -> JobDeadline {
        let deadline = JobDeadline::after(timeout);
        self.deadline = Some(deadline);
        deadline
    }

    /// Absolute wall-clock form of the selected deadline for durable state.
    #[must_use]
    pub fn deadline_at_millis(&self) -> Option<u64> {
        self.deadline.map(|deadline| deadline.unix_millis)
    }

    /// Check cancellation and the deadline before observable side effects.
    ///
    /// # Errors
    /// Returns the highest-priority pending stop reason.
    pub fn check(&self) -> Result<(), ExecutionStopReason> {
        if *self.cancellation.borrow() {
            Err(ExecutionStopReason::Cancelled)
        } else if self
            .deadline
            .is_some_and(|deadline| tokio::time::Instant::now() >= deadline.monotonic)
        {
            Err(ExecutionStopReason::DeadlineExceeded)
        } else {
            Ok(())
        }
    }

    /// Await work only while the job remains active and within its deadline.
    ///
    /// # Errors
    /// Returns the highest-priority cancellation or deadline stop reason.
    pub async fn wait<T>(&mut self, future: impl Future<Output = T>) -> Result<T, ExecutionStopReason> {
        self.check()?;
        let deadline = self.deadline;
        tokio::select! {
            biased;
            () = cancellation_requested(&mut self.cancellation) => Err(ExecutionStopReason::Cancelled),
            () = deadline_reached(deadline) => Err(ExecutionStopReason::DeadlineExceeded),
            output = future => Ok(output),
        }
    }
}

async fn cancellation_requested(receiver: &mut watch::Receiver<bool>) {
    loop {
        if *receiver.borrow_and_update() {
            return;
        }
        if receiver.changed().await.is_err() {
            pending::<()>().await;
        }
    }
}

async fn deadline_reached(deadline: Option<JobDeadline>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.monotonic).await,
        None => pending::<()>().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn repeated_cancellation_is_idempotent_and_wins_a_ready_deadline() {
        let deadline = JobDeadline::after(Duration::ZERO);
        let (mut control, cancellation) = ExecutionControl::until(deadline);
        cancellation.cancel();
        cancellation.cancel();

        assert_eq!(control.wait(pending::<()>()).await, Err(ExecutionStopReason::Cancelled));
    }

    #[test]
    fn cancellable_control_arms_its_deadline_later() {
        let (mut control, _cancellation) = ExecutionControl::cancellable();
        assert_eq!(control.deadline_at_millis(), None);

        let deadline = control.start_timeout(Duration::from_secs(1));

        assert_eq!(control.deadline_at_millis(), Some(deadline.unix_millis()));
    }
}
