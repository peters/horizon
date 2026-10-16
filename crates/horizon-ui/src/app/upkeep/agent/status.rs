//! The `status` answer: the settings and what the board shows, with the board's limits.
use horizon_core::upkeep::Status;
use horizon_core::upkeep::worker::{self, Config};
use serde_json::{Value, json};

/// The most pull requests, runs and activity lines a `status` answer carries.
const STATUS_PULL_REQUESTS: usize = 200;
const STATUS_RUNS: usize = 48;
const STATUS_ACTIVITY: usize = 20;

/// The settings, and what the board shows, with the board's limits. The agent on the worker
/// writes the status document, so only its parsed, bounded fields go back.
pub(super) fn status(cloud: &str, config: &Config, loop_running: Option<bool>) -> Value {
    let mut answer = json!({
        "cloud": cloud,
        "repositories": config.repositories,
        "authors": config.authors,
        "every_minutes": config.every_minutes,
        "checkouts": config.checkouts.name(),
        "loop_running": loop_running,
    });
    match horizon_core::upkeep::read_status(&worker::directory(cloud).join("status.json")) {
        Ok(board) => {
            if let Some(object) = answer.as_object_mut() {
                object.extend(board_json(&board).as_object().cloned().unwrap_or_default());
            }
        }
        // The parser's message can quote the document, which the worker wrote.
        Err(_) => answer["status_error"] = Value::from("The Upkeep status is missing or not valid."),
    }
    answer
}

pub(super) fn board_json(board: &Status) -> Value {
    let time = |at: Option<horizon_core::upkeep::time::OffsetDateTime>| {
        at.and_then(|at| {
            at.format(&horizon_core::upkeep::time::format_description::well_known::Rfc3339)
                .ok()
        })
    };
    let pull_requests: Vec<Value> = board
        .pull_requests
        .iter()
        .take(STATUS_PULL_REQUESTS)
        .map(|pr| {
            json!({
                "repository": pr.repository,
                "number": pr.number,
                "title": pr.title,
                "url": pr.github_url(),
                "state": pr.state,
                "needs": pr.needs_tooling().then_some("tooling").or(pr.needs_person().then_some("person")),
                "checks": pr.checks,
                "note": pr.note,
                "updated_at": time(pr.updated_at),
            })
        })
        .collect();
    let runs: Vec<Value> = board
        .runs
        .iter()
        .rev()
        .take(STATUS_RUNS)
        .map(|run| {
            json!({
                "started_at": time(Some(run.started_at)),
                "finished_at": time(run.finished_at),
                "outcome": run.outcome,
                "merged": run.merged,
            })
        })
        .collect();
    let activity: Vec<Value> = board
        .activity
        .iter()
        .rev()
        .take(STATUS_ACTIVITY)
        .map(|line| json!({ "at": time(Some(line.at)), "text": line.text }))
        .collect();
    json!({
        "paused": board.schedule.paused,
        "next_run_at": time(board.schedule.next_run_at),
        "runs": runs,
        "pull_requests": pull_requests,
        "activity": activity,
    })
}
