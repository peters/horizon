use super::*;

#[test]
fn dedicated_workers_receive_their_idle_period() {
    let mut spec = spec();
    assert!(
        create_body(&spec)["env"]
            .get(crate::IDLE_STOP_ENVIRONMENT_KEY)
            .is_none()
    );
    spec.profile.idle_stop_minutes = Some(30);
    assert_eq!(create_body(&spec)["env"][crate::IDLE_STOP_ENVIRONMENT_KEY], "30");
    spec.profile.gpu = true;
    assert_eq!(create_body(&spec)["env"][crate::IDLE_STOP_ENVIRONMENT_KEY], "30");
}

#[test]
fn initialized_workers_never_receive_an_idle_period() {
    let mut spec = spec();
    spec.profile.idle_stop_minutes = Some(30);
    spec.startup_metadata = Some(crate::StartupMetadata::new("{}".into()).unwrap());
    assert!(
        create_body(&spec)["env"]
            .get(crate::IDLE_STOP_ENVIRONMENT_KEY)
            .is_none()
    );
}

#[test]
fn a_worker_must_report_exactly_its_recorded_idle_period() {
    let verify = |spec: &WorkerSpec, env: Value| {
        let mut value = worker(spec);
        value["env"] = env;
        serde_json::from_value::<Worker>(value).unwrap().verify(spec)
    };
    let mut spec = spec();
    assert!(verify(&spec, json!({})).is_ok());
    assert!(verify(&spec, json!({crate::IDLE_STOP_ENVIRONMENT_KEY: "30"})).is_err());
    spec.profile.idle_stop_minutes = Some(30);
    assert!(verify(&spec, json!({crate::IDLE_STOP_ENVIRONMENT_KEY: "30"})).is_ok());
    assert!(verify(&spec, json!({})).is_err());
    assert!(verify(&spec, json!({crate::IDLE_STOP_ENVIRONMENT_KEY: "45"})).is_err());
}
