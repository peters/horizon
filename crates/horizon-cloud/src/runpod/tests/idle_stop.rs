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
