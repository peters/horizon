//! Worker cost estimated from the provider's hourly rate. Amounts use the
//! provider's billing currency, which `RunPod` prices in US dollars.
use super::progress;
use crate::usage_stats::format_cost;
use horizon_cloud::{Worker, WorkerStatus};
use std::time::{Duration, SystemTime};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const SECONDS_PER_HOUR: f64 = 3600.0;

/// Estimated cost of a worker's current run, from its latest start until now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunCost {
    pub elapsed: Duration,
    pub amount: f64,
    /// Hourly rate behind the estimate.
    pub rate: f64,
}

impl RunCost {
    /// For example `This run · 1h 12m · $0.83 · $0.690/h`.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "This run · {} · {} · {}",
            progress::duration(self.elapsed),
            format_cost(self.amount),
            format_rate(self.rate)
        )
    }
}

/// The savings-plan rate when the provider reports one, otherwise the list rate.
/// A zero adjusted rate is treated as absent so a missing plan never shows a free worker.
#[must_use]
pub fn hourly_rate(worker: &Worker) -> Option<f64> {
    worker
        .adjusted_cost_per_hr
        .filter(|rate| *rate > 0.0)
        .or(worker.cost_per_hr)
}

/// The run's cost so far, or `None` unless the worker is running with a known
/// rate and start time. A start in the future (clock skew) counts as zero elapsed.
#[must_use]
pub fn current_run(worker: &Worker, now: SystemTime) -> Option<RunCost> {
    if !matches!(worker.status(), WorkerStatus::Running | WorkerStatus::Starting) {
        return None;
    }
    let rate = hourly_rate(worker)?;
    let started = started_at(worker.last_started_at.as_deref()?)?;
    let elapsed = now.duration_since(started).unwrap_or_default();
    Some(RunCost {
        elapsed,
        amount: rate * elapsed.as_secs_f64() / SECONDS_PER_HOUR,
        rate,
    })
}

/// For example `$0.690/h`; sub-cent rates stay distinguishable.
#[must_use]
pub fn format_rate(rate: f64) -> String {
    format!("${rate:.3}/h")
}

fn started_at(value: &str) -> Option<SystemTime> {
    let since_epoch = OffsetDateTime::parse(value, &Rfc3339).ok()? - OffsetDateTime::UNIX_EPOCH;
    SystemTime::UNIX_EPOCH.checked_add(since_epoch.try_into().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const STARTED: &str = "2024-07-12T19:14:40.144Z";

    fn worker(fields: &Value) -> Worker {
        let mut value = json!({
            "id":"worker1","name":"horizon-cloud-synthetic","imageName":"registry.example/worker",
            "desiredStatus":"RUNNING","publicIp":"192.0.2.1","portMappings":{"22":22001},
            "costPerHr":0.74,"adjustedCostPerHr":0.69,"lastStartedAt":STARTED
        });
        for (key, field) in fields.as_object().unwrap() {
            value[key] = field.clone();
        }
        serde_json::from_value(value).unwrap()
    }

    fn started() -> SystemTime {
        SystemTime::from(OffsetDateTime::parse(STARTED, &Rfc3339).unwrap())
    }

    #[test]
    fn current_run_uses_the_adjusted_rate_since_the_latest_start() {
        let now = started() + Duration::from_mins(72);
        let run = current_run(&worker(&json!({})), now).unwrap();
        assert_eq!(run.elapsed, Duration::from_mins(72));
        assert!((run.rate - 0.69).abs() < f64::EPSILON);
        assert!((run.amount - 0.828).abs() < 1e-9);
        assert_eq!(run.summary(), "This run · 1h 12m · $0.83 · $0.690/h");
        let early = current_run(&worker(&json!({})), started() + Duration::from_secs(307)).unwrap();
        assert_eq!(early.summary(), "This run · 5m 07s · $0.06 · $0.690/h");
    }

    #[test]
    fn list_rate_applies_without_a_positive_adjusted_rate() {
        for adjusted in [Value::Null, json!(0.0)] {
            let worker = worker(&json!({"adjustedCostPerHr": adjusted}));
            assert_eq!(hourly_rate(&worker), Some(0.74));
            let run = current_run(&worker, started() + Duration::from_secs(3600)).unwrap();
            assert!((run.amount - 0.74).abs() < 1e-9);
        }
        let unpriced = worker(&json!({"adjustedCostPerHr": null, "costPerHr": null}));
        assert_eq!(hourly_rate(&unpriced), None);
        assert_eq!(current_run(&unpriced, started()), None);
        assert_eq!(format_rate(0.0042), "$0.004/h");
    }

    #[test]
    fn start_times_accept_rfc3339_offsets_and_fractions_only() {
        let now = started() + Duration::from_secs(60);
        for same_instant in [STARTED, "2024-07-12T15:14:40.144-04:00"] {
            let run = current_run(&worker(&json!({"lastStartedAt": same_instant})), now).unwrap();
            assert_eq!(run.elapsed, Duration::from_secs(60));
        }
        let whole = current_run(&worker(&json!({"lastStartedAt": "2024-07-12T19:14:40Z"})), now).unwrap();
        assert_eq!(whole.elapsed, Duration::from_millis(60_144));
        for invalid in [
            json!(null),
            json!(""),
            json!("2024-07-12 19:14:40.144"),
            json!("Fri Jul 12 2024 15:14:40 GMT-0400"),
            json!("1969-12-31T23:59:59Z"),
        ] {
            assert_eq!(current_run(&worker(&json!({"lastStartedAt": invalid})), now), None);
        }
    }

    #[test]
    fn a_start_after_now_counts_as_zero_elapsed() {
        let run = current_run(&worker(&json!({})), started() - Duration::from_secs(30)).unwrap();
        assert_eq!(run.elapsed, Duration::ZERO);
        assert!(run.amount.abs() < f64::EPSILON);
    }

    #[test]
    fn only_a_running_worker_has_a_current_run() {
        let now = started() + Duration::from_secs(60);
        for status in ["EXITED", "TERMINATED", "UNKNOWN"] {
            assert_eq!(current_run(&worker(&json!({"desiredStatus": status})), now), None);
        }
        let unpublished = worker(&json!({"publicIp": "", "portMappings": null}));
        assert_eq!(unpublished.status(), WorkerStatus::Starting);
        assert!(
            current_run(&unpublished, now).is_some(),
            "a pending SSH endpoint does not pause the run"
        );
    }

    #[test]
    fn worker_records_keep_run_timing_and_accept_string_rates() {
        let parsed = worker(&json!({"adjustedCostPerHr": "0.690"}));
        assert_eq!(parsed.adjusted_cost_per_hr, Some(0.69));
        assert_eq!(parsed.last_started_at.as_deref(), Some(STARTED));
        let saved = serde_json::to_value(&parsed).unwrap();
        assert_eq!(saved["lastStartedAt"], STARTED);
        assert_eq!(saved["adjustedCostPerHr"], 0.69);
        let restored: Worker = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), saved);
        for invalid in [json!(-0.1), json!("free"), json!(true)] {
            let mut value = saved.clone();
            value["adjustedCostPerHr"] = invalid;
            assert!(serde_json::from_value::<Worker>(value).is_err());
        }
    }
}
