//! What a cloud costs: the header's one-line spend and the drawer's Cost tab. A
//! figure that is not known yet says why instead of reading "Unavailable".
use super::super::Runtime;
use super::strip::Spend;
use crate::theme;
use egui::{RichText, Sense, vec2};
use horizon_core::cloud_runtime::{self, billing::BillingBucket};
use std::time::{Instant, SystemTime};

/// Billing periods drawn in the Cost tab, newest last.
const BARS: usize = 24;

/// Whether the saved record describes the worker now. A deployment or redeploy keeps the
/// previous record (a deleted one, say) until it reaches Ready, so while one runs its
/// deletion state is history.
pub(super) fn record_is_current(runtime: &Runtime) -> bool {
    runtime.receiver.is_none() || runtime.progress.is_deletion()
}

/// A stopped or deleted worker bills no compute; its last rate would read as if it did.
pub(super) fn compute_idle(runtime: &Runtime) -> bool {
    matches!(
        runtime.stage,
        Some(super::super::Stage::Stopped | super::super::Stage::Deleted)
    ) || (runtime.receiver.is_none()
        // A saved stopped worker stays stopped until an operation actually runs, even when
        // a resume that failed its preflight moved the card's stage.
        && runtime.state.as_ref().is_some_and(|state| state.stage == super::super::Stage::Stopped))
        || (record_is_current(runtime)
            && runtime
                .state
                .as_ref()
                .is_some_and(|state| matches!(state.operation, cloud_runtime::CreateState::Terminated { .. })))
}

/// What is left of a deleted worker: nothing, or its workspace storage while cleanup is
/// unfinished (the provider confirmed the worker's deletion; the saved stage reads as before).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Deleted {
    Fully,
    StorageLeft,
}

fn worker_deleted(runtime: &Runtime) -> Option<Deleted> {
    if runtime.stage == Some(super::super::Stage::Deleted) {
        Some(Deleted::Fully)
    } else {
        (record_is_current(runtime)
            && runtime
                .state
                .as_ref()
                .is_some_and(|state| matches!(state.operation, cloud_runtime::CreateState::Terminated { .. })))
        .then_some(Deleted::StorageLeft)
    }
}

/// Whether a worker was requested, so the provider may bill it even before Horizon
/// has read its record.
pub(super) fn worker_requested(runtime: &Runtime) -> bool {
    runtime.state.as_ref().is_some_and(|state| {
        matches!(
            state.operation,
            cloud_runtime::CreateState::Requested | cloud_runtime::CreateState::Bound { .. }
        )
    }) || runtime.stage.is_some_and(|stage| {
        use super::super::Stage;
        matches!(
            stage,
            Stage::Provision | Stage::Readiness | Stage::Worktrees | Stage::Sessions
        )
    })
}

/// The Cost tab's one-word summary.
pub(super) fn teaser(runtime: &Runtime) -> String {
    let rate = runtime
        .state
        .as_ref()
        .and_then(|state| state.worker.as_ref())
        .and_then(cloud_runtime::cost::hourly_rate);
    // The worker's state first: it does not depend on whether a rate was ever reported.
    if worker_deleted(runtime).is_some() {
        return "deleted".into();
    }
    if compute_idle(runtime) {
        return "stopped".into();
    }
    rate.map_or_else(|| "—".into(), |rate| format!("${rate:.2}/h"))
}

