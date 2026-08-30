//! Deadline control for deterministic plan execution.

use std::future::{Future, pending};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Why a deterministic plan stopped before reaching a terminal tool result.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStopReason {
    /// The MCP execution deadline elapsed.
    DeadlineExceeded,
}

impl ExecutionStopReason {
    /// Stable operator-facing description of this stop condition.
    pub const MESSAGE: &'static str = "execution deadline exceeded; an in-flight browser action may still complete";
}

/// One deadline shared across MCP initialization, tool calls, and shutdown.
pub struct ExecutionControl {
    deadline: Option<tokio::time::Instant>,
}

impl ExecutionControl {
    pub(crate) const fn unbounded() -> Self {
        Self { deadline: None }
    }

    /// Start an execution deadline from the current monotonic clock.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        let now = tokio::time::Instant::now();
        Self {
            deadline: Some(now.checked_add(timeout).unwrap_or(now)),
        }
    }

    /// Await MCP work only while the execution deadline remains available.
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

async fn deadline_reached(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => pending::<()>().await,
    }
}
