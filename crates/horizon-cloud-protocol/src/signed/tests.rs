use super::*;
use crate::ProjectId;
use ring::rand::SystemRandom;
use serde_json::{Value, json};

fn fixture() -> (Ed25519KeyPair, ControllerBinding, Target) {
    let encoded = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let key = Ed25519KeyPair::from_pkcs8(encoded.as_ref()).unwrap();
    let binding = ControllerBinding::new(
        AllocationId::generate(),
        ControllerId::generate(),
        key.public_key().as_ref().try_into().unwrap(),
    );
    let target = Target::Project {
        identity: ProjectIdentity::new(
            ProjectId::generate(),
            "saved-session".into(),
            "workspace".into(),
            "cloud".into(),
        )
        .unwrap(),
    };
    (key, binding, target)
}

fn request(key: &Ed25519KeyPair, binding: &ControllerBinding, target: Target) -> SignedIntent {
    let intent = Intent::new(
        binding,
        OperationId::generate(),
        17,
        target,
        Action::AttachProject,
        b"synthetic-payload",
    )
    .unwrap();
    SignedIntent::sign(intent, binding, key).unwrap()
}

#[test]
fn signed_intent_roundtrips_without_private_material_and_requires_the_exact_payload() {
    let (key, binding, target) = fixture();
    let request = request(&key, &binding, target.clone());
    let bytes = serde_json::to_vec(&request).unwrap();
    let restored = SignedIntent::parse(&bytes).unwrap();
    let verified = restored.verify(&binding, b"synthetic-payload").unwrap();
    assert_eq!(verified.target(), &target);
    assert_eq!(verified.action(), Action::AttachProject);
    assert_eq!(verified.expected_revision(), 17);
    assert_eq!(verified.operation(), request.intent.operation());
    assert_eq!(
        restored.verify(&binding, b"different-payload").unwrap_err(),
        Error::Fingerprint
    );
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 2);
    assert!(value.get("intent").is_some());
    assert_eq!(value["signature"].as_array().unwrap().len(), 64);
}

#[test]
fn every_routing_replay_and_concurrency_field_is_authenticated() {
    let (key, binding, target) = fixture();
    let request = request(&key, &binding, target);
    let original = serde_json::to_value(&request).unwrap();
    for (pointer, replacement) in [
        ("/intent/version", json!(2)),
        ("/intent/allocation", json!(AllocationId::generate())),
        ("/intent/controller", json!(ControllerId::generate())),
        ("/intent/operation", json!(OperationId::generate())),
        ("/intent/expected_revision", json!(18)),
        ("/intent/action", json!("remove_project")),
        ("/intent/target/identity/project_id", json!(ProjectId::generate())),
        ("/intent/target/identity/session_id", json!("other-session")),
        ("/intent/target/identity/workspace_id", json!("other-workspace")),
        ("/intent/target/identity/cloud_id", json!("other-cloud")),
        (
            "/intent/payload_hash/0",
            json!((u16::from(request.intent.payload_hash[0]) + 1) % 256),
        ),
        ("/signature/0", json!((u16::from(request.signature[0]) + 1) % 256)),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        let restored = SignedIntent::parse(&serde_json::to_vec(&changed).unwrap()).unwrap();
        if pointer.starts_with("/intent/") {
            assert_ne!(
                restored.intent.fingerprint().unwrap(),
                request.intent.fingerprint().unwrap()
            );
        }
        assert!(
            restored.verify(&binding, b"synthetic-payload").is_err(),
            "accepted {pointer}"
        );
    }
}

