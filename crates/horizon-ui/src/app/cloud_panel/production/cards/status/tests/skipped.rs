//! Steps an image-only cloud never runs.
use super::*;

#[test]
fn an_image_only_cloud_skips_build_and_push_instead_of_finishing_them() {
    // A deployment started here, before its record is saved.
    let (mut runtime, _sender) = live(Stage::Provision);
    runtime.launched_skips = &[Stage::Build, Stage::Push];
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.track.skipped, [Stage::Build, Stage::Push]);
    assert_eq!(status.track.current, Some(3));
    // Once saved, the record's profile decides, also after a restart.
    let mut runtime = Runtime {
        state: Some(deployment("Ready", &bound(), &running_worker())),
        ..Runtime::default()
    };
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.track.skipped, [Stage::Build, Stage::Push]);
    runtime.state.as_mut().unwrap().profile.build =
        serde_json::from_value(serde_json::json!({"context": ".", "dockerfile": "Dockerfile"})).unwrap();
    assert!(of(&runtime, Occupancy::default(), now()).track.skipped.is_empty());
}

#[test]
fn a_deletion_of_an_image_only_cloud_skips_no_step() {
    let (mut runtime, _sender) = live(Stage::ReleaseDevices);
    runtime.state = Some(deployment("Ready", &bound(), &running_worker()));
    runtime.progress.begin_deletion();
    runtime.progress.stage(Stage::ReleaseDevices, Instant::now());
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.track.stages, &Stage::DELETION);
    assert!(status.track.skipped.is_empty());
}
