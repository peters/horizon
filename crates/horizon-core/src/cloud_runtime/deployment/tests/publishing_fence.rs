use super::*;

#[test]
#[cfg_attr(windows, ignore = "Cloud records require Unix directory durability")]
fn a_retained_publisher_requires_its_original_grant_only_when_a_new_image_is_needed() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let mut state: Deployment = serde_json::from_value(serde_json::json!({
        "version":5,"cloud_id":"publishing-fence","repository":root,"revision":"a".repeat(40),
        "profile":{"provider":"runpod","image":"registry.example/team/worker","cpu":2,"memory_gb":4,
            "build":{"context":".","dockerfile":"Dockerfile"}},
        "publication":{"profile":"cpu","requested_image":"","destination":{
            "repository":"registry.example/team/worker","publisher":{"kind":"registry","binding":"synthetic-grant"}}},
        "stage":"Validate","operation":{"state":"prepared"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file":root.join("missing-key"),"ssh_identity_file":root.join("missing-identity"),
        "docker_config":root.join("absent-docker"),"cpu_flavors":[],"gpu_types":[]
    }))
    .unwrap();
    let request = Request::new(
        state.cloud_id.clone(),
        state.repository.clone(),
        state.revision.clone(),
        state.profile.clone(),
        root.into(),
        settings,
    );
    let store = Store::lock(root).unwrap();
    store.save(&state).unwrap();
    let path = root.join("deployment.json");
    let before = std::fs::read(&path).unwrap();
    let cancel = Cancellation::default();
    let emit = |_| {};
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    assert!(matches!(
        image::Preparation::new(&request, &store, &mut state, &runner),
        Err(Error::Invalid(message)) if message.contains("publishing and read-only pull credentials")
    ));
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert!(!root.join("absent-docker").exists());
    state.spec = Some(WorkerSpec {
        exact_placement: false,
        operation_id: state.cloud_id.clone(),
        profile: state.profile.clone(),
        image_digest: format!("registry.example/team/worker@sha256:{}", "a".repeat(64)),
        public_key: String::new(),
        registry_auth_id: None,
        gpu_types: Vec::new(),
        cpu_flavors: Vec::new(),
        data_centers: Vec::new(),
        startup_metadata: None,
    });
    assert!(
        image::Preparation::new(&request, &store, &mut state, &runner).is_ok(),
        "reusing an existing image does not require publishing credentials"
    );
}