#[test]
fn version_one_encoding_and_domain_are_fixed_and_wire_order_is_immaterial() {
    let allocation = uuid::Uuid::from_u128(1).try_into().unwrap();
    let controller = uuid::Uuid::from_u128(2).try_into().unwrap();
    let operation = uuid::Uuid::from_u128(3).try_into().unwrap();
    let (key, _, _) = fixture();
    let binding = ControllerBinding::new(allocation, controller, key.public_key().as_ref().try_into().unwrap());
    let intent = Intent::new(
        &binding,
        operation,
        4,
        Target::Allocation {},
        Action::InspectAllocation,
        b"",
    )
    .unwrap();
    let expected = concat!(
        "horizon-cloud-management-v1\0",
        r#"{"version":1,"allocation":"00000000-0000-0000-0000-000000000001","controller":"00000000-0000-0000-0000-000000000002","target":{"scope":"allocation"},"operation":"00000000-0000-0000-0000-000000000003","expected_revision":4,"action":"inspect_allocation","payload_hash":[227,176,196,66,152,252,28,20,154,251,244,200,153,111,185,36,39,174,65,228,100,155,147,76,164,149,153,27,120,82,184,85]}"#,
    );
    assert_eq!(intent.signing_bytes().unwrap(), expected.as_bytes());
    let request = SignedIntent::sign(intent, &binding, &key).unwrap();
    let sorted = serde_json::to_value(&request).unwrap();
    let restored = SignedIntent::parse(&serde_json::to_vec_pretty(&sorted).unwrap()).unwrap();
    assert_eq!(restored.verify(&binding, b"").unwrap(), &request.intent);
    for addition in [
        r#","identity":{"project_id":"00000000-0000-0000-0000-000000000005"}"#,
        r#","unknown":"private-value""#,
        r#","scope":"allocation""#,
    ] {
        let wire = serde_json::to_string(&request).unwrap().replace(
            r#""target":{"scope":"allocation"}"#,
            &format!(r#""target":{{"scope":"allocation"{addition}}}"#),
        );
        assert_eq!(SignedIntent::parse(wire.as_bytes()).unwrap_err(), Error::Encoding);
    }
    let without_domain = SignedIntent {
        signature: key
            .sign(&serde_json::to_vec(&request.intent).unwrap())
            .as_ref()
            .to_vec(),
        intent: request.intent,
    };
    assert_eq!(without_domain.verify(&binding, b"").unwrap_err(), Error::Signature);
    assert!(serde_json::from_str::<OperationId>("\"00000000-0000-0000-0000-000000000000\"").is_err());
}

#[test]
fn a_different_signing_key_cannot_impersonate_the_pinned_controller() {
    let (key, binding, target) = fixture();
    let (foreign, _, _) = fixture();
    let request = request(&key, &binding, target.clone());
    let wrong_key = ControllerBinding::new(
        binding.allocation,
        binding.controller,
        foreign.public_key().as_ref().try_into().unwrap(),
    );
    assert_eq!(
        request.verify(&wrong_key, b"synthetic-payload").unwrap_err(),
        Error::Signature
    );
    let intent = Intent::new(
        &binding,
        OperationId::generate(),
        0,
        target,
        Action::AttachProject,
        b"synthetic-payload",
    )
    .unwrap();
    assert_eq!(
        SignedIntent::sign(intent, &binding, &foreign).unwrap_err(),
        Error::Controller
    );
}

#[test]
fn actions_cannot_change_between_project_and_allocation_scope() {
    let (_, binding, target) = fixture();
    for action in [
        Action::Bootstrap,
        Action::InspectAllocation,
        Action::PrepareWorkerTransition,
        Action::ConfirmWorkerTransition,
        Action::ReconcileWorkerTransition,
        Action::CancelWorkerTransition,
        Action::ResumeWorker,
    ] {
        assert!(
            Intent::new(
                &binding,
                OperationId::generate(),
                0,
                Target::Allocation {},
                action,
                b"{}"
            )
            .is_ok()
        );
        assert_eq!(
            Intent::new(&binding, OperationId::generate(), 0, target.clone(), action, b"{}").unwrap_err(),
            Error::Scope
        );
    }
    for action in [
        Action::AttachProject,
        Action::InspectProject,
        Action::ReconcileProject,
        Action::StopProjectSessions,
        Action::RemoveProject,
        Action::PrepareProjectDataDeletion,
        Action::ConfirmProjectDataDeletion,
        Action::ReconcileProjectDataDeletion,
    ] {
        assert!(Intent::new(&binding, OperationId::generate(), 0, target.clone(), action, b"{}").is_ok());
        assert_eq!(
            Intent::new(
                &binding,
                OperationId::generate(),
                0,
                Target::Allocation {},
                action,
                b"{}"
            )
            .unwrap_err(),
            Error::Scope
        );
    }
}

#[test]
fn unsupported_encoding_and_unrecognized_fields_fail_without_disclosing_values() {
    let (key, binding, target) = fixture();
    let request = request(&key, &binding, target);
    let original = serde_json::to_value(&request).unwrap();
    for pointer in ["", "/intent", "/intent/target", "/intent/target/identity"] {
        let mut value = original.clone();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), json!("private-value"));
        let error = SignedIntent::parse(&serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert_eq!(error, Error::Encoding);
        assert!(!error.to_string().contains("private-value"));
    }
    for bytes in [b"not-json".to_vec(), vec![b' '; MAX_MESSAGE_BYTES + 1]] {
        assert_eq!(SignedIntent::parse(&bytes).unwrap_err(), Error::Encoding);
    }
}

#[test]
fn oversized_payloads_and_invalid_signature_lengths_are_rejected() {
    let (key, binding, target) = fixture();
    let payload = vec![0; MAX_PAYLOAD_BYTES + 1];
    assert_eq!(
        Intent::new(
            &binding,
            OperationId::generate(),
            0,
            target.clone(),
            Action::AttachProject,
            &payload
        )
        .unwrap_err(),
        Error::Encoding
    );
    let mut request = request(&key, &binding, target);
    assert_eq!(request.verify(&binding, &payload).unwrap_err(), Error::Encoding);
    for signature in [Vec::new(), vec![0; 63], vec![0; 65]] {
        request.signature = signature;
        assert_eq!(
            request.verify(&binding, b"synthetic-payload").unwrap_err(),
            Error::Signature
        );
    }
}
