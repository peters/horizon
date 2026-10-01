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
        let mut newer_driver_view = RemoteOrientationView::default();
        for rejection in &rejections.completed {
            newer_driver_view.record_completion(rejection.clone());
        }
        assert_eq!(
            newer_driver_view.completed, view.completed,
            "later driver events cannot lose a queue refusal"
        );
    }
}
