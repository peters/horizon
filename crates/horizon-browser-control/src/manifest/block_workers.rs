//! The bounds of the workers of one orchestrator, and of the three texts that
//! travel between an orchestrator and a worker of it.
//!
//! A worker is an agent panel that Horizon starts from a preset of the person's
//! settings. The request of the orchestrator names that preset and gives one
//! task, so the preset holds every command, argument and kind that runs, and no
//! part of a request becomes a command line, a path or an option. The task, each
//! message of the orchestrator and each report of a worker are text of a
//! program: Horizon types the first two into a terminal as one line each, and it
//! passes the third to the program of the orchestrator as one turn.
//!
//! This module holds the rules and the refusal codes that the MCP server and the
//! UI both use, as [`super::block_proposal`] does for the front desk. The design
//! is [`docs/architecture/orchestrator-workers.md`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::block_machines::INVALID_REQUEST;

/// Most workers of one orchestrator that run at the same time.
pub const MAX_WORKERS: usize = 4;

/// Most characters of the task of one `worker_start`, and of one message and one
/// report.
pub const MAX_WORKER_TEXT_CHARS: usize = 4_000;

/// How long a worker waits before it reads a second message, and before an
/// orchestrator reads a second report of the same worker, in milliseconds.
pub const WORKER_INTERVAL_MILLIS: u64 = 10_000;

/// Most reports of one orchestrator that wait for the end of a turn.
pub const MAX_WAITING_REPORTS: usize = 8;

/// Most characters of the last report of a worker in the answer of `workers`.
pub const MAX_WORKER_REPORT_CHARS: usize = 400;

/// Refusal code of a start beyond [`MAX_WORKERS`].
pub const TOO_MANY_WORKERS: &str = "too_many_workers";

/// Refusal code of a request for a program that the chip `Workers` does not
/// hold: the person said `Not now`, and the refusal names no bound.
pub const NOT_ALLOWED: &str = "not_allowed";

/// The sentence that tells a worker how to report. Horizon types it after the
/// task of the first prompt line of the worker, so one line carries both.
pub const REPORT_SENTENCE: &str = " When the task is done, or when you cannot continue, call the block_panel tool with operation worker_report and one text that says what you did.";

/// Why one request of the workers is not one that Horizon may hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerRefusal {
    pub code: &'static str,
    pub message: String,
}

impl WorkerRefusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Check the task of one `worker_start`: 1 to [`MAX_WORKER_TEXT_CHARS`]
/// characters. The line breaks stay: Horizon replaces every control character of
/// the task with a space before it types the line, so the terminal reads one
/// line.
///
/// # Errors
/// Names the bound, with [`INVALID_REQUEST`].
pub fn check_task(task: &str) -> Result<(), WorkerRefusal> {
    check_text("task", task)
}

/// Check the text of one `worker_message`: 1 to [`MAX_WORKER_TEXT_CHARS`]
/// characters.
///
/// # Errors
/// Names the bound, with [`INVALID_REQUEST`].
pub fn check_message(text: &str) -> Result<(), WorkerRefusal> {
    check_text("text", text)
}

/// Check the text of one `worker_report`: 1 to [`MAX_WORKER_TEXT_CHARS`]
/// characters.
///
/// # Errors
/// Names the bound, with [`INVALID_REQUEST`].
pub fn check_report(text: &str) -> Result<(), WorkerRefusal> {
    check_text("text", text)
}

/// The rule of one text that a program sends: it is not white space, and it is
/// at most [`MAX_WORKER_TEXT_CHARS`] characters.
fn check_text(field: &str, text: &str) -> Result<(), WorkerRefusal> {
    let refused = |message: String| WorkerRefusal::new(INVALID_REQUEST, message);
    if text.trim().is_empty() {
        return Err(refused(format!("{field} cannot be empty")));
    }
    if text.chars().count() > MAX_WORKER_TEXT_CHARS {
        return Err(refused(format!(
            "{field} has {} characters, above the {MAX_WORKER_TEXT_CHARS} character limit",
            text.chars().count()
        )));
    }
    Ok(())
}

/// The task of one `worker_start` as the one line that Horizon types into the
/// worker: each control character becomes a space, so a terminal reads one line
/// and the program reads one message.
#[must_use]
pub fn task_line(task: &str) -> String {
    task.chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .collect()
}

/// The one prompt line of a worker: its task and [`REPORT_SENTENCE`].
#[must_use]
pub fn worker_prompt(task: &str) -> String {
    format!("{}{REPORT_SENTENCE}", task_line(task).trim())
}

/// One worker of one orchestrator, as `workers` reports it.
///
/// Every field is Horizon's own record: the local id of the panel of the
/// worker, the name of the preset that it started from, the kind of its program,
/// its state, and the last report of the program. The report is text of a
/// program, so it is cut to [`MAX_WORKER_REPORT_CHARS`] characters.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[schemars(extend("type" = "object"))]
pub struct WorkerSummary {
    /// The local id of the panel of the worker, which `worker_message` takes.
    pub worker: String,
    /// The name of the preset of the settings that the worker started from.
    pub program: String,
    /// The kind of that preset, as the settings store it, such as `claude`.
    pub kind: String,
    /// `working` while the agent of the panel runs a turn, and `idle` otherwise.
    pub state: String,
    /// The last text that the worker reported, cut to 400 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_report: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_a_message_and_a_report_hold_one_to_four_thousand_characters() {
        for check in [check_task, check_message, check_report] {
            assert_eq!(
                check(" ").expect_err("an empty text").code,
                INVALID_REQUEST,
                "white space is no text"
            );
            assert!(check("alpha").is_ok());
            let longest = "t".repeat(MAX_WORKER_TEXT_CHARS);
            assert!(check(&longest).is_ok(), "4 000 characters pass");
            let long = "t".repeat(MAX_WORKER_TEXT_CHARS + 1);
            let refusal = check(&long).expect_err("a text above the bound");
            assert_eq!(refusal.code, INVALID_REQUEST);
            assert!(refusal.message.contains("4001 characters"), "{}", refusal.message);
        }
        // A task and a report may hold line breaks: they are text of a program.
        assert!(check_task("line one\nline two").is_ok());
        assert!(check_report("one\ntwo").is_ok());
    }

    #[test]
    fn the_prompt_line_of_a_worker_is_its_task_on_one_line_and_the_sentence() {
        assert_eq!(task_line("fix it\nnow\u{7}"), "fix it now ");
        assert_eq!(
            worker_prompt("fix it"),
            "fix it When the task is done, or when you cannot continue, call the block_panel tool with operation worker_report and one text that says what you did."
        );
        assert!(
            worker_prompt(" two lines\r\nhere ").starts_with("two lines  here When the task"),
            "the line breaks become spaces"
        );
        assert_eq!(
            worker_prompt("fix it").lines().count(),
            1,
            "the prompt of a worker is one line"
        );
    }

    #[test]
    fn the_summary_of_a_worker_names_its_panel_its_program_and_its_state() {
        let summary = WorkerSummary {
            worker: "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e".into(),
            program: "Codex".into(),
            kind: "codex".into(),
            state: "idle".into(),
            last_report: None,
        };
        let json = serde_json::to_value(&summary).expect("the summary of a worker");
        assert_eq!(json["state"], serde_json::json!("idle"));
        assert!(
            json.get("last_report").is_none(),
            "a worker that reported nothing answers no report"
        );
    }
}
