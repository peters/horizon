//! Queue refusal acknowledgements survive later worker presentation events.
use horizon_browser::BrowserCommand;
use horizon_browser_protocol::remote::{RemoteOrientationCompletion, RemoteOrientationView};

pub(super) fn forward(
    view: &mut RemoteOrientationView,
    rejections: &mut RemoteOrientationView,
    command: BrowserCommand,
    send: impl FnOnce(BrowserCommand) -> bool,
) {
    let orientation = match &command {
        BrowserCommand::Orientation { action_id, .. } => Some(action_id.clone()),
        _ => None,
    };
    if !send(command)
        && let Some(action_id) = orientation
    {
        let rejected = RemoteOrientationCompletion {
            action_id,
            error: Some("orientation_queue_rejected: worker driver is unavailable or busy".into()),
        };
        rejections.record_completion(rejected.clone());
        view.record_completion(rejected);
    }
}

// Reserve capacity for both sources so repeated history cannot starve new acknowledgements.
pub(super) fn merge(mut driver: RemoteOrientationView, rejections: &RemoteOrientationView) -> RemoteOrientationView {
    let limit = RemoteOrientationView::COMPLETION_LIMIT;
    let driver_count = driver
        .completed
        .len()
        .min(limit - rejections.completed.len().min(limit / 2));
    let rejection_count = rejections.completed.len().min(limit - driver_count);
    driver.completed.drain(..driver.completed.len() - driver_count);
    for completion in &rejections.completed[rejections.completed.len() - rejection_count..] {
        driver.record_completion(completion.clone());
    }
    driver
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_browser_protocol::remote::RemoteOrientation;
    #[test]
    fn rejected_worker_queue_returns_matching_nonfatal_completion() {
        let mut view = RemoteOrientationView::default();
        let mut rejections = RemoteOrientationView::default();
        forward(
            &mut view,
            &mut rejections,
            BrowserCommand::Orientation {
                action_id: "queued".into(),
                orientation: RemoteOrientation::Landscape,
            },
            |_| false,
        );
        assert_eq!(view.completed[0].action_id, "queued");
        assert!(
            view.completed[0]
                .error
                .as_deref()
                .unwrap()
                .contains("orientation_queue_rejected")
        );
        assert_eq!(view.completed, rejections.completed);
        let newer_driver_view = merge(RemoteOrientationView::default(), &rejections);
        assert_eq!(
            newer_driver_view.completed, view.completed,
            "later driver events cannot lose a queue refusal"
        );
    }
    fn completed(prefix: &str, count: usize, rejected: bool) -> RemoteOrientationView {
        let mut view = RemoteOrientationView::default();
        for index in 0..count {
            view.record_completion(RemoteOrientationCompletion {
                action_id: format!("{prefix}-{index}"),
                error: rejected.then(|| "orientation_queue_rejected".into()),
            });
        }
        view
    }

    #[test]
    fn saturated_sources_cannot_evict_each_others_fresh_completions() {
        let limit = RemoteOrientationView::COMPLETION_LIMIT;
        for (driver_count, rejection_count) in [(1, limit), (limit, 1), (limit, limit)] {
            let driver = completed("accepted", driver_count, false);
            let rejections = completed("rejected", rejection_count, true);
            for _ in 0..3 {
                let view = merge(driver.clone(), &rejections);
                assert_eq!(view.completed.len(), limit);
                assert!(view.completed.iter().any(|entry| {
                    entry.action_id == format!("accepted-{}", driver_count - 1) && entry.error.is_none()
                }));
                assert!(view.completed.iter().any(|entry| {
                    entry.action_id == format!("rejected-{}", rejection_count - 1) && entry.error.is_some()
                }));
            }
        }
    }

    #[test]
    fn unused_completion_capacity_remains_available_to_the_other_source() {
        let limit = RemoteOrientationView::COMPLETION_LIMIT;
        for (driver_count, rejection_count) in [(limit, 0), (0, limit), (2, 3)] {
            let driver = completed("accepted", driver_count, false);
            let rejections = completed("rejected", rejection_count, true);
            assert_eq!(
                merge(driver, &rejections).completed.len(),
                driver_count + rejection_count
            );
        }
    }
}
