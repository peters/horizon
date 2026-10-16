use super::super::service_admission::OwnerScope;
use super::*;
use crate::cloud_panel::{CloudLaunch, Placement};
use crate::cloud_runtime::ssh::Connection;
use horizon_cloud::WorkerSpec;
use horizon_cloud_protocol::exposure::{ConnectionLimits, SourceRule};
use serde_json::json;

fn retained(cloud: &str, worker: &str, owner: Option<OwnerScope>) -> Record {
    let owner = owner.unwrap_or_else(|| {
        OwnerScope::new(
            &Uuid::new_v4().to_string(),
            &Uuid::new_v4().to_string(),
            "workspace",
            cloud,
        )
        .unwrap()
    });
    let profile: horizon_cloud::Profile = serde_json::from_value(json!({
        "provider":"hetzner", "image":"registry.example/worker", "cpu":4, "memory_gb":8
    }))
    .unwrap();
    let binding = Binding {
        owner,
        launch: CloudLaunch {
            publishing_selection: None,
            deployment_started: true,
            id: cloud.into(),
            revision: "a".repeat(40),
            profile_name: "cpu".into(),
            profile: profile.clone(),
            placement: Placement::default(),
        },
        spec: WorkerSpec {
            operation_id: "allocation-a".into(),
            image_digest: format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            profile,
            public_key: "synthetic-public-key".into(),
            registry_auth_id: None,
            gpu_types: vec![],
            cpu_flavors: vec![],
            data_centers: vec![],
            exact_placement: false,
            startup_metadata: None,
        },
        worker_id: worker.into(),
        pinned_ssh_args: Connection {
            host: "192.0.2.5".into(),
            port: 22,
            identity: std::env::temp_dir().join("synthetic-identity"),
            known_hosts: std::env::temp_dir().join("synthetic-known-hosts"),
            host_key_alias: "synthetic-worker".into(),
            proxy_command: None,
        }
        .pinned_args(),
        provider_credential: std::env::temp_dir().join("synthetic-provider-reference").into(),
    };
    Record::starting(
        binding,
        Uuid::new_v4(),
        Service {
            id: Uuid::new_v4(),
            policy: ExposureRequest {
                mode: ExposureMode::Restricted,
                target_port: 8123,
                sources: vec![SourceRule::Cidr("192.0.2.0/24".into())],
                limits: ConnectionLimits::default(),
            },
            route: Route::Gate {},
        },
        Uuid::new_v4(),
        1_000,
    )
    .unwrap()
}
#[test]
fn renewal_changes_only_the_selected_service_after_explicit_new_operation() {
    let mut record = retained("exposure-cloud", "worker-a", None);
    let epoch = record.epoch();
    let custody = Uuid::new_v4();
    let first = record.intents()[0].clone();
    record.observed(epoch, custody, 2_000).unwrap();
    assert!(
        record
            .renewing(epoch, custody, first.service.id, Uuid::new_v4(), 3_000)
            .is_err()
    );
    record.active(epoch, custody, first.operation, 20000, 2_000).unwrap();
    let mut second = first.service.clone();
    second.id = Uuid::new_v4();
    let second_operation = Uuid::new_v4();
    record.add(epoch, custody, second, second_operation, 3_000).unwrap();
    let before_conflict = record.clone();
    assert!(record.active(epoch, custody, second_operation, 20000, 3_000).is_err());
    assert_eq!(record, before_conflict);
    record.active(epoch, custody, second_operation, 20001, 3_000).unwrap();
    let second_before = record.intents()[1].clone();
    let renewed = Uuid::new_v4();
    record
        .renewing(epoch, custody, first.service.id, renewed, 4_000)
        .unwrap();
    let pending = record.clone();
    assert!(
        record
            .renewing(epoch, custody, first.service.id, renewed, 5_000)
            .is_err()
    );
    assert_eq!(record, pending);
    assert!(record.active(epoch, custody, first.operation, 20000, 5_000).is_err());
    record.active(epoch, custody, renewed, 20000, 5_000).unwrap();
    assert_eq!(record.intents()[0].expires_at_millis, 4_000 + MAX_LIFETIME_MILLIS);
    assert_eq!(record.intents()[1], second_before);
    let before = record.clone();
    for now in [4_999, record.intents()[0].expires_at_millis, i64::MAX] {
        assert!(
            record
                .renewing(epoch, custody, first.service.id, Uuid::new_v4(), now)
                .is_err()
        );
        assert_eq!(record, before);
    }
}

