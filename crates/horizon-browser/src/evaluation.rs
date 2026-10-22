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

/// Script bound this session gets when its driver reports none. It is the
/// `WebDriver` default, so a driver that reports nothing keeps the bound it
/// would have had, and Horizon never restores a value it did not read.
pub(crate) const ESTABLISHED_SCRIPT_TIMEOUT_MILLIS: u64 = 30_000;

/// What the session itself had as its script bound before this evaluation.
enum ScriptBaseline {
    /// The driver reported its own setting; it goes back afterwards.
    Reported(Value),
    /// The driver reported no usable setting, as `Appium`'s base driver does
    /// with its `command` and `implicit` answer. The previous value is
    /// unreadable, so the session gets an established bound instead of a guess.
    Unreported,
}

impl ScriptBaseline {
    fn read(response: &Value) -> Self {
        match response.pointer("/value/script") {
            Some(script) if script.is_number() => Self::Reported(script.clone()),
            _ => Self::Unreported,
        }
    }

    fn bound(&self) -> Value {
        match self {
            Self::Reported(script) => script.clone(),
            Self::Unreported => json!(ESTABLISHED_SCRIPT_TIMEOUT_MILLIS),
        }
    }

    /// A driver that reports no script bound cannot be trusted to await a
    /// returned promise inside `execute/sync`: `Appium`'s script backend
    /// serializes the pending promise instead. Those evaluations await the
    /// promise in the page instead.
    fn needs_page_await(&self) -> bool {
        matches!(self, Self::Unreported)
    }

    /// Why the session could not be left at the bound this baseline describes.
    fn write_failure(&self) -> String {
        match self {
            Self::Reported(_) => "the browser script timeout could not be restored".to_string(),
            Self::Unreported => {
                format!("the session script timeout could not be established at {ESTABLISHED_SCRIPT_TIMEOUT_MILLIS} ms")
            }
        }
    }
}

/// Page global that carries one pending evaluation promise from its
/// `execute/sync` call to the `execute/async` call that awaits it.
const PENDING_PROMISE_GLOBAL: &str = "__horizonPendingPromise";

/// What one evaluation answer carries. The page wraps its answer so a pending
/// promise cannot be mistaken for an empty object.
enum EvaluationAnswer {
    Settled(Value),
    Pending,
    Rejected(String),
    Lost,
}

fn evaluation_answer(value: &Value) -> Result<EvaluationAnswer, BrowserControlFailure> {
    if let Some(settled) = value.get("horizonValue") {
        return Ok(EvaluationAnswer::Settled(settled.clone()));
    }
    if value.get("horizonPending").is_some_and(Value::is_boolean) {
        return Ok(EvaluationAnswer::Pending);
    }
    if let Some(reason) = value.get("horizonRejected") {
        return Ok(EvaluationAnswer::Rejected(
            reason.as_str().unwrap_or("promise rejected").to_string(),
        ));
    }
    if value.get("horizonLost").is_some_and(Value::is_boolean) {
        return Ok(EvaluationAnswer::Lost);
    }
    Err(BrowserControlFailure::new(
        "invalid_result",
        "WebDriver returned no script value",
    ))
}

/// `execute/sync` script. Without the handshake it is the expression itself, as
/// every driver that reports a script bound awaits its promise. With the
/// handshake the expression runs once, and a returned promise is kept for the
/// `execute/async` call instead of being serialized as it is.
fn sync_script(expression: &str, handshake: bool) -> String {
    if !handshake {
        return format!("return ({expression});");
    }
    format!(
        "const value = ({expression});\
         if (value && typeof value.then === 'function') {{\
           window.{PENDING_PROMISE_GLOBAL} = value;\
           return {{horizonPending: true}};\
         }}\
         return {{horizonValue: value === undefined ? null : value}};"
    )
}

