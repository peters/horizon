//! Deadline control for deterministic browser jobs.

use std::future::{Future, pending};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Why a deterministic plan stopped before reaching a terminal tool result.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStopReason {
    /// The deterministic job deadline elapsed.
    DeadlineExceeded,
}

impl ExecutionStopReason {
    /// Stable phase-neutral description of this stop condition.
    pub const MESSAGE: &'static str = "job deadline exceeded";
    /// Description used only after a browser action has been dispatched.
    pub const IN_FLIGHT_MESSAGE: &'static str = "job deadline exceeded; an in-flight browser action may still complete";
}

#[derive(Clone, Copy)]
struct JobDeadline {
    monotonic: tokio::time::Instant,
    unix_millis: u64,
}

/// One deadline shared across preparation, MCP execution, and shutdown.
pub struct ExecutionControl {
    deadline: Option<JobDeadline>,
}

impl ExecutionControl {
    pub(crate) const fn unbounded() -> Self {
        Self { deadline: None }
    }

    /// Start a job deadline from the current monotonic and wall clocks.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        let wall_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
        let now = tokio::time::Instant::now();
        let timeout_millis = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        Self {
            deadline: Some(JobDeadline {
                monotonic: now.checked_add(timeout).unwrap_or(now),
                unix_millis: wall_millis.saturating_add(timeout_millis),
            }),
        }
    }

    /// Absolute wall-clock form of the selected deadline for durable state.
    #[must_use]
    pub fn deadline_at_millis(&self) -> Option<u64> {
        self.deadline.map(|deadline| deadline.unix_millis)
    }

    /// Check the deadline before starting work with observable side effects.
    ///
    /// # Errors
    /// Returns [`ExecutionStopReason::DeadlineExceeded`] once the deadline has
    /// elapsed.
    pub fn check(&self) -> Result<(), ExecutionStopReason> {
        if self
            .deadline
            .is_some_and(|deadline| tokio::time::Instant::now() >= deadline.monotonic)
        {
            Err(ExecutionStopReason::DeadlineExceeded)
        } else {
            Ok(())
        }
    }

    /// Await work only while the shared job deadline remains available.
    ///
    /// # Errors
    /// Returns [`ExecutionStopReason::DeadlineExceeded`] when the deadline wins.
    pub async fn wait<T>(&mut self, future: impl Future<Output = T>) -> Result<T, ExecutionStopReason> {
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
