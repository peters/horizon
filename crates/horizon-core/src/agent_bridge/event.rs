//! The typed events of one bridge, and the reason that a line was dropped.
//!
//! These events are the whole vocabulary of the module: a program line becomes
//! one of them, or nothing. No event carries the text of a line that the codec
//! refused, and no event carries a whole tool input: a summary is built from the
//! fixed keys of [`super::codec`] and cut to [`super::MAX_SUMMARY_CHARS`].

/// One event of the program that a bridge runs.
///
/// `AgentText` carries the whole text of one message so far, so a later event
/// with the same `message_id` replaces the earlier one. A summary of a tool is
/// the fixed key of its input, cleaned and cut, and never the input itself.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum BridgeEvent {
    /// The program named the session of its conversation. A program emits this
    /// at the start of every turn.
    SessionStarted { session_id: String },
    /// One turn of the conversation started.
    TurnStarted,
    /// The whole text of one agent message so far.
    AgentText { message_id: String, text: String },
    /// The agent called one tool.
    ToolStarted {
        tool_id: String,
        name: String,
        summary: String,
    },
    /// The result of one tool call arrived.
    ToolEnded {
        tool_id: String,
        failed: bool,
        summary: String,
    },
    /// The program refused one tool call, and said why.
    PermissionDenied { tool: String, message: String },
    /// A window of the account of the program limits it.
    Limit {
        status: String,
        using_overage: bool,
        resets_at_unix: Option<u64>,
    },
    /// One turn of the conversation ended.
    TurnEnded {
        failed: bool,
        duration_ms: Option<u64>,
        cost_usd: Option<f64>,
    },
    /// The codec refused one line and dropped it whole. The event holds the
    /// reason and never the line.
    Dropped { reason: DropReason },
    /// The standard output of the program ended, with the exit code of the
    /// program and the last bytes of its standard error.
    Ended {
        exit_code: Option<i32>,
        stderr_tail: String,
    },
}

/// Why the codec refused one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropReason {
    /// The line is above [`super::MAX_LINE_BYTES`]. The reader skipped it to its
    /// newline and held no byte of it.
    TooLong,
    /// The line is not JSON.
    NotJson,
    /// The line is JSON but not an object with a `type` string.
    UnknownShape,
}
