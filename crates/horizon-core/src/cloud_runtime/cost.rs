//! Worker cost estimated from the provider's hourly rate. Amounts use the
//! provider's billing currency, which `RunPod` prices in US dollars.
use super::progress;
use crate::usage_stats::format_cost;
use horizon_cloud::{
    Worker, WorkerStatus,
    runpod::billing::{BillingBucket, BucketSize},
};
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
    let (started, rate) = live_run(worker)?;
    let elapsed = now.duration_since(started).unwrap_or_default();
    Some(RunCost {
        elapsed,
        amount: rate * elapsed.as_secs_f64() / SECONDS_PER_HOUR,
        rate,
    })
}

/// A worker's cost since creation, or over the read window when it was billed
/// before that window: the provider's billing plus an estimate for the time its
/// billing has not reached yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TotalCost {
    pub billed: f64,
    pub estimated: f64,
    /// Where the billed span ends: the latest bucket's start when that bucket is
    /// estimated instead, otherwise its end, never after `now`. `None` without billing.
    pub billed_through: Option<SystemTime>,
    /// The window start when charges before it may exist and are not included;
    /// `None` when billing began inside the window, so the total is lifetime.
    pub excludes_before: Option<SystemTime>,
}

impl TotalCost {
    #[must_use]
    pub fn total(&self) -> f64 {
        self.billed + self.estimated
    }

    /// For example `Since creation · $4.20 (billed $3.37 + $0.83 estimated)`, or
    /// `Past 12 months · …` when older charges are not included. The parts are
    /// rounded to cents first, so the shown sum adds up.
    #[must_use]
    pub fn summary(&self) -> String {
        let (billed, estimated) = (cents(self.billed), cents(self.estimated));
        format!(
            "{} · {} (billed {} + {} estimated)",
            if self.excludes_before.is_some() {
                "Past 12 months"
            } else {
                "Since creation"
            },
            format_cost(billed + estimated),
            format_cost(billed),
            format_cost(estimated)
        )
    }

    /// `$4.20 total` for a lifetime total, `$4.20 12 mo` for the read window.
    fn badge(&self) -> String {
        let span = if self.excludes_before.is_some() {
            "12 mo"
        } else {
            "total"
        };
        format!("{} {span}", format_cost(self.shown_total()))
    }

    fn shown_total(&self) -> f64 {
        cents(self.billed) + cents(self.estimated)
    }
}

/// Combines non-overlapping billing buckets read since `from` with the current
/// run; see [`Billing::new`] and [`Billing::total`].
#[must_use]
pub fn total(buckets: &[BillingBucket], from: SystemTime, worker: &Worker, now: SystemTime) -> TotalCost {
    Billing::new(buckets, from).total(worker, now)
}

/// Billing buckets reduced once to what a total needs, so repeated totals parse nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Billing {
    /// Buckets that start before the latest one.
    earlier: f64,
    latest: Option<Latest>,
    excludes_before: Option<SystemTime>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Latest {
    start: SystemTime,
    end: Option<SystemTime>,
    amount: f64,
}

impl Billing {
    /// Reduces the buckets of a window that starts at `from`. The total counts as
    /// lifetime only when the window's first day has no charges, so billing began
    /// inside the window; otherwise older charges may exist and it covers the
    /// window. A worker that was billed nothing at all for that first day, such as
    /// one stopped without its own disk, is taken to have started later. Buckets
    /// without an RFC 3339 start are ignored rather than risk counting them twice.
    #[must_use]
    pub fn new(buckets: &[BillingBucket], from: SystemTime) -> Self {
        let dated = || {
            buckets
                .iter()
                .filter_map(|bucket| Some((started_at(&bucket.time)?, bucket)))
        };
        let Some((start, bucket)) = dated().max_by_key(|(start, _)| *start) else {
            return Self::default();
        };
        let first_day_end = from.checked_add(BucketSize::Day.duration());
        let excludes_before = dated()
            .any(|(start, _)| first_day_end.is_none_or(|end| start < end))
            .then_some(from);
        let (mut earlier, mut amount) = (0.0, 0.0);
        for (bucket_start, bucket) in dated() {
            if bucket_start < start {
                earlier += bucket.amount;
            } else {
                amount += bucket.amount;
            }
        }
        Self {
            earlier,
            latest: Some(Latest {
                start,
                end: start.checked_add(bucket.size.duration()),
                amount,
            }),
            excludes_before,
        }
    }