#[test]
fn close_acknowledgement_cannot_reuse_open_or_renew_operation_or_another_service_port() {
    let mut record = retained("exposure-cloud", "worker-a", None);
    let epoch = record.epoch();
    let custody = Uuid::new_v4();
    let first = record.intents()[0].clone();
    record.observed(epoch, custody, 2_000).unwrap();
    record.active(epoch, custody, first.operation, 20000, 2_000).unwrap();
    let close = Uuid::new_v4();
    record.closing(epoch, custody, first.service.id, close).unwrap();
    let pending = record.clone();
    for (e, c, o, p) in [
        (Uuid::new_v4(), custody, close, 20000),
        (epoch, Uuid::new_v4(), close, 20000),
        (epoch, custody, first.operation, 20000),
        (epoch, custody, close, 20001),
    ] {
        assert!(record.service_closed(e, c, o, p).is_err());
        assert_eq!(record, pending);
    }
    record.service_closed(epoch, custody, close, 20000).unwrap();
    assert_eq!(record.intents()[0].phase, Phase::Closed { ingress: Some(20000) });
    assert_eq!(record.intents()[0].expires_at_millis, first.expires_at_millis);
}

#[test]
fn finish_requires_exact_saved_operation_and_unknown_never_reopens() {
    let mut record = retained("exposure-cloud", "worker-a", None);
    let epoch = record.epoch();
    let custody = Uuid::new_v4();
    let operation = Uuid::new_v4();
    record.observed(epoch, custody, 2_000).unwrap();
    assert!(record.finished(epoch, custody, operation).is_err());
    record.uncertain(epoch).unwrap();
    let uncertain = record.clone();
    assert!(record.observed(epoch, custody, 3_000).is_err());
    assert!(
        record
            .add(
                epoch,
                custody,
                record.intents()[0].service.clone(),
                Uuid::new_v4(),
                3_000
            )
            .is_err()
    );
    assert_eq!(record, uncertain);
    record.finishing(epoch, custody, operation).unwrap();
    let pending = record.clone();
    assert!(
        record
            .intents()
            .iter()
            .all(|intent| matches!(intent.phase, Phase::Closing { .. }))
    );
    for (e, c, o) in [
        (Uuid::new_v4(), custody, operation),
        (epoch, Uuid::new_v4(), operation),
        (epoch, custody, Uuid::new_v4()),
    ] {
        assert!(record.finished(e, c, o).is_err());
        assert_eq!(record, pending);
    }
    record.uncertain(epoch).unwrap();
    assert_eq!(record.startup(), Startup::FinishUnknown { custody, operation });
    let uncertain_finish = record.clone();
    assert!(record.finishing(epoch, custody, Uuid::new_v4()).is_err());
    assert!(record.finished(epoch, custody, Uuid::new_v4()).is_err());
    assert_eq!(record, uncertain_finish);
    record.finished(epoch, custody, operation).unwrap();
    let closed = record.clone();
    assert!(record.uncertain(epoch).is_err());
    assert!(record.observed(epoch, custody, 4_000).is_err());
    assert_eq!(record, closed);
    assert_eq!(
        serde_json::from_slice::<Record>(&serde_json::to_vec(&record).unwrap()).unwrap(),
        record
    );
}

#[test]
fn corrupt_phase_deadline_nested_binding_and_unknown_custody_cannot_drop_history() {
    let record = retained("exposure-cloud", "worker-a", None);
    let original = serde_json::to_value(&record).unwrap();
    for (path, value) in [
        ("phase", json!({"state":"active","ingress":20000})),
        ("phase", json!({"state":"opening","approved":true})),
        ("expires_at_millis", json!(1_000 + MAX_LIFETIME_MILLIS + 1)),
        ("operation", json!(Uuid::nil())),
    ] {
        let mut value_record = original.clone();
        value_record["intents"][0][path] = value;
        assert!(serde_json::from_value::<Record>(value_record).is_err());
    }
    let mut value = original.clone();
    value["startup"] = json!({"state":"closed","custody":Uuid::new_v4()});
    assert!(serde_json::from_value::<Record>(value).is_err());
    let mut value = original;
    value["binding"]["launch"]["new_unknown_contract_field"] = true.into();
    assert!(serde_json::from_value::<Record>(value).is_err());
    let encoded = serde_json::to_string(&record).unwrap();
    let duplicate = encoded.replacen(
        "\"provider\":\"hetzner\"",
        "\"provider\":\"runpod\",\"provider\":\"hetzner\"",
        1,
    );
    assert_ne!(encoded, duplicate);
    assert!(serde_json::from_str::<Record>(&duplicate).is_err());
    let mut unknown = record;
    unknown.uncertain(unknown.epoch()).unwrap();
    assert!(
        unknown
            .finishing(unknown.epoch(), Uuid::new_v4(), Uuid::new_v4())
            .is_err()
    );
}

