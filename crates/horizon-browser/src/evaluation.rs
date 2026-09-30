//! Caller-bound JavaScript evaluation shared by the browser drivers.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::navigation::{PendingNavigation, now_millis};
use crate::{AgentAction, BrowserControlAction, BrowserControlFailure};

pub(crate) struct EvaluationDeadline {
    started: Instant,
    queued_for: Duration,
    bound: Duration,
}

impl EvaluationDeadline {
    pub(crate) fn new(request: &AgentAction, timeout_millis: Option<u64>) -> Self {
        Self {
            started: Instant::now(),
            queued_for: PendingNavigation::queued_for(request, now_millis()),
            bound: Duration::from_millis(
                timeout_millis.unwrap_or(BrowserControlAction::DEFAULT_EVALUATION_TIMEOUT_MILLIS),
            ),
        }
    }

    fn elapsed(&self) -> Duration {
        self.queued_for.saturating_add(self.started.elapsed())
    }

    pub(crate) fn timeout(&self) -> BrowserControlFailure {
        BrowserControlFailure::new(
            "evaluation_timeout",
            format!(
                "JavaScript evaluation did not settle within the {} ms bound (elapsed {} ms)",
                self.bound.as_millis(),
                self.elapsed().as_millis(),
            ),
        )
    }

    pub(crate) fn remaining(&self) -> Result<Duration, BrowserControlFailure> {
        self.bound
            .checked_sub(self.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| self.timeout())
    }

    pub(crate) fn finish<T>(&self, result: Result<T, BrowserControlFailure>) -> Result<T, BrowserControlFailure> {
        self.remaining()?;
        result
    }
}