    /// The worker's cost since creation, without counting anything twice.
    ///
    /// Billing trails the worker, so its latest bucket may still be filling. While
    /// the worker runs and its run started before the latest bucket ended, that
    /// bucket is replaced by an estimate: the hourly rate times the running time
    /// from the later of the bucket's start and the run's start until `now`, and
    /// every earlier bucket counts as billed. Otherwise, when the worker is stopped
    /// or its run started after the latest bucket ended, every bucket counts as
    /// billed and only the current run, if any, is estimated. Without buckets the
    /// whole current run is estimated.
    #[must_use]
    pub fn total(&self, worker: &Worker, now: SystemTime) -> TotalCost {
        let run = live_run(worker);
        let Some(latest) = self.latest else {
            return TotalCost {
                billed: 0.0,
                estimated: run.map_or(0.0, |(started, rate)| estimate(rate, started, now)),
                billed_through: None,
                excludes_before: self.excludes_before,
            };
        };
        match run {
            Some((started, rate)) if latest.end.is_none_or(|end| started < end) => TotalCost {
                billed: self.earlier,
                estimated: estimate(rate, started.max(latest.start), now),
                billed_through: Some(latest.start.min(now)),
                excludes_before: self.excludes_before,
            },
            run => TotalCost {
                billed: self.earlier + latest.amount,
                estimated: run.map_or(0.0, |(started, rate)| estimate(rate, started, now)),
                billed_through: latest.end.map(|end| end.min(now)),
                excludes_before: self.excludes_before,
            },
        }
    }
}

/// Compact header text: `$0.83 run · $4.20 total`, `$4.20 total` or `$0.83 run`,
/// with `12 mo` instead of `total` when older charges are not included.
#[must_use]
pub fn badge(run: Option<&RunCost>, total: Option<&TotalCost>) -> Option<String> {
    match (run, total) {
        (Some(run), Some(total)) => Some(format!("{} run · {}", format_cost(run.amount), total.badge())),
        (None, Some(total)) => Some(total.badge()),
        (Some(run), None) => Some(format!("{} run", format_cost(run.amount))),
        (None, None) => None,
    }
}

/// For example `$0.690/h`; sub-cent rates stay distinguishable.
#[must_use]
pub fn format_rate(rate: f64) -> String {
    format!("${rate:.3}/h")
}

/// Start and hourly rate of a running worker's current run.
fn live_run(worker: &Worker) -> Option<(SystemTime, f64)> {
    if !matches!(worker.status(), WorkerStatus::Running | WorkerStatus::Starting) {
        return None;
    }
    let rate = hourly_rate(worker)?;
    Some((started_at(worker.last_started_at.as_deref()?)?, rate))
}

fn estimate(rate: f64, from: SystemTime, now: SystemTime) -> f64 {
    rate * now.duration_since(from).unwrap_or_default().as_secs_f64() / SECONDS_PER_HOUR
}

fn cents(amount: f64) -> f64 {
    (amount * 100.0).round() / 100.0
}