/// `execute/async` script that settles the promise the `execute/sync` call kept.
/// The driver bounds it with the session script timeout this evaluation set, so
/// the caller deadline governs it too.
fn async_script() -> String {
    format!(
        "const done = arguments[arguments.length - 1];\
         const pending = window.{PENDING_PROMISE_GLOBAL};\
         delete window.{PENDING_PROMISE_GLOBAL};\
         if (pending === undefined) {{\
           done({{horizonLost: true}});\
           return;\
         }}\
         Promise.resolve(pending).then(\
           (value) => done({{horizonValue: value === undefined ? null : value}}),\
           (error) => done({{horizonRejected: String(error && error.message ? error.message : error)}})\
         );"
    )
}

/// One evaluation under the session's bound, with the promise handshake when
/// the driver needs it.
fn evaluate_with_handshake(
    command: &mut impl FnMut(&str, Option<&Value>, Duration) -> Result<Value, BrowserControlFailure>,
    expression: &str,
    deadline: &EvaluationDeadline,
    handshake: bool,
) -> Result<Value, BrowserControlFailure> {
    let missing = || BrowserControlFailure::new("invalid_result", "WebDriver returned no script value");
    let response = command(
        "execute/sync",
        Some(&json!({"script": sync_script(expression, handshake), "args": []})),
        deadline.remaining()?,
    )?;
    let value = response.get("value").cloned().ok_or_else(missing)?;
    if !handshake {
        return Ok(value);
    }
    let answer = evaluation_answer(&value)?;
    if !matches!(answer, EvaluationAnswer::Pending) {
        return settle(answer);
    }
    let awaited = command(
        "execute/async",
        Some(&json!({"script": async_script(), "args": []})),
        deadline.remaining()?,
    )?;
    let settled = awaited.get("value").cloned().ok_or_else(missing)?;
    settle(evaluation_answer(&settled)?)
}

fn settle(answer: EvaluationAnswer) -> Result<Value, BrowserControlFailure> {
    match answer {
        EvaluationAnswer::Settled(value) => Ok(value),
        EvaluationAnswer::Rejected(reason) => Err(BrowserControlFailure::new(
            "javascript_error",
            format!("promise rejected: {reason}"),
        )),
        EvaluationAnswer::Lost => Err(BrowserControlFailure::new(
            "javascript_error",
            "the document was replaced before the evaluation promise settled",
        )),
        EvaluationAnswer::Pending => Err(BrowserControlFailure::new(
            "invalid_result",
            "the evaluation promise is still pending after the driver answered",
        )),
    }
}

