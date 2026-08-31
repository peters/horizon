//! Whole-job deadline control for deterministic plan execution.

use std::future::{Future, pending};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Why a deterministic plan stopped before reaching a terminal tool result.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStopReason {
    /// The whole-job deadline elapsed.
    DeadlineExceeded,
}

impl ExecutionStopReason {
    /// Stable phase-neutral description of this stop condition.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::DeadlineExceeded => "job deadline exceeded",
        }
    }

    /// Description used only after a browser action has been dispatched.
    #[must_use]
    pub const fn in_flight_message(self) -> &'static str {
        match self {
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
        let now = tokio::time::Instant::now();
        let monotonic = now.checked_add(timeout).unwrap_or(now);
        let wall_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
        let timeout_millis = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        Self {
            monotonic,
            unix_millis: wall_millis.saturating_add(timeout_millis),
        }
    }

    /// Absolute wall-clock form suitable for durable diagnostics.
    #[must_use]
    pub const fn unix_millis(self) -> u64 {
        self.unix_millis
    }

    /// Reject a synchronous handoff after the monotonic deadline.
    ///
    /// # Errors
    /// Returns [`ExecutionStopReason::DeadlineExceeded`] once time is exhausted.
    pub fn check(self) -> Result<(), ExecutionStopReason> {
        if tokio::time::Instant::now() >= self.monotonic {
            Err(ExecutionStopReason::DeadlineExceeded)
        } else {
            Ok(())
        }
    }
}

/// One deadline shared across input, durable setup, MCP work, and shutdown.
pub struct ExecutionControl {
    deadline: Option<JobDeadline>,
}

impl ExecutionControl {
    pub(crate) const fn unbounded() -> Self {
        Self { deadline: None }
    }

    /// Enforce an already-selected whole-job deadline.
    #[must_use]
    pub const fn until(deadline: JobDeadline) -> Self {
        Self {
            deadline: Some(deadline),
        }
    }

    /// Await work only while the whole-job deadline remains available.
    ///
    /// # Errors
    /// Returns [`ExecutionStopReason::DeadlineExceeded`] when the deadline wins.
    pub async fn wait<T>(&mut self, future: impl Future<Output = T>) -> Result<T, ExecutionStopReason> {
        if let Some(deadline) = self.deadline {
            deadline.check()?;
        }
        let deadline = self.deadline;
        tokio::select! {
            biased;
            () = deadline_reached(deadline) => Err(ExecutionStopReason::DeadlineExceeded),
            output = future => Ok(output),
        }
    }
}

async fn deadline_reached(deadline: Option<JobDeadline>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.monotonic).await,
        None => pending::<()>().await,
    }
}
