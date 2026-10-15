use super::*;

const LEGACY: &str = "HORIZON_IDLE_STOP_MINUTES";

#[test]
fn no_worker_receives_an_idle_period() {
    let mut spec = spec();
    spec.profile.idle_stop_minutes = Some(30);
    assert!(create_body(&spec)["env"].get(LEGACY).is_none());
    spec.profile.gpu = true;
    assert!(create_body(&spec)["env"].get(LEGACY).is_none());
}

// A pod created before the agent stop carries its idle period; it still reconnects,
// whether or not its profile still sets one.
#[test]
fn an_existing_pod_that_carries_an_idle_period_still_verifies() {
    let verify = |spec: &WorkerSpec, env: Value| {
        let mut value = worker(spec);
        value["env"] = env;
        wire::worker(value).unwrap().verify(spec)
    };
    let mut spec = spec();
    for minutes in [None, Some(30)] {
        spec.profile.idle_stop_minutes = minutes;
        assert!(verify(&spec, json!({})).is_ok());
        assert!(verify(&spec, json!({LEGACY: "30"})).is_ok());
        assert!(verify(&spec, json!({LEGACY: "45"})).is_ok());
    }
    // The identity it carries still counts.
    assert!(verify(&spec, json!({LEGACY: "30", "HORIZON_CLOUD_OPERATION": "another"})).is_err());
}