#[test]
fn bounded_service_history_and_shared_management_policy_cannot_be_bypassed_by_route_facts() {
    let original = retained("exposure-cloud", "worker-a", None);
    let service = original.intents()[0].service.clone();
    for port in [
        0, 22, 5900, 9222, 2375, 1055, 1056, 47280, 47281, 20000, 20031, 20032, 20063,
    ] {
        let mut invalid = service.clone();
        invalid.policy.target_port = port;
        assert!(
            Record::starting(
                original.binding().clone(),
                Uuid::new_v4(),
                invalid,
                Uuid::new_v4(),
                1000
            )
            .is_err()
        );
    }
    let mut funnel = service.clone();
    funnel.route = Route::FunnelHttps { https: 443 };
    assert!(
        Record::starting(
            original.binding().clone(),
            Uuid::new_v4(),
            funnel.clone(),
            Uuid::new_v4(),
            1000
        )
        .is_err()
    );
    funnel.policy.mode = ExposureMode::Public;
    funnel.policy.sources.clear();
    assert!(
        Record::starting(
            original.binding().clone(),
            Uuid::new_v4(),
            funnel.clone(),
            Uuid::new_v4(),
            1000
        )
        .is_ok()
    );
    funnel.route = Route::FunnelHttps { https: 444 };
    assert!(Record::starting(original.binding().clone(), Uuid::new_v4(), funnel, Uuid::new_v4(), 1000).is_err());
    let mut record = original;
    let epoch = record.epoch();
    let custody = Uuid::new_v4();
    record.observed(epoch, custody, 1000).unwrap();
    for _ in 1..MAX_SERVICES {
        let mut next = service.clone();
        next.id = Uuid::new_v4();
        record.add(epoch, custody, next, Uuid::new_v4(), 1000).unwrap();
    }
    let before = record.clone();
    let mut extra = service;
    extra.id = Uuid::new_v4();
    assert!(record.add(epoch, custody, extra, Uuid::new_v4(), 1000).is_err());
    assert_eq!(record, before);
}

#[test]
fn raw_provider_progress_cannot_overwrite_duplicate_or_aliased_map_keys() {
    let mut record = retained("exposure-cloud", "worker-a", None);
    let service = record.intents()[0].service.id;
    record.provider = Some(ProviderProgress {
        epoch: record.epoch(),
        phase: ProviderPhase::Observed {},
        firewall_id: Some(7),
        rules_json: "[]".into(),
        foreign: [(5, "a".repeat(64))].into(),
        action_ids: vec![],
        services: [(service, 20000)].into(),
    });
    let encoded = serde_json::to_string(&record).unwrap();
    assert_eq!(serde_json::from_str::<Record>(&encoded).unwrap(), record);
    for (original, replacement) in [
        (
            format!("\"{service}\":20000"),
            format!("\"{service}\":20000,\"{service}\":20001"),
        ),
        (
            format!("\"5\":\"{}\"", "a".repeat(64)),
            format!("\"5\":\"{}\",\"5\":\"{}\"", "b".repeat(64), "a".repeat(64)),
        ),
        (
            format!("\"5\":\"{}\"", "a".repeat(64)),
            format!("\"05\":\"{}\",\"5\":\"{}\"", "b".repeat(64), "a".repeat(64)),
        ),
    ] {
        let duplicate = encoded.replacen(&original, &replacement, 1);
        assert_ne!(duplicate, encoded);
        assert!(serde_json::from_str::<Record>(&duplicate).is_err());
    }
}

#[test]
fn decoded_finish_and_finish_unknown_cannot_reuse_a_retained_service_operation() {
    let mut record = retained("exposure-cloud", "worker-a", None);
    let epoch = record.epoch();
    let custody = Uuid::new_v4();
    let service_operation = record.intents()[0].operation;
    let finish_operation = Uuid::new_v4();
    record.observed(epoch, custody, 2000).unwrap();
    assert!(record.finishing(epoch, custody, service_operation).is_err());
    record.finishing(epoch, custody, finish_operation).unwrap();
    for state in ["finishing", "finish_unknown"] {
        let mut value = serde_json::to_value(&record).unwrap();
        value["startup"]["state"] = json!(state);
        let mut decoded: Record = serde_json::from_value(value.clone()).unwrap();
        assert!(decoded.finished(epoch, custody, service_operation).is_err());
        decoded.finished(epoch, custody, finish_operation).unwrap();
        value["startup"]["operation"] = json!(service_operation);
        assert!(serde_json::from_value::<Record>(value).is_err());
    }
}