/// `WebDriver` awaits promise results in execute/sync. Its session script bound
/// must match this action too, then the session's own setting must be left in
/// place: the value the driver reports, or an established bound when the driver
/// reports none.
pub(crate) fn evaluate_classic(
    mut command: impl FnMut(&str, Option<&Value>, Duration) -> Result<Value, BrowserControlFailure>,
    expression: &str,
    deadline: &EvaluationDeadline,
) -> Result<Value, BrowserControlFailure> {
    let settings = deadline.finish(command("timeouts", None, deadline.remaining()?))?;
    let baseline = ScriptBaseline::read(&settings);
    let handshake = baseline.needs_page_await();
    let remaining = deadline.remaining()?;
    let bound_millis = remaining.as_millis().max(1);
    let result = command("timeouts", Some(&json!({"script": bound_millis})), remaining)
        .and_then(|_| evaluate_with_handshake(&mut command, expression, deadline, handshake));
    let result = deadline.finish(result);
    // Write the session bound even when setting the bound or evaluating returned
    // a transport error: either command might have reached the browser first.
    let write_failure = baseline.write_failure();
    if command(
        "timeouts",
        Some(&json!({"script": baseline.bound()})),
        Duration::from_secs(1),
    )
    .is_err()
    {
        return Err(match result {
            Err(mut failure) => {
                failure.message.push_str("; ");
                failure.message.push_str(&write_failure);
                failure
            }
            Ok(_) => BrowserControlFailure::new("evaluation_restore_failed", write_failure),
        });
    }
    result
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
                    Ok(json!({"value":{"script":30_000}}))
                } else {
                    Ok(json!({"value":null}))
                }
            },
            "Promise.reject('failure')",
            &deadline,
        )
        .expect_err("failed");
        assert_eq!(error.code, "javascript_error");
        assert_eq!(calls.last().expect("restored").1, Some(json!({"script":30_000})));
    }

    #[test]
    fn only_a_reported_number_is_restored() {
        assert!(matches!(
            ScriptBaseline::read(&json!({"value":{"script":30_000}})),
            ScriptBaseline::Reported(_)
        ));
        for unreported in [
            json!({"value":{"command":60,"implicit":0}}),
            json!({"value":{"script":null}}),
            json!({"value":{"script":"30000"}}),
            json!({"value":null}),
            json!({}),
        ] {
            assert!(matches!(ScriptBaseline::read(&unreported), ScriptBaseline::Unreported));
        }
    }

    #[test]
    fn an_unreported_script_timeout_is_established_instead_of_guessed() {
        let deadline = EvaluationDeadline::new(&request(), Some(60_000));
        let mut calls = Vec::new();
        let value = evaluate_classic(
            |path, body, timeout| {
                calls.push((path.to_string(), body.cloned(), timeout));
                Ok(if body.is_none() {
                    // The answer `Appium`'s base driver gives: no `script`.
                    json!({"value":{"command":60,"implicit":0}})
                } else if path == "execute/sync" {
                    json!({"value":{"horizonValue":{"answer":42}}})
                } else {
                    json!({"value":null})
                })
            },
            "({answer: 42})",
            &deadline,
        )
        .expect("evaluated without a reported script timeout");
        assert_eq!(value, json!({"answer":42}));
        assert_eq!(calls.len(), 4);
        assert!(
            calls[1].1.as_ref().expect("bound")["script"]
                .as_u64()
                .expect("milliseconds")
                > 59_000
        );
        assert_eq!(
            calls[3].1,
            Some(json!({"script": ESTABLISHED_SCRIPT_TIMEOUT_MILLIS})),
            "the session is left at the bound Horizon established"
        );
    }

    #[test]
    fn a_reported_script_bound_keeps_the_plain_sync_script() {
        let deadline = EvaluationDeadline::new(&request(), Some(20_000));
        let mut calls = Vec::new();
        let value = evaluate_classic(
            |path, body, _| {
                calls.push((path.to_string(), body.cloned()));
                Ok(if body.is_none() {
                    json!({"value":{"script":30_000}})
                } else if path == "execute/sync" {
                    json!({"value":"plain"})
                } else {
                    json!({"value":null})
                })
            },
            "true",
            &deadline,
        )
        .expect("evaluated");
        assert_eq!(value, json!("plain"));
        assert_eq!(calls[2].0, "execute/sync");
        let script = calls[2].1.as_ref().expect("body")["script"].as_str().expect("script");
        assert_eq!(script, "return (true);");
        assert!(!script.contains(PENDING_PROMISE_GLOBAL));
        assert_eq!(calls.len(), 4);
    }

    #[test]
    fn an_unreported_script_bound_awaits_a_promise_in_the_page() {
        let deadline = EvaluationDeadline::new(&request(), Some(20_000));
        let mut calls = Vec::new();
        let value = evaluate_classic(
            |path, body, _| {
                calls.push((path.to_string(), body.cloned()));
                Ok(if body.is_none() {
                    json!({"value":{"command":60,"implicit":0}})
                } else if path == "execute/sync" {
                    json!({"value":{"horizonPending":true}})
                } else if path == "execute/async" {
                    json!({"value":{"horizonValue":7}})
                } else {
                    json!({"value":null})
                })
            },
            "new Promise((resolve) => setTimeout(() => resolve(7), 50))",
            &deadline,
        )
        .expect("awaited the promise");
        assert_eq!(value, json!(7));
        assert_eq!(
            calls.iter().map(|call| call.0.as_str()).collect::<Vec<_>>(),
            ["timeouts", "timeouts", "execute/sync", "execute/async", "timeouts"]
        );
        let sync = calls[2].1.as_ref().expect("sync body")["script"]
            .as_str()
            .expect("sync script");
        assert!(sync.contains(PENDING_PROMISE_GLOBAL));
        let awaited = calls[3].1.as_ref().expect("async body")["script"]
            .as_str()
            .expect("async script");
        assert!(awaited.contains(PENDING_PROMISE_GLOBAL));
        assert!(awaited.contains("arguments[arguments.length - 1]"));
        assert_eq!(
            calls[4].1,
            Some(json!({"script": ESTABLISHED_SCRIPT_TIMEOUT_MILLIS})),
            "the session bound is established after the await too"
        );
    }

    #[test]
    fn a_rejected_evaluation_promise_is_a_javascript_error() {
        let deadline = EvaluationDeadline::new(&request(), Some(20_000));
        let error = evaluate_classic(
            |path, body, _| {
                Ok(if body.is_none() {
                    json!({"value":{"command":60,"implicit":0}})
                } else if path == "execute/sync" {
                    json!({"value":{"horizonPending":true}})
                } else if path == "execute/async" {
                    json!({"value":{"horizonRejected":"synthetic rejection"}})
                } else {
                    json!({"value":null})
                })
            },
            "Promise.reject(new Error('synthetic rejection'))",
            &deadline,
        )
        .expect_err("rejected");
        assert_eq!(error.code, "javascript_error");
        assert!(error.message.contains("promise rejected: synthetic rejection"));
    }

    #[test]
    fn a_promise_that_outlives_its_document_is_a_typed_failure() {
        let deadline = EvaluationDeadline::new(&request(), Some(20_000));
        let error = evaluate_classic(
            |path, body, _| {
                Ok(if body.is_none() {
                    json!({"value":{"command":60,"implicit":0}})
                } else if path == "execute/sync" {
                    json!({"value":{"horizonPending":true}})
                } else if path == "execute/async" {
                    json!({"value":{"horizonLost":true}})
                } else {
                    json!({"value":null})
                })
            },
            "new Promise(() => {})",
            &deadline,
        )
        .expect_err("lost its document");
        assert_eq!(error.code, "javascript_error");
        assert!(error.message.contains("document was replaced"));
    }

    #[test]
    fn an_unreported_script_timeout_stays_established_after_a_failed_dispatch() {
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
                    Ok(json!({"value":{"command":60,"implicit":0}}))
                } else {
                    Ok(json!({"value":null}))
                }
            },
            "Promise.reject('failure')",
            &deadline,
        )
        .expect_err("failed");
        assert_eq!(error.code, "javascript_error");
        assert_eq!(
            calls.last().expect("established").1,
            Some(json!({"script": ESTABLISHED_SCRIPT_TIMEOUT_MILLIS}))
        );
    }

    #[test]
    fn a_failed_establishment_is_reported_as_an_establishment() {
        let deadline = EvaluationDeadline::new(&request(), Some(20_000));
        let error = evaluate_classic(
            |path, body, _| {
                if body.is_none() {
                    Ok(json!({"value":{"command":60,"implicit":0}}))
                } else if path == "execute/sync" {
                    Ok(json!({"value":{"horizonValue":42}}))
                } else if body == Some(&json!({"script": ESTABLISHED_SCRIPT_TIMEOUT_MILLIS})) {
                    Err(BrowserControlFailure::new("javascript_error", "write failed"))
                } else {
                    Ok(json!({"value":null}))
                }
            },
            "42",
            &deadline,
        )
        .expect_err("establishment failed");
        assert_eq!(error.code, "evaluation_restore_failed");
        assert!(error.message.contains("could not be established"));
        assert!(!error.message.contains("restored"));
    }
}
