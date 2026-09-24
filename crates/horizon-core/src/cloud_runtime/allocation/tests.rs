use super::*;
use serde_json::{Value, json};
use uuid::Uuid;

fn existing() -> ExistingWorker {
    let root = std::env::temp_dir();
    ExistingWorker::new(
        AllocationId::generate(),
        ControllerId::generate(),
        root.join("synthetic-provider-key"),
        root.join("synthetic-ssh-key"),
    )
    .unwrap()
}

#[test]
fn omitted_placement_keeps_dedicated_behavior_without_minting_identity() {
    let binding = PlacementBinding::parse(br#"{"version":1}"#).unwrap();
    assert_eq!(binding, PlacementBinding::default());
    assert_eq!(
        binding.placement(),
        &Placement::NewWorker {
            sharing: SharingMode::Dedicated
        }
    );
    assert_eq!(
        serde_json::to_value(binding).unwrap(),
        json!({"version":1,"placement":{"kind":"new_worker","sharing":"dedicated"}})
    );
}

#[test]
fn sharing_is_an_explicit_persisted_choice_and_unknown_modes_are_rejected() {
    let dedicated = PlacementBinding::parse(br#"{"version":1,"placement":{"kind":"new_worker"}}"#).unwrap();
    assert_eq!(dedicated, PlacementBinding::default());
    let shared = PlacementBinding::new(Placement::NewWorker {
        sharing: SharingMode::TrustedShared,
    });
    assert_eq!(
        PlacementBinding::parse(&serde_json::to_vec(&shared).unwrap()).unwrap(),
        shared
    );
    assert!(
        PlacementBinding::parse(br#"{"version":1,"placement":{"kind":"new_worker","sharing":"automatic"}}"#).is_err()
    );
    let mut existing = serde_json::to_value(PlacementBinding::new(Placement::ExistingWorker(existing()))).unwrap();
    existing["placement"]["sharing"] = json!("trusted_shared");
    assert!(PlacementBinding::parse(&serde_json::to_vec(&existing).unwrap()).is_err());
}

#[test]
fn malformed_existing_choices_never_fall_back_to_new_compute() {
    for value in [
        json!({}),
        json!({"version":0}),
        json!({"version":2}),
        json!({"version":1,"placement":null}),
        json!({"version":1,"placement":{"kind":"existing_worker"}}),
        json!({"version":1,"placement":{"kind":"automatic"}}),
        json!({"version":1,"placement":{"kind":"new_worker","allocation_id":"ignored"}}),
        json!({"version":1,"unexpected":"private-value"}),
    ] {
        let error = PlacementBinding::parse(&serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert_eq!(error, BindingError::Encoding);
        assert!(!error.to_string().contains("private-value"));
    }
}

#[test]
fn saved_placement_retains_allocation_controller_and_credential_bindings() {
    let worker = existing();
    let binding = PlacementBinding::new(Placement::ExistingWorker(worker.clone()));
    let saved = serde_json::to_vec(&binding).unwrap();
    assert_eq!(PlacementBinding::parse(&saved).unwrap(), binding);
    let Placement::ExistingWorker(restored) = binding.placement() else {
        panic!("existing placement lost");
    };
    assert_eq!(restored.allocation_id(), worker.allocation_id());
    assert_eq!(restored.controller_id(), worker.controller_id());
    assert_eq!(restored.provider_credential_file(), worker.provider_credential_file());
    assert_eq!(restored.ssh_identity_file(), worker.ssh_identity_file());
}

#[test]
fn relative_credentials_and_unknown_existing_fields_fail_on_restore() {
    let binding = PlacementBinding::new(Placement::ExistingWorker(existing()));
    for field in ["provider_credential_file", "ssh_identity_file"] {
        for relative in ["", "key", "../key"] {
            let mut value = serde_json::to_value(&binding).unwrap();
            value["placement"][field] = json!(relative);
            assert!(PlacementBinding::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }
    let mut value = serde_json::to_value(&binding).unwrap();
    value["placement"]["provider_id"] = json!("not-an-allocation-binding");
    assert!(PlacementBinding::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn allocation_and_project_ids_cannot_be_nil_or_paths() {
    for value in [Uuid::nil().to_string(), "../worker".into(), String::new()] {
        let encoded = serde_json::to_string(&value).unwrap();
        assert!(serde_json::from_str::<AllocationId>(&encoded).is_err());
        assert!(serde_json::from_str::<ProjectId>(&encoded).is_err());
        assert!(serde_json::from_str::<ControllerId>(&encoded).is_err());
    }
    let id = AllocationId::generate();
    assert_eq!(
        serde_json::from_value::<AllocationId>(json!(id.to_string())).unwrap(),
        id
    );
}

#[test]
fn copied_presentation_does_not_authorize_a_different_session_or_workspace() {
    let member = ProjectIdentity::new(
        ProjectId::generate(),
        "session".into(),
        "workspace".into(),
        "cloud".into(),
    )
    .unwrap();
    assert!(member.belongs_to("session", "workspace", "cloud"));
    for (session, workspace, cloud) in [
        ("copy", "workspace", "cloud"),
        ("session", "copy", "cloud"),
        ("session", "workspace", "copy"),
    ] {
        assert!(!member.belongs_to(session, workspace, cloud));
    }
    let restored: ProjectIdentity = serde_json::from_value(serde_json::to_value(&member).unwrap()).unwrap();
    assert_eq!(restored, member);
    assert_eq!(restored.project_id(), member.project_id());
}

#[test]
fn unsafe_membership_is_rejected_at_deserialization_not_only_construction() {
    let member = ProjectIdentity::new(
        ProjectId::generate(),
        "session".into(),
        "workspace".into(),
        "cloud".into(),
    )
    .unwrap();
    for field in ["session_id", "workspace_id", "cloud_id"] {
        for invalid in ["", "..", "a/b", "a\\b", "a:b", "a\n"] {
            let mut value = serde_json::to_value(&member).unwrap();
            value[field] = Value::String(invalid.into());
            assert!(serde_json::from_value::<ProjectIdentity>(value).is_err());
        }
    }
}
