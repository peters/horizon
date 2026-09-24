use super::*;
use serde_json::json;

fn identity() -> ProjectIdentity {
    ProjectIdentity::new(
        ProjectId::generate(),
        "saved-session".into(),
        "workspace".into(),
        "legacy-cloud".into(),
    )
    .unwrap()
}

fn legacy() -> Value {
    let profile = json!({"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8});
    let value = json!({
        "version":1,"cloud_id":"legacy-cloud",
        "repository":std::env::temp_dir().join("synthetic-repository"),"revision":"committed-revision",
        "registry_generation":"saved-registry-generation",
        "profile":profile,"stage":"Stopping","operation":{"state":"bound","worker_id":"worker1"},
        "spec":{
            "operation_id":"legacy-cloud","image_digest":format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            "profile":profile,"public_key":"unchanged-public-key","registry_auth_id":"saved-pull-reference",
            "gpu_types":[],"cpu_flavors":["cpu3c"],"data_centers":["TEST-1"]
        },
        "worker":{
            "id":"worker1","name":"horizon-cloud-legacy-cloud","imageName":"saved-image","desiredStatus":"RUNNING",
            "publicIp":"127.0.0.1","portMappings":{"22":22022},"costPerHr":0.1,
            "memoryInGb":8,"vcpuCount":4,"gpuCount":0,"containerDiskInGb":20,
            "volumeInGb":20,"volumeMountPath":"/workspace",
            "networkVolume":{"id":"volume1","size":20,"dataCenterId":"TEST-1"},"env":{"SYNTHETIC":"retained"}
        },
        "sessions":[{"panel_id":"agent1","agent":"shell","tmux":"original-tmux", "branch":"original-branch", "worktree":"/workspace/original-worktree"}],
        "source_ready":true,"ready_after_seconds":123,"ready_history":"Observed",
        "stop_requested":true,"browserstack_released":false,"browserstack_targets":["owned-target"]
    });
    serde_json::to_value(serde_json::from_value::<Deployment>(value).unwrap()).unwrap()
}

fn convert(value: &Value) -> Result<Records, Error> {
    Records::from_legacy(
        &serde_json::to_vec(value).unwrap(),
        identity(),
        AllocationId::generate(),
        ControllerId::generate(),
    )
}

#[test]
fn complete_payload_survives_every_provider_state_without_new_session_identity() {
    for operation in [
        json!({"state":"prepared"}),
        json!({"state":"requested"}),
        json!({"state":"bound","worker_id":"worker1"}),
        json!({"state":"terminated","worker_id":"worker1"}),
    ] {
        for stage in [
            "Provision",
            "Readiness",
            "Sessions",
            "Ready",
            "Stopping",
            "Stopped",
            "Deleted",
        ] {
            let mut original = legacy();
            original["operation"] = operation.clone();
            original["stage"] = json!(stage);
            let pair = convert(&original).unwrap();
            let restored = Records::decode(&pair.allocation_bytes().unwrap(), &pair.project_bytes().unwrap()).unwrap();
            assert_eq!(serde_json::to_value(restored.deployment()).unwrap(), original);
            assert_eq!(restored.identity(), pair.identity());
            assert_eq!(restored.allocation_id(), pair.allocation_id());
            let allocation: Value = serde_json::from_slice(&pair.allocation_bytes().unwrap()).unwrap();
            assert_eq!(allocation["sharing"], "dedicated");
            assert_eq!(allocation["protocol"], "legacy_dedicated");
            assert_eq!(allocation["stop_requested"], true);
        }
    }
}

#[test]
fn preallocation_and_absent_legacy_optionals_do_not_invent_provider_facts() {
    let mut original = legacy();
    original["operation"] = json!({"state":"prepared"});
    original["spec"] = Value::Null;
    original["worker"] = Value::Null;
    original["sessions"] = json!([]);
    for field in [
        "registry_generation",
        "source_ready",
        "ready_after_seconds",
        "ready_history",
        "stop_requested",
        "browserstack_released",
        "browserstack_targets",
    ] {
        original.as_object_mut().unwrap().remove(field);
    }
    let expected: Deployment = serde_json::from_value(original.clone()).unwrap();
    let pair = convert(&original).unwrap();
    assert_eq!(
        serde_json::to_value(pair.deployment()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert!(pair.deployment().worker.is_none());
    assert_eq!(pair.deployment().operation, horizon_cloud::CreateState::Prepared);
}

#[test]
#[cfg(unix)]
fn readiness_history_matches_the_existing_loader_for_historical_records() {
    use crate::cloud_runtime::state::{ReadyHistory, Store};

    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    for stage in [
        "Provision",
        "Readiness",
        "Sessions",
        "Ready",
        "Stopping",
        "Stopped",
        "Deleted",
    ] {
        for history in [None, Some("Unobserved"), Some("Observed")] {
            let mut original = legacy();
            original["stage"] = json!(stage);
            original.as_object_mut().unwrap().remove("ready_history");
            if let Some(history) = history {
                original["ready_history"] = json!(history);
            }
            std::fs::write(
                root.path().join("deployment.json"),
                serde_json::to_vec(&original).unwrap(),
            )
            .unwrap();
            let expected = store.load().unwrap().unwrap();
            let pair = convert(&original).unwrap();
            let restored = Records::decode(&pair.allocation_bytes().unwrap(), &pair.project_bytes().unwrap()).unwrap();
            assert_eq!(
                serde_json::to_value(restored.deployment()).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
            if stage == "Ready" {
                assert_eq!(expected.ready_history, ReadyHistory::Observed);
            }
        }
    }
}

#[test]
fn unknown_source_fields_are_rejected_instead_of_dropping_cleanup_fences() {
    for pointer in [
        "",
        "/profile",
        "/spec",
        "/spec/profile",
        "/worker",
        "/worker/networkVolume",
        "/operation",
        "/sessions/0",
    ] {
        let mut original = legacy();
        original
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown_cleanup_fence".into(), json!(true));
        assert!(convert(&original).is_err(), "accepted unknown field at {pointer}");
    }
}

#[test]
fn caller_cannot_change_the_legacy_cloud_binding() {
    let mut original = legacy();
    original["cloud_id"] = json!("other-cloud");
    assert!(matches!(convert(&original), Err(Error::Ownership)));
    original["cloud_id"] = json!("legacy-cloud");
    original["version"] = json!(2);
    assert!(matches!(convert(&original), Err(Error::Version)));
}

#[test]
fn restored_records_reject_cross_project_pairing_and_changed_session_membership() {
    let original = convert(&legacy()).unwrap();
    let sibling = convert(&legacy()).unwrap();
    assert!(Records::decode(&original.allocation_bytes().unwrap(), &sibling.project_bytes().unwrap()).is_err());
    for field in ["session_id", "workspace_id", "cloud_id"] {
        let mut project: Value = serde_json::from_slice(&original.project_bytes().unwrap()).unwrap();
        project["identity"][field] = json!("another-owner");
        assert!(
            Records::decode(
                &original.allocation_bytes().unwrap(),
                &serde_json::to_vec(&project).unwrap()
            )
            .is_err()
        );
    }
}

#[test]
fn saved_records_reject_unsupported_versions_protocols_sharing_and_nested_fields() {
    let original = convert(&legacy()).unwrap();
    for (pointer, replacement) in [
        ("/version", json!(2)),
        ("/sharing", json!("trusted_shared")),
        ("/protocol", json!("shared_v1")),
        ("/controller", json!(uuid::Uuid::nil())),
    ] {
        let mut allocation: Value = serde_json::from_slice(&original.allocation_bytes().unwrap()).unwrap();
        *allocation.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            Records::decode(
                &serde_json::to_vec(&allocation).unwrap(),
                &original.project_bytes().unwrap()
            )
            .is_err()
        );
    }
    let mut project: Value = serde_json::from_slice(&original.project_bytes().unwrap()).unwrap();
    project["version"] = json!(1);
    assert!(
        Records::decode(
            &original.allocation_bytes().unwrap(),
            &serde_json::to_vec(&project).unwrap()
        )
        .is_err()
    );
    let mut allocation: Value = serde_json::from_slice(&original.allocation_bytes().unwrap()).unwrap();
    allocation["worker"]["unknown_fence"] = json!(true);
    assert!(
        Records::decode(
            &serde_json::to_vec(&allocation).unwrap(),
            &original.project_bytes().unwrap()
        )
        .is_err()
    );
}

#[test]
fn duplicate_cleanup_fields_and_private_parse_errors_do_not_get_normalized_away() {
    let serialized = serde_json::to_string(&legacy()).unwrap();
    let duplicate = format!("{{\"stop_requested\":false,{}", &serialized[1..]);
    let error = Records::from_legacy(
        duplicate.as_bytes(),
        identity(),
        AllocationId::generate(),
        ControllerId::generate(),
    )
    .unwrap_err();
    assert!(matches!(error, Error::Encoding));
    let error = Records::decode(b"private-value", b"{}").unwrap_err();
    assert!(!error.to_string().contains("private-value"));
}

#[test]
fn duplicate_keys_in_nested_maps_are_rejected_before_either_decode_path() {
    let original = legacy();
    let pair = convert(&original).unwrap();
    for (needle, duplicate) in [
        (
            r#""SYNTHETIC":"retained""#,
            r#""SYNTHETIC":"lost","SYNTHETIC":"retained""#,
        ),
        (r#""22":22022"#, r#""22":1,"22":22022"#),
    ] {
        let source = serde_json::to_string(&original).unwrap();
        assert!(source.contains(needle));
        let source = source.replace(needle, duplicate);
        assert!(
            Records::from_legacy(
                source.as_bytes(),
                identity(),
                AllocationId::generate(),
                ControllerId::generate()
            )
            .is_err()
        );
        let allocation: Value = serde_json::from_slice(&pair.allocation_bytes().unwrap()).unwrap();
        let source = serde_json::to_string(&allocation).unwrap();
        assert!(source.contains(needle));
        let source = source.replace(needle, duplicate);
        assert!(Records::decode(source.as_bytes(), &pair.project_bytes().unwrap()).is_err());
    }
    // Escaped spellings of the same key must not evade the duplicate check.
    assert!(unique_json::parse(br#"{"key":1,"\u006bey":2}"#).is_err());
}

#[test]
fn supported_legacy_spellings_and_normalized_values_preserve_the_existing_runtime_view() {
    for address in ["", "0:0:0:0:0:0:0:1"] {
        let mut original = legacy();
        let image = original["worker"].as_object_mut().unwrap().remove("imageName").unwrap();
        original["worker"]["image"] = image;
        original["worker"]["publicIp"] = json!(address);
        original["worker"]["costPerHr"] = json!("0.100");
        original["worker"]["adjustedCostPerHr"] = json!("0.090");
        original["profile"]["capabilities"]["browserstack"] = Value::Null;
        original["profile"]["capabilities"]["agents"] = json!(["grok", "claude", "codex"]);
        original["spec"]["profile"]["capabilities"]["browserstack"] = json!({"targets":[]});
        original["browserstack_targets"] = json!(["target-b", "target-a"]);
        let expected: Deployment = serde_json::from_value(original.clone()).unwrap();
        let pair = convert(&original).unwrap();
        assert_eq!(
            serde_json::to_value(pair.deployment()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        let mut allocation: Value = serde_json::from_slice(&pair.allocation_bytes().unwrap()).unwrap();
        allocation["worker"] = original["worker"].clone();
        let restored = Records::decode(
            &serde_json::to_vec(&allocation).unwrap(),
            &pair.project_bytes().unwrap(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(restored.deployment()).unwrap(),
            serde_json::to_value(pair.deployment()).unwrap()
        );
    }
}

#[test]
fn run_timing_survives_the_split_verbatim_and_saved_records_reencode_identically() {
    for timing in [
        None,
        Some(json!({"adjustedCostPerHr":0.09,"lastStartedAt":"2024-07-12T19:14:40.144Z"})),
        Some(json!({"lastStartedAt":"2024-07-12T15:14:40.1440-04:00"})),
    ] {
        let mut original = legacy();
        // A record saved before run timing existed has neither key.
        assert!(original["worker"].get("adjustedCostPerHr").is_none());
        assert!(original["worker"].get("lastStartedAt").is_none());
        for (key, value) in timing.iter().flat_map(|fields| fields.as_object().unwrap()) {
            original["worker"][key] = value.clone();
        }
        let pair = convert(&original).unwrap();
        let (allocation, project) = (pair.allocation_bytes().unwrap(), pair.project_bytes().unwrap());
        let restored = Records::decode(&allocation, &project).unwrap();
        assert_eq!(serde_json::to_value(restored.deployment()).unwrap(), original);
        assert_eq!(restored.allocation_bytes().unwrap(), allocation);
        assert_eq!(restored.project_bytes().unwrap(), project);
        let saved: Value = serde_json::from_slice(&allocation).unwrap();
        assert_eq!(saved["worker"]["lastStartedAt"], original["worker"]["lastStartedAt"]);
    }
}

#[test]
fn explicit_null_run_timing_splits_as_absent() {
    for key in ["adjustedCostPerHr", "lastStartedAt"] {
        let mut original = legacy();
        original["worker"][key] = Value::Null;
        let pair = convert(&original).unwrap();
        let saved: Value = serde_json::from_slice(&pair.allocation_bytes().unwrap()).unwrap();
        assert!(saved["worker"].get(key).is_none(), "{key} null is saved as absent");
        original["worker"].as_object_mut().unwrap().remove(key);
        assert_eq!(serde_json::to_value(pair.deployment()).unwrap(), original);
    }
}

#[test]
fn alias_conflicts_and_unknown_fields_inside_strict_profiles_are_rejected() {
    for image in ["saved-image", "different-image"] {
        let mut original = legacy();
        original["worker"]["image"] = json!(image);
        assert!(convert(&original).is_err());
    }
    for pointer in ["/profile/capabilities", "/spec/profile/storage", "/profile/bootstrap"] {
        let mut original = legacy();
        original
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown_cleanup_fence".into(), Value::Null);
        assert!(convert(&original).is_err());
    }
}