/// `WebDriver` awaits promise results in execute/sync. Its session script bound
/// must match this action too, then the previous setting must be restored.
pub(crate) fn evaluate_classic(
    mut command: impl FnMut(&str, Option<&Value>, Duration) -> Result<Value, BrowserControlFailure>,
    expression: &str,
    deadline: &EvaluationDeadline,
) -> Result<Value, BrowserControlFailure> {
    let original = deadline.finish(command("timeouts", None, deadline.remaining()?))?;
    let script = original
        .pointer("/value/script")
        .ok_or_else(|| BrowserControlFailure::new("invalid_result", "WebDriver returned no script timeout"))?;
    let remaining = deadline.remaining()?;
    let bound_millis = remaining.as_millis().max(1);
    let result = command("timeouts", Some(&json!({"script": bound_millis})), remaining).and_then(|_| {
        let remaining = deadline.remaining()?;
        command(
            "execute/sync",
            Some(&json!({"script": format!("return ({expression});"), "args": []})),
            remaining,
        )
    });
    let result = deadline.finish(result);
    // Restore even when setting the bound or evaluating returned a transport
    // error: either command might have reached the browser before it failed.
    if command("timeouts", Some(&json!({"script": script})), Duration::from_secs(1)).is_err() {
        return Err(match result {
            Err(mut failure) => {
                failure
                    .message
                    .push_str("; the browser script timeout could not be restored");
                failure
            }
            Ok(_) => BrowserControlFailure::new(
                "evaluation_restore_failed",
                "The browser script timeout could not be restored",
            ),
        });
    }
    result.and_then(|response| {
        response
            .get("value")
            .cloned()
            .ok_or_else(|| BrowserControlFailure::new("invalid_result", "WebDriver returned no script value"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> AgentAction {
        AgentAction {
            action_id: "evaluation-test".into(),
            actor: "test".into(),
            requested_at_millis: now_millis(),
            action: BrowserControlAction::Evaluate {
                expression: "true".into(),
                timeout_millis: None,
            },
        }
    }

    #[test]
    fn full_caller_bound_replaces_the_old_ten_second_limit() {
        let mut deadline = EvaluationDeadline::new(&request(), Some(20_000));
        deadline.started = Instant::now()
            .checked_sub(Duration::from_secs(12))
            .expect("test clock supports past interval");
        assert_eq!(
            deadline.finish(Ok(json!(true))).expect("within caller bound"),
            json!(true)
        );
        assert!(deadline.remaining().expect("remaining") > Duration::from_secs(7));
    }

    #[test]
    fn queue_latency_and_late_success_produce_a_typed_timeout() {
        let mut request = request();
        request.requested_at_millis -= 2_000;
        let deadline = EvaluationDeadline::new(&request, Some(1_000));
        let timeout = deadline.finish(Ok(json!(true))).expect_err("expired while queued");
        assert_eq!(timeout.code, "evaluation_timeout");
        assert!(timeout.message.contains("1000 ms bound"));
        assert!(timeout.message.contains("elapsed 200"));
    }

    #[test]
    fn javascript_errors_keep_their_classification_before_expiry() {
        let deadline = EvaluationDeadline::new(&request(), None);
        let error = BrowserControlFailure::new("javascript_error", "synthetic failure");
        assert_eq!(
            deadline.finish::<Value>(Err(error)).expect_err("failed").code,
            "javascript_error"
        );
    }

    #[test]
    fn classic_script_bound_and_http_bound_follow_the_caller_and_restore() {
        let deadline = EvaluationDeadline::new(&request(), Some(60_000));
        let mut calls = Vec::new();
        let value = evaluate_classic(
            |path, body, timeout| {
                calls.push((path.to_string(), body.cloned(), timeout));
                Ok(if body.is_none() {
                    json!({"value":{"script":30_000}})
                } else if path == "execute/sync" {
                    json!({"value":42})
                } else {
                    json!({"value":null})
                })
            },
            "Promise.resolve(42)",
            &deadline,
        )
        .expect("evaluated");
        assert_eq!(value, json!(42));
        assert_eq!(calls.len(), 4);
        assert!(
            calls[1].1.as_ref().expect("bound")["script"]
                .as_u64()
                .expect("milliseconds")
                > 59_000
        );
        assert!(calls[2].2 > Duration::from_secs(59));
        assert_eq!(calls[3].1, Some(json!({"script":30_000})));
    }

    #[test]
    fn a_timed_out_settings_read_has_the_same_typed_deadline() {
        let deadline = EvaluationDeadline::new(&request(), Some(1));
        let error = evaluate_classic(
            |_, _, _| {
                std::thread::sleep(Duration::from_millis(10));
                Err(BrowserControlFailure::new("javascript_error", "read timed out"))
            },
            "true",
            &deadline,
        )
        .expect_err("expired settings read");
        assert_eq!(error.code, "evaluation_timeout");
    }

    #[test]
    fn failed_restoration_keeps_the_original_typed_timeout() {
        let deadline = EvaluationDeadline::new(&request(), Some(50));
        let error = evaluate_classic(
            |path, body, _| {
                if body.is_none() {
                    Ok(json!({"value":{"script":30_000}}))
                } else if path == "execute/sync" {
                    std::thread::sleep(Duration::from_millis(60));
                    Err(BrowserControlFailure::new("javascript_error", "read timed out"))
                } else if body == Some(&json!({"script":30_000})) {
                    Err(BrowserControlFailure::new("javascript_error", "restore failed"))
                } else {
                    Ok(json!({"value":null}))
                }
            },
            "new Promise(() => {})",
            &deadline,
        )
        .expect_err("timed out and restore failed");
        assert_eq!(error.code, "evaluation_timeout");
        assert!(error.message.contains("could not be restored"));
    }

    #[test]
    fn classic_restores_the_previous_bound_after_a_failed_dispatch() {
        let deadline = EvaluationDeadline::new(&request(), Some(20_000));
        let mut calls = Vec::new();
        let error = evaluate_classic(
            |path, body, _| {
                calls.push((path.to_string(), body.cloned()));
                if path == "execute/sync" {
                    Err(BrowserControlFailure::new(
                        "javascript_error",
                        "synthetic JavaScript failure",
                    ))
                } else if body.is_none() {
                    Ok(json!({"value":{"script":null}}))
                } else {
                    Ok(json!({"value":null}))
                }
            },
            "Promise.reject('failure')",
            &deadline,
        )
        .expect_err("failed");
        assert_eq!(error.code, "javascript_error");
        assert_eq!(calls.last().expect("restored").1, Some(json!({"script":null})));
    }
}
