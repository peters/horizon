use super::*;
use crate::browser::{
    BrowserDrainOutput, BrowserEvent, BrowserStatus,
    remote::{OrientationSupport, RemoteOrientationCompletion, RemoteOrientationState, RemoteOrientationView},
};
#[test]
fn runtime_failure_preserves_ready_panel_and_queue_rejection_is_not_pending() {
    let mut panel = BrowserPanelState::inert();
    panel.status = BrowserStatus::Ready;
    panel.request_orientation(RemoteOrientation::Landscape);
    assert!(panel.orientation.pending.is_none());
    assert!(panel.orientation.error.is_some());
    let mut output = BrowserDrainOutput::default();
    panel.apply_event(
        BrowserEvent::OrientationChanged(RemoteOrientationView {
            completed: Vec::new(),
            action_id: None,
            state: RemoteOrientationState {
                support: OrientationSupport::Supported,
                applied: None,
            },
            pending: None,
            error: Some("orientation_timeout: inspect before retrying".into()),
        }),
        &mut output,
    );
    assert!(matches!(panel.status, BrowserStatus::Ready));
    assert!(output.had_output);
    assert!(
        panel
            .orientation
            .error
            .as_deref()
            .unwrap()
            .contains("orientation_timeout")
    );
}
#[test]
fn lost_or_evicted_acknowledgement_cannot_leave_controls_pending_forever() {
    let mut panel = BrowserPanelState::inert();
    panel.orientation.action_id = Some("request-a".into());
    panel.orientation.pending = Some(RemoteOrientation::Landscape);
    panel.orientation_pending_since = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(30));
    assert!(panel.expire_orientation_pending());
    assert!(panel.orientation.pending.is_none());
    assert!(
        panel
            .orientation
            .error
            .as_deref()
            .unwrap()
            .contains("inspect before retrying")
    );
    let error = panel.orientation.error.clone();
    panel.apply_orientation_view(RemoteOrientationView::default());
    assert_eq!(panel.orientation.error, error);
    assert!(panel.orientation_pending_since.is_some());
    panel.apply_orientation_view(RemoteOrientationView {
        action_id: Some("request-a".into()),
        pending: Some(RemoteOrientation::Landscape),
        ..RemoteOrientationView::default()
    });
    assert!(
        panel.orientation.pending.is_none(),
        "stale pending polls cannot resurrect a lost acknowledgement"
    );
}

#[test]
fn matching_terminal_acknowledgement_settles_an_expired_request() {
    let mut panel = BrowserPanelState::inert();
    panel.orientation.action_id = Some("request-a".into());
    panel.orientation.pending = Some(RemoteOrientation::Landscape);
    panel.orientation_pending_since = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(30));
    assert!(panel.expire_orientation_pending());
    panel.apply_orientation_view(RemoteOrientationView {
        action_id: Some("unrelated".into()),
        ..RemoteOrientationView::default()
    });
    assert!(panel.orientation_pending_since.is_some());
    panel.apply_orientation_view(RemoteOrientationView {
        action_id: Some("request-a".into()),
        state: RemoteOrientationState {
            support: OrientationSupport::Supported,
            applied: Some(RemoteOrientation::Landscape),
        },
        ..RemoteOrientationView::default()
    });
    assert!(panel.orientation_pending_since.is_none());
    assert!(panel.orientation.error.is_none());
    assert_eq!(panel.orientation.state.applied, Some(RemoteOrientation::Landscape));
}

#[test]
fn expired_request_completion_settles_while_another_viewer_is_rotating() {
    let mut panel = BrowserPanelState::inert();
    panel.orientation.action_id = Some("request-a".into());
    panel.orientation.pending = Some(RemoteOrientation::Landscape);
    panel.orientation_pending_since = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(30));
    assert!(panel.expire_orientation_pending());
    panel.apply_orientation_view(RemoteOrientationView {
        action_id: Some("request-b".into()),
        pending: Some(RemoteOrientation::Portrait),
        completed: vec![RemoteOrientationCompletion {
            action_id: "request-a".into(),
            error: Some("orientation_superseded: inspect current orientation".into()),
        }],
        ..RemoteOrientationView::default()
    });
    assert!(panel.orientation_pending_since.is_none(), "request-a is terminal");
    assert_eq!(panel.orientation.pending, Some(RemoteOrientation::Portrait));
    assert!(
        panel
            .orientation
            .error
            .as_deref()
            .unwrap()
            .contains("orientation_superseded")
    );
    panel.apply_orientation_view(RemoteOrientationView {
        action_id: Some("request-b".into()),
        state: RemoteOrientationState {
            support: OrientationSupport::Supported,
            applied: Some(RemoteOrientation::Portrait),
        },
        ..RemoteOrientationView::default()
    });
    assert!(panel.orientation.pending.is_none());
    assert_eq!(panel.orientation.state.applied, Some(RemoteOrientation::Portrait));
}