pub(super) fn spend(runtime: &Runtime, now: SystemTime) -> Spend {
    if runtime.state_unavailable {
        return Spend {
            line: "Costs unknown".into(),
            explanation: "The deployment record could not be read, so Horizon cannot tell whether a worker is billing."
                .into(),
        };
    }
    let worker = runtime.state.as_ref().and_then(|state| state.worker.as_ref());
    let rate = worker.and_then(cloud_runtime::cost::hourly_rate);
    let run = runtime.current_run_cost(now);
    let total = runtime.total_cost(now);
    let idle = compute_idle(runtime);
    let deleted = worker_deleted(runtime);
    let mut parts = Vec::new();
    // A deleted worker bills no compute; its history can still follow.
    if let Some(deleted) = deleted {
        parts.push(match deleted {
            Deleted::Fully => "Nothing billing now".to_owned(),
            Deleted::StorageLeft => "Worker deleted · storage may still bill".to_owned(),
        });
    } else {
        match rate {
            Some(_) if idle => parts.push("compute stopped".to_owned()),
            Some(rate) => parts.push(cloud_runtime::cost::format_rate(rate)),
            None => {}
        }
    }
    if let Some(run) = &run {
        parts.push(format!("{} run", horizon_core::format_cost(run.amount)));
    }
    match &total {
        Some(total) if total.excludes_before.is_some() => {
            parts.push(format!("{} 12 mo", horizon_core::format_cost(total.total())));
        }
        Some(total) => parts.push(format!("{} total", horizon_core::format_cost(total.total()))),
        None if runtime.billing.error().is_some() => parts.push("total unavailable".into()),
        None => {}
    }
    let line = if !parts.is_empty() {
        parts.join(" · ")
    } else if idle {
        // Known stopped: missing rate data does not make the worker billable again.
        "compute stopped".to_owned()
    } else if worker.is_some() || worker_requested(runtime) {
        // Billing starts with the request, before the provider describes the worker.
        "Worker billing · rate pending".to_owned()
    } else {
        "No charges yet".to_owned()
    };
    let explanation = match &total {
        Some(total) => runtime.billing.explanation(total, Instant::now()),
        None => match runtime.billing.error() {
            Some(error) => format!("Provider billing unavailable: {error}. Horizon tries again every few minutes."),
            None if idle && (worker.is_some() || worker_requested(runtime)) => {
                "Compute is stopped; the provider has not reported billed periods yet.".into()
            }
            // The rate shown is the worker's own; only billed periods are still missing.
            None if rate.is_some() => {
                "The rate is the worker's last reported hourly price; the provider has not reported billed periods yet."
                    .into()
            }
            None if worker.is_some() || worker_requested(runtime) => {
                "A worker was requested; the provider has not reported its rate or billing yet.".into()
            }
            None => "Nothing is billed until a worker is requested.".into(),
        },
    };
    Spend { line, explanation }
}

/// The Cost tab's run note. The run is estimated once the worker is Ready; before that a
/// requested worker may already bill, so it is pending rather than "not running".
fn run_note(runtime: &Runtime, estimated: bool) -> &'static str {
    if estimated {
        "estimate since the last start"
    } else if !compute_idle(runtime) && worker_requested(runtime) {
        "estimate pending until ready"
    } else {
        "not running"
    }
}

/// The Cost tab's rate: a stopped or deleted worker's last rate is history, not what bills now.
fn rate_metric(runtime: &Runtime, rate: Option<f64>) -> (String, &'static str) {
    // The worker's state comes first: a stopped worker is stopped with or without a rate.
    match worker_deleted(runtime) {
        Some(Deleted::Fully) => return ("—".into(), "worker deleted; nothing billing"),
        Some(Deleted::StorageLeft) => return ("—".into(), "worker deleted; storage may still bill"),
        None => {}
    }
    if compute_idle(runtime) {
        return ("Stopped".into(), "compute stopped; storage may still bill");
    }
    match rate {
        Some(rate) => (cloud_runtime::cost::format_rate(rate), "last reported worker rate"),
        None if worker_requested(runtime) || runtime.state.as_ref().is_some_and(|state| state.worker.is_some()) => {
            ("—".into(), "rate pending")
        }
        None => ("—".into(), "known once a worker is requested"),
    }
}

pub(super) fn show(ui: &mut egui::Ui, runtime: &Runtime) {
    let now = SystemTime::now();
    let worker = runtime.state.as_ref().and_then(|state| state.worker.as_ref());
    let rate = worker.and_then(cloud_runtime::cost::hourly_rate);
    let run = runtime.current_run_cost(now);
    let total = runtime.total_cost(now);
    let (rate_value, rate_note) = rate_metric(runtime, rate);
    ui.columns(3, |columns| {
        metric(
            &mut columns[0],
            "Rate",
            &rate_value,
            rate_note,
            "Last reported worker hourly rate. Storage and other provider charges may be additional.",
        );
        metric(
            &mut columns[1],
            "This run",
            &run.as_ref()
                .map_or_else(|| "—".into(), |run| horizon_core::format_cost(run.amount)),
            run_note(runtime, run.is_some()),
            "Estimate from the last observed worker start and rate. Reconnect or check the provider to refresh worker state.",
        );
        let (title, value, note, hover) = match &total {
            Some(total) => (
                if total.excludes_before.is_some() {
                    "Past 12 months"
                } else {
                    "Since creation"
                },
                horizon_core::format_cost(total.total()),
                "billed + estimated",
                runtime.billing.explanation(total, Instant::now()),
            ),
            None if runtime.billing.error().is_some() => (
                "Since creation",
                "Unavailable".into(),
                "billing could not be read",
                runtime
                    .billing
                    .error()
                    .map_or_else(String::new, |error| format!("Provider billing unavailable: {error}")),
            ),
            None if runtime.billing.refreshing() => (
                "Since creation",
                "Reading…".into(),
                "reading provider billing",
                "Horizon is reading the provider's billing.".into(),
            ),
            None if runtime.billing.sample().is_some() => (
                "Since creation",
                "Awaiting billing".into(),
                "awaiting provider billing",
                "No billed periods have been reported. The current-run estimate does not establish a lifetime total."
                    .into(),
            ),
            None => (
                "Since creation",
                "—".into(),
                if worker.is_some() || worker_requested(runtime) { "billing not read yet" } else { "nothing billed yet" },
                "Provider billing has not been read yet.".into(),
            ),
        };
        metric(&mut columns[2], title, &value, note, &hover);
    });
    ui.add_space(6.0);
    super::worker_cost(ui, runtime, now);
    ui.add_space(6.0);
    if let Some(sample) = runtime.billing.sample() {
        ui.label(RichText::new("Billed per period").size(12.0).color(theme::FG_DIM()));
        bars(ui, &sample.history.buckets);
    }
    ui.add_space(8.0);
    ui.label(RichText::new(billing_note(runtime)).size(13.0).color(theme::FG_DIM()));
}