pub(super) fn started_at(value: &str) -> Option<SystemTime> {
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

    fn at(time: &str) -> SystemTime {
        SystemTime::from(OffsetDateTime::parse(time, &Rfc3339).unwrap())
    }

    fn bucket(time: &str, size: BucketSize, amount: f64) -> BillingBucket {
        BillingBucket {
            time: time.into(),
            size,
            amount,
            time_billed_ms: 0,
        }
    }

    /// A window that starts long before the synthetic worker was first billed.
    fn window() -> SystemTime {
        at("2023-07-14T00:00:00Z")
    }

    /// Two whole days, then hours of the current day with a partial latest hour.
    fn history() -> Vec<BillingBucket> {
        vec![
            bucket("2024-07-10T00:00:00Z", BucketSize::Day, 5.0),
            bucket("2024-07-11T00:00:00Z", BucketSize::Day, 12.0),
            bucket("2024-07-12T17:00:00Z", BucketSize::Hour, 0.69),
            bucket("2024-07-12T18:00:00Z", BucketSize::Hour, 0.69),
            bucket("2024-07-12T19:00:00Z", BucketSize::Hour, 0.25),
        ]
    }

    fn assert_total(total: TotalCost, billed: f64, estimated: f64, through: Option<&str>) {
        assert!((total.billed - billed).abs() < 1e-9, "billed {total:?}");
        assert!((total.estimated - estimated).abs() < 1e-9, "estimated {total:?}");
        assert!((total.total() - billed - estimated).abs() < 1e-9);
        assert_eq!(total.billed_through, through.map(at));
    }

    #[test]
    fn a_partial_latest_bucket_of_a_running_worker_is_estimated_instead_of_billed() {
        let running = worker(&json!({"lastStartedAt": "2024-07-12T09:00:00Z"}));
        let now = at("2024-07-12T19:30:00Z");
        assert_total(
            total(&history(), window(), &running, now),
            18.38,
            0.345,
            Some("2024-07-12T19:00:00Z"),
        );
        let mut shuffled = history();
        shuffled.reverse();
        shuffled.push(bucket("yesterday", BucketSize::Hour, 99.0));
        assert_total(
            total(&shuffled, window(), &running, now),
            18.38,
            0.345,
            Some("2024-07-12T19:00:00Z"),
        );
    }

    #[test]
    fn a_run_that_started_inside_the_latest_bucket_is_estimated_from_its_start() {
        let now = started() + Duration::from_mins(30);
        assert_total(
            total(&history(), window(), &worker(&json!({})), now),
            18.38,
            0.345,
            Some("2024-07-12T19:00:00Z"),
        );
    }

    #[test]
    fn a_run_that_started_after_the_latest_bucket_ended_keeps_that_bucket_billed() {
        let mut billed = history();
        billed.truncate(3);
        let now = started() + Duration::from_hours(1);
        assert_total(
            total(&billed, window(), &worker(&json!({})), now),
            17.69,
            0.69,
            Some("2024-07-12T18:00:00Z"),
        );
    }

    #[test]
    fn a_stopped_worker_counts_every_bucket_as_billed_and_estimates_nothing() {
        let stopped = worker(&json!({"desiredStatus": "EXITED"}));
        let now = at("2024-07-12T19:30:00Z");
        assert_total(
            total(&history(), window(), &stopped, now),
            18.63,
            0.0,
            Some("2024-07-12T19:30:00Z"),
        );
        assert_total(
            total(&history(), window(), &stopped, at("2024-07-13T08:00:00Z")),
            18.63,
            0.0,
            Some("2024-07-12T20:00:00Z"),
        );
    }

    #[test]
    fn without_billing_only_the_current_run_is_estimated() {
        let now = started() + Duration::from_mins(72);
        let running = total(&[], window(), &worker(&json!({})), now);
        assert_total(running, 0.0, 0.828, None);
        let stopped = total(&[], window(), &worker(&json!({"desiredStatus": "EXITED"})), now);
        assert_total(stopped, 0.0, 0.0, None);
        let unpriced = worker(&json!({"costPerHr": null, "adjustedCostPerHr": null}));
        assert_total(
            total(&history(), window(), &unpriced, now),
            18.63,
            0.0,
            Some("2024-07-12T20:00:00Z"),
        );
    }

    #[test]
    fn a_latest_day_bucket_is_estimated_from_the_later_of_its_start_and_the_run() {
        let mut days = history();
        days.truncate(2);
        let running = worker(&json!({"lastStartedAt": "2024-07-11T20:00:00Z"}));
        assert_total(
            total(&days, window(), &running, at("2024-07-12T00:30:00Z")),
            5.0,
            0.69 * 4.5,
            Some("2024-07-11T00:00:00Z"),
        );
        let since_before = worker(&json!({"lastStartedAt": "2024-07-09T12:00:00Z"}));
        assert_total(
            total(&days, window(), &since_before, at("2024-07-11T06:00:00Z")),
            5.0,
            0.69 * 6.0,
            Some("2024-07-11T00:00:00Z"),
        );
    }

    #[test]
    fn clock_skew_never_estimates_negative_time() {
        let future_start = worker(&json!({"lastStartedAt": "2024-07-12T19:45:00Z"}));
        assert_total(
            total(&history(), window(), &future_start, at("2024-07-12T19:30:00Z")),
            18.38,
            0.0,
            Some("2024-07-12T19:00:00Z"),
        );
        let running = worker(&json!({"lastStartedAt": "2024-07-12T09:00:00Z"}));
        assert_total(
            total(&history(), window(), &running, at("2024-07-12T18:40:00Z")),
            18.38,
            0.0,
            Some("2024-07-12T18:40:00Z"),
        );
    }

    #[test]
    fn totals_and_badges_show_cent_rounded_parts_that_add_up() {
        let mut total = TotalCost {
            billed: 3.374,
            estimated: 0.834,
            billed_through: None,
            excludes_before: None,
        };
        assert_eq!(
            total.summary(),
            "Since creation · $4.20 (billed $3.37 + $0.83 estimated)"
        );
        let run = RunCost {
            elapsed: Duration::from_mins(72),
            amount: 0.828,
            rate: 0.69,
        };
        assert_eq!(
            badge(Some(&run), Some(&total)).as_deref(),
            Some("$0.83 run · $4.20 total")
        );
        assert_eq!(badge(None, Some(&total)).as_deref(), Some("$4.20 total"));
        assert_eq!(badge(Some(&run), None).as_deref(), Some("$0.83 run"));
        assert_eq!(badge(None, None), None);
        total.excludes_before = Some(window());
        assert_eq!(
            total.summary(),
            "Past 12 months · $4.20 (billed $3.37 + $0.83 estimated)"
        );
        assert_eq!(
            badge(Some(&run), Some(&total)).as_deref(),
            Some("$0.83 run · $4.20 12 mo")
        );
        assert_eq!(badge(None, Some(&total)).as_deref(), Some("$4.20 12 mo"));
    }

    #[test]
    fn a_total_is_lifetime_only_when_billing_began_after_the_first_day_of_the_window() {
        let running = worker(&json!({"lastStartedAt": "2024-07-12T09:00:00Z"}));
        let now = at("2024-07-12T19:30:00Z");
        let lifetime = |from: &str| total(&history(), at(from), &running, now).excludes_before;
        assert_eq!(lifetime("2023-07-14T00:00:00Z"), None);
        assert_eq!(
            lifetime("2024-07-09T00:00:00Z"),
            None,
            "the first charge is a day later"
        );
        assert_eq!(lifetime("2024-07-10T00:00:00Z"), Some(at("2024-07-10T00:00:00Z")));
        assert_eq!(
            lifetime("2024-07-09T12:00:00Z"),
            Some(at("2024-07-09T12:00:00Z")),
            "a charge within a day of the window start may continue before it"
        );
        let windowed = total(&history(), at("2024-07-10T00:00:00Z"), &running, now);
        assert_total(windowed, 18.38, 0.345, Some("2024-07-12T19:00:00Z"));
        let stopped = worker(&json!({"desiredStatus": "EXITED"}));
        for from in ["2023-07-14T00:00:00Z", "2024-07-10T00:00:00Z"] {
            let empty = total(&[], at(from), &stopped, now);
            assert_eq!(
                empty.excludes_before, None,
                "no billing in the window means none before it"
            );
        }
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
