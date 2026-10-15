use super::*;

#[test]
fn a_deleted_clouds_node_cleanup_failure_is_not_called_a_redeploy_failure() {
    let runtime = Runtime {
        stage: Some(Stage::Deleted),
        operation: Some(super::super::super::Action::Delete),
        error: Some("Tailnet node n-exact remains; API is offline".into()),
        state: Some({
            let mut state = deployment(
                "Deleted",
                &serde_json::json!({"state":"terminated","worker_id":"worker"}),
                &serde_json::Value::Null,
            );
            state.tailnet_enrollment = serde_json::from_value(serde_json::json!({"receipts":[{
                "tailnet_id":"work","provider":"runpod","worker_id":"worker",
                "allocation_request_id":"status","device_id":"n-exact"
            }]}))
            .unwrap();
            state
        }),
        ..Runtime::default()
    };
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Tailnet node cleanup pending");
    assert!(status.numbers.contains("n-exact"));
    assert_eq!(status.primary, Some(Primary::Manage));
    let group = horizon_core::cloud_panel::CloudGroup::new(
        1,
        "Owned cloud".into(),
        "workspace".into(),
        "/synthetic".into(),
        [0.0, 0.0],
    );
    let (condition, line) = super::super::super::list::condition(&group, &runtime, &horizon_core::Board::new(), now());
    assert_eq!(condition, horizon_core::cloud_list::Condition::Attention);
    assert!(line.contains("Tailnet node cleanup pending") && line.contains("n-exact"));
}