/// What still bills, for the state the worker is in.
fn billing_note(runtime: &Runtime) -> &'static str {
    match worker_deleted(runtime) {
        Some(Deleted::Fully) => return "The worker is deleted; it bills no compute. Past periods stay listed above.",
        Some(Deleted::StorageLeft) => {
            return "The worker is deleted, but its workspace storage may still bill until cleanup finishes in Manage.";
        }
        None => {}
    }
    if compute_idle(runtime) {
        "Compute is stopped. Storage kept for resuming may still bill until the cloud is deleted."
    } else {
        "Disconnecting does not stop compute or storage charges. Stop the worker to end compute charges."
    }
}

fn metric(ui: &mut egui::Ui, title: &str, value: &str, note: &str, hover: &str) {
    ui.add(egui::Label::new(RichText::new(title).size(12.0).color(theme::FG_DIM())).truncate());
    ui.add(egui::Label::new(RichText::new(value).size(22.0).color(theme::FG())).truncate())
        .on_hover_text(hover);
    ui.add(egui::Label::new(RichText::new(note).size(12.0).color(theme::FG_DIM())).truncate());
}

#[expect(clippy::cast_possible_truncation, reason = "a share in [0, 1] fits f32")]
fn ratio(part: f64, whole: f64) -> f32 {
    (part / whole).clamp(0.0, 1.0) as f32
}

