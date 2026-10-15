use super::*;
use crate::cloud_runtime::publishing::{self, Publication};

#[test]
fn a_rebuild_refuses_a_retained_publisher_before_legacy_credentials_are_loaded() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state.publication = Some(Box::new(Publication {
        profile: "dev".into(),
        requested_image: state.profile.image.clone(),
        destination: horizon_cloud::image_publishing::Destination::bound(
            state.profile.image.clone(),
            "synthetic-grant".into(),
        )
        .unwrap(),
        account: None,
    }));
    let store = fixture.store();
    store.save(&state).unwrap();
    let before = std::fs::read(fixture.root.join("deployment.json")).unwrap();
    let settings: Settings = serde_json::from_value(json!({
        "runpod_key_file":fixture.root.join("missing-key"),"ssh_identity_file":fixture.root.join("missing-identity"),
        "docker_config":fixture.root.join("absent-docker"),"cpu_flavors":[],"gpu_types":[]
    }))
    .unwrap();
    let request = Request::new(
        state.cloud_id.clone(),
        state.repository.clone(),
        state.revision.clone(),
        state.profile.clone(),
        fixture.root.clone(),
        settings,
    );
    let cancel = Cancellation::default();
    let emit = |_| {};
    let result = live::Live::new(&request, &store, &state, true, &cancel, &emit);
    assert!(matches!(
        result,
        Err(Error::Invalid(publishing::IMAGE_PREPARATION_STAGED))
    ));
    assert_eq!(std::fs::read(fixture.root.join("deployment.json")).unwrap(), before);
    assert!(!fixture.root.join("absent-docker").exists());
}
