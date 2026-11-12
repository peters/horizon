//! The budget that the tests of the session writer wait with.

use std::time::Duration;

/// How long a test waits for the writer to make a board durable.
///
/// The bound is a budget of the test and not a rule of the product: the writer
/// syncs each file before and after its rename, and a machine whose disk is busy
/// can take a minute for one save. The exit of Horizon keeps its own bound
/// (`MAX_SHUTDOWN_WAIT`), which warns and ends the exit instead of waiting.
pub(crate) const SAVE_WAIT: Duration = Duration::from_secs(180);