/// The newest billed periods as bars, scaled to the largest.
fn bars(ui: &mut egui::Ui, buckets: &[BillingBucket]) {
    let shown = &buckets[buckets.len().saturating_sub(BARS)..];
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 90.0), Sense::hover());
    let painter = ui.painter();
    if shown.is_empty() {
        painter.text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            "No billed periods reported yet.",
            egui::FontId::proportional(13.0),
            theme::FG_DIM(),
        );
        return;
    }
    let largest = shown.iter().map(|bucket| bucket.amount).fold(0.0_f64, f64::max);
    let width = rect.width() / crate::app::util::usize_to_f32(BARS);
    let offset = BARS - shown.len();
    let mut hovered = None;
    for (index, bucket) in shown.iter().enumerate() {
        let share = if largest > 0.0 {
            ratio(bucket.amount, largest)
        } else {
            0.0
        };
        let height = (rect.height() * share).max(3.0);
        let left = rect.left() + crate::app::util::usize_to_f32(offset + index) * width;
        let bar = egui::Rect::from_min_max(
            egui::pos2(left + 2.0, rect.bottom() - height),
            egui::pos2(left + width - 2.0, rect.bottom()),
        );
        painter.rect_filled(
            bar,
            2,
            if bucket.amount > 0.0 {
                theme::blend(theme::ACCENT(), theme::PANEL_BG(), 0.25)
            } else {
                theme::BORDER_SUBTLE()
            },
        );
        if response
            .hover_pos()
            .is_some_and(|pointer| (left..left + width).contains(&pointer.x))
        {
            hovered = Some(bucket);
        }
    }
    if let Some(bucket) = hovered {
        response.on_hover_text_at_pointer(format!(
            "{} · {}",
            bucket.time,
            horizon_core::format_cost(bucket.amount)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::{Runtime, Stage};
    use super::*;

    fn runtime() -> Runtime {
        Runtime {
            stage: Some(Stage::Ready),
            state: Some(
                serde_json::from_value(serde_json::json!({
                    "version":1,"cloud_id":"billing-fixture","repository":"/synthetic","revision":"a",
                    "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
                    "stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
                    "spec":null,"sessions":[],"worker":{"id":"worker1","name":"billing fixture","imageName":"registry.example/worker","desiredStatus":"RUNNING",
                        "costPerHr":0.69,"lastStartedAt":"2024-07-12T19:14:40Z"}
                }))
                .unwrap(),
            ),
            ..Default::default()
        }
    }

    fn texts(runtime: &Runtime) -> Vec<String> {
        use crate::test_egui::DiscardTextures;
        egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| show(ui, runtime))
            .discard_textures()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn unreported_billing_is_never_a_zero_lifetime_total() {
        use cloud_runtime::billing::{BillingBucket, BucketSize, History};
        let mut runtime = runtime();
        runtime.billing.record(
            "worker1",
            Ok(History {
                buckets: Vec::new(),
                from: SystemTime::UNIX_EPOCH,
            }),
            Instant::now(),
        );
        let now = SystemTime::now();
        assert!(runtime.total_cost(now).is_none());
        let line = spend(&runtime, now).line;
        assert!(
            line.contains("$0.690/h") && line.contains("run") && !line.contains("total"),
            "{line}"
        );
        let shown = texts(&runtime);
        assert!(shown.iter().any(|text| text == "Awaiting billing"), "{shown:?}");
        assert!(shown.iter().any(|text| text == "No billed periods reported yet."));
        assert!(!shown.iter().any(|text| text == "Unavailable"));
        runtime.stage = Some(Stage::Stopped);
        let state = runtime.state.as_mut().unwrap();
        state.stage = Stage::Stopped;
        state.worker.as_mut().unwrap().desired_status = "EXITED".into();
        assert!(runtime.cost_badge(now).is_none());
        runtime.billing.record(
            "worker1",
            Ok(History {
                buckets: vec![BillingBucket {
                    time: "2024-07-12T19:00:00Z".into(),
                    size: BucketSize::Hour,
                    amount: 0.0,
                    time_billed_ms: 0,
                }],
                from: SystemTime::UNIX_EPOCH,
            }),
            Instant::now(),
        );
        assert_eq!(runtime.cost_badge(now).as_deref(), Some("$0.00 total"));
        assert!(spend(&runtime, now).line.ends_with("$0.00 total"));
    }

    #[test]
    fn a_cloud_without_a_worker_says_nothing_is_billed_instead_of_unavailable() {
        let runtime = Runtime::default();
        assert_eq!(spend(&runtime, SystemTime::now()).line, "No charges yet");
        let shown = texts(&runtime);
        assert!(shown.iter().any(|text| text == "known once a worker is requested"));
        assert!(shown.iter().any(|text| text == "not running"));
        assert!(shown.iter().any(|text| text == "nothing billed yet"));
        assert!(!shown.iter().any(|text| text.contains("Unavailable")), "{shown:?}");
    }

    #[test]
    fn a_requested_worker_is_never_called_free() {
        let runtime = Runtime {
            stage: Some(Stage::Readiness),
            ..Runtime::default()
        };
        assert_eq!(spend(&runtime, SystemTime::now()).line, "Worker billing · rate pending");
    }

    #[test]
    fn a_worker_starting_up_is_pending_not_idle_in_the_cost_tab() {
        let starting = Runtime {
            stage: Some(Stage::Readiness),
            ..runtime()
        };
        let shown = texts(&starting);
        assert!(
            shown.iter().any(|text| text == "estimate pending until ready"),
            "{shown:?}"
        );
        assert!(!shown.iter().any(|text| text == "not running"), "{shown:?}");
        let stopped = Runtime {
            stage: Some(Stage::Stopped),
            ..runtime()
        };
        assert!(texts(&stopped).iter().any(|text| text == "not running"));
    }

    #[test]
    fn a_deleted_cloud_does_not_claim_it_was_never_billed() {
        let runtime = Runtime {
            stage: Some(Stage::Deleted),
            ..Runtime::default()
        };
        assert_eq!(spend(&runtime, SystemTime::now()).line, "Nothing billing now");
    }

    #[test]
    fn deleted_and_stopped_workers_never_read_as_billing_compute() {
        let mut deleted = runtime();
        deleted.stage = Some(Stage::Deleted);
        let line = spend(&deleted, SystemTime::now()).line;
        assert!(line.starts_with("Nothing billing now"), "{line}");
        assert!(!line.contains("/h"), "no rate for a deleted worker: {line}");
        let stopped = Runtime {
            stage: Some(Stage::Stopped),
            state: Some(
                serde_json::from_value(serde_json::json!({
                    "version":1,"cloud_id":"stopped","repository":"/synthetic","revision":"a",
                    "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
                    "stage":"Stopped","operation":{"state":"bound","worker_id":"w"},"spec":null,"sessions":[],"worker":null
                }))
                .unwrap(),
            ),
            ..Runtime::default()
        };
        assert_eq!(spend(&stopped, SystemTime::now()).line, "compute stopped");
    }

    #[test]
    fn the_cost_tab_rate_is_not_a_stopped_or_deleted_workers_last_rate() {
        assert!(texts(&runtime()).iter().any(|text| text.ends_with("/h")));
        for (stage, wanted) in [
            (Stage::Stopped, "Stopped"),
            (Stage::Deleted, "worker deleted; nothing billing"),
        ] {
            let idle = Runtime {
                stage: Some(stage),
                ..runtime()
            };
            let shown = texts(&idle);
            assert!(shown.iter().any(|text| text == wanted), "{stage:?}: {shown:?}");
            assert!(!shown.iter().any(|text| text.contains("/h")), "{stage:?}: {shown:?}");
        }
    }

    #[test]
    fn a_worker_deleted_while_storage_cleanup_is_pending_says_storage_may_bill() {
        let mut pending_cleanup = runtime();
        let state = pending_cleanup.state.as_mut().unwrap();
        state.operation =
            serde_json::from_value(serde_json::json!({"state": "terminated", "worker_id": "worker1"})).unwrap();
        assert_eq!(
            pending_cleanup.stage,
            Some(Stage::Ready),
            "the saved stage still reads as before"
        );
        let line = spend(&pending_cleanup, SystemTime::now()).line;
        assert!(line.starts_with("Worker deleted · storage may still bill"), "{line}");
        let shown = texts(&pending_cleanup);
        assert!(
            shown
                .iter()
                .any(|text| text == "worker deleted; storage may still bill"),
            "{shown:?}"
        );
        assert!(
            !shown.iter().any(|text| text.contains("nothing billing")),
            "storage can still bill: {shown:?}"
        );
        assert!(!shown.iter().any(|text| text.contains("resuming")), "{shown:?}");
    }

    #[test]
    fn the_spend_explanation_never_contradicts_the_line_it_explains() {
        let running = runtime();
        let spend_now = spend(&running, SystemTime::now());
        assert!(spend_now.line.contains("/h"), "{}", spend_now.line);
        assert!(
            !spend_now.explanation.contains("not reported its rate"),
            "{}",
            spend_now.explanation
        );
        let stopped = Runtime {
            stage: Some(Stage::Stopped),
            ..runtime()
        };
        let spend_stopped = spend(&stopped, SystemTime::now());
        assert!(
            spend_stopped.line.starts_with("compute stopped"),
            "{}",
            spend_stopped.line
        );
        assert!(
            spend_stopped.explanation.starts_with("Compute is stopped"),
            "{}",
            spend_stopped.explanation
        );
    }

    #[test]
    fn a_stopped_worker_without_a_reported_rate_still_reads_as_stopped() {
        let mut stopped = Runtime {
            stage: Some(Stage::Stopped),
            ..runtime()
        };
        stopped.state.as_mut().unwrap().worker = None;
        let shown = texts(&stopped);
        assert!(shown.iter().any(|text| text == "Stopped"), "{shown:?}");
        assert!(
            !shown.iter().any(|text| text == "known once a worker is requested"),
            "{shown:?}"
        );
    }

    #[test]
    fn a_saved_stopped_worker_reads_as_stopped_after_a_resume_fails_its_preflight() {
        let mut failed_resume = Runtime {
            stage: Some(Stage::Provision),
            ..runtime()
        };
        failed_resume.state.as_mut().unwrap().stage = Stage::Stopped;
        let line = spend(&failed_resume, SystemTime::now()).line;
        assert!(line.starts_with("compute stopped"), "{line}");
        assert_eq!(teaser(&failed_resume), "stopped");
        failed_resume.state.as_mut().unwrap().worker = None;
        assert_eq!(teaser(&failed_resume), "stopped", "with or without a reported rate");
    }

    #[test]
    fn a_terminated_worker_stops_accruing_a_run() {
        let mut deleted = runtime();
        deleted.state.as_mut().unwrap().operation =
            serde_json::from_value(serde_json::json!({"state": "terminated", "worker_id": "worker1"})).unwrap();
        assert!(
            deleted.current_run_cost(SystemTime::now()).is_none(),
            "the worker is gone"
        );
        let line = spend(&deleted, SystemTime::now()).line;
        assert!(!line.contains(" run"), "{line}");
        assert!(
            runtime().current_run_cost(SystemTime::now()).is_some(),
            "a running worker still accrues"
        );
    }
}
