use super::*;
#[test]
fn a_closing_cloud_shows_its_disposal_over_its_panels() {
    let mut group = CloudGroup::new(101, "test".into(), "ws".into(), ".".into(), [0.0, 0.0]);
    assert!(
        !view::body_visible(&group, true),
        "a local cloud has no disposal to show"
    );
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "test".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: serde_json::from_value(serde_json::json!({
            "provider": "runpod", "image": "example.invalid/worker", "cpu": 4, "memory_gb": 8,
        }))
        .unwrap(),
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    assert!(view::body_visible(&group, false), "an empty cloud shows its steps");
    group.panels.push("member".into());
    assert!(
        !view::body_visible(&group, false),
        "its panels take the place of the steps"
    );
    assert!(
        view::body_visible(&group, true),
        "a cloud being closed shows its disposal"
    );
    group.collapsed = true;
    assert!(!view::body_visible(&group, true), "a collapsed cloud shows nothing");
}
