use super::*;
use crate::exposure::{ConnectionLimits, RESERVED_INGRESS_START, SourceRule, ingress};
use serde_json::{Value, json};

fn policy() -> ExposureRequest {
    ExposureRequest {
        mode: ExposureMode::Restricted,
        target_port: 18080,
        sources: vec![SourceRule::Cidr("192.0.2.0/24".into())],
        limits: ConnectionLimits::default(),
    }
}

fn open() -> Request {
    Request {
        session: Session {
            version: VERSION,
            allocation: "original-allocation".into(),
            epoch: OperationId::generate(),
            challenge: OperationId::generate(),
        },
        custody: OperationId::generate(),
        id: OperationId::generate(),
        command: Command::Open {
            service: OperationId::generate(),
            policy: policy(),
            route: crate::exposure::Route::Gate {},
            members: None,
            lifetime: Lifetime {
                anchor_millis: 123,
                remaining_millis: MAX_LIFETIME_MILLIS,
            },
        },
    }
}

fn reply() -> Reply {
    let request = open();
    let Command::Open {
        service,
        policy,
        lifetime,
        ..
    } = request.command
    else {
        unreachable!()
    };
    Reply {
        session: request.session,
        custody: request.custody,
        worker: WorkerObservation::Unavailable {},
        id: Some(request.id),
        outcome: Outcome::Applied,
        elapsed_millis: 124,
        entries: vec![Entry {
            service,
            operation: request.id,
            policy,
            route: crate::exposure::Route::Gate {},
            url: None,
            tailnet_ingress: None,
            ingress: RESERVED_INGRESS_START,
            remaining_millis: lifetime.remaining_millis - 1,
            phase: Phase::Active,
        }],
    }
}

#[test]
fn independent_service_and_command_identities_survive_all_command_round_trips() {
    let request = open();
    let Command::Open { service, lifetime, .. } = request.command.clone() else {
        unreachable!()
    };
    assert_ne!(service, request.id);
    for command in [
        request.command.clone(),
        Command::Renew { service, lifetime },
        Command::Close { service },
        Command::Read,
        Command::Finish,
    ] {
        let value = Request {
            id: OperationId::generate(),
            command,
            ..request.clone()
        };
        assert_eq!(Request::from_json(&serde_json::to_vec(&value).unwrap()).unwrap(), value);
    }
    let reply = reply();
    assert_eq!(Reply::from_json(&serde_json::to_vec(&reply).unwrap()).unwrap(), reply);
}

#[test]
fn frames_refuse_unknown_duplicate_trailing_and_oversized_input() {
    let request = open();
    let value = serde_json::to_value(&request).unwrap();
    for field in ["approved", "socket", "backend", "credential", "root", "member_ids"] {
        let mut changed = value.clone();
        changed[field] = true.into();
        assert!(Request::from_json(&serde_json::to_vec(&changed).unwrap()).is_err());
    }
    for pointer in ["/session", "/command", "/command/policy", "/command/lifetime"] {
        let mut changed = value.clone();
        changed.pointer_mut(pointer).unwrap()["unknown"] = true.into();
        assert!(Request::from_json(&serde_json::to_vec(&changed).unwrap()).is_err());
    }
    let encoded = serde_json::to_string(&request).unwrap();
    for bad in [
        format!("{encoded}{{}}"),
        encoded.replacen("\"version\":3", "\"version\":3,\"version\":3", 1),
        encoded.replacen(
            "\"target_port\":18080",
            "\"target_port\":18080,\"target_port\":18080",
            1,
        ),
    ] {
        assert_ne!(bad, encoded);
        assert!(Request::from_json(bad.as_bytes()).is_err());
    }
    assert_eq!(Request::from_json(&vec![b' '; MAX_REQUEST_BYTES + 1]), Err(Error::Size));
    assert_eq!(Reply::from_json(&vec![b' '; MAX_REPLY_BYTES + 1]), Err(Error::Size));
}

#[test]
fn original_session_and_nonnil_identity_facts_are_required() {
    let value = serde_json::to_value(open()).unwrap();
    for (pointer, bad) in [
        ("/session/version", json!(1)),
        ("/session/allocation", json!("../another-worker")),
        ("/session/epoch", json!(uuid::Uuid::nil())),
        ("/session/challenge", json!(uuid::Uuid::nil())),
        ("/custody", json!(uuid::Uuid::nil())),
        ("/id", json!(uuid::Uuid::nil())),
        ("/command/service", json!(uuid::Uuid::nil())),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() = bad;
        assert!(Request::from_json(&serde_json::to_vec(&changed).unwrap()).is_err());
    }
    let mut missing = value;
    missing["command"].as_object_mut().unwrap().remove("lifetime");
    assert!(Request::from_json(&serde_json::to_vec(&missing).unwrap()).is_err());
}

#[test]
fn renewal_uses_bounded_elapsed_interval_without_desktop_clock_defaults() {
    for (anchor_millis, remaining_millis) in [(0, 0), (0, MAX_LIFETIME_MILLIS + 1), (u64::MAX, 1)] {
        assert_eq!(
            Lifetime {
                anchor_millis,
                remaining_millis
            }
            .deadline_millis(),
            Err(Error::Lifetime)
        );
    }
    assert_eq!(
        Lifetime {
            anchor_millis: 123,
            remaining_millis: MAX_LIFETIME_MILLIS
        }
        .deadline_millis(),
        Ok(123 + MAX_LIFETIME_MILLIS)
    );
    let mut request = open();
    let Command::Open { service, .. } = request.command else {
        unreachable!()
    };
    request.command = Command::Renew {
        service,
        lifetime: Lifetime {
            anchor_millis: u64::MAX,
            remaining_millis: 1,
        },
    };
    assert_eq!(request.validate(), Err(Error::Lifetime));
}

#[test]
fn open_and_observations_use_the_shared_complete_policy_validator() {
    for mode in [ExposureMode::TailnetOnly, ExposureMode::Public] {
        let mut request = open();
        let Command::Open { policy, .. } = &mut request.command else {
            unreachable!()
        };
        policy.mode = mode;
        policy.sources.clear();
        assert_eq!(request.validate().is_ok(), mode == ExposureMode::Public);
    }
    let request = serde_json::to_value(open()).unwrap();
    for port in [0, 22, 5900, 9222, 2375, 20000, 20031, 20032, 20063]
        .into_iter()
        .chain(ingress::LISTENER_START..=ingress::BACKEND_END)
    {
        let mut changed = request.clone();
        changed["command"]["policy"]["target_port"] = port.into();
        assert!(Request::from_json(&serde_json::to_vec(&changed).unwrap()).is_err());
    }
    for mode in ["off", "public", "tailnet_only"] {
        let mut changed = request.clone();
        changed["command"]["policy"]["mode"] = mode.into();
        assert!(Request::from_json(&serde_json::to_vec(&changed).unwrap()).is_err());
    }
    let mut changed = request;
    changed["command"]["policy"]["sources"] = json!([
        {"kind":"cidr","value":"192.0.2.0/24"},
        {"kind":"dns_name","value":"*.example.ts.net"}
    ]);
    assert!(Request::from_json(&serde_json::to_vec(&changed).unwrap()).is_err());
    let mut observed = reply();
    observed.entries[0].policy.mode = ExposureMode::Off;
    observed.entries[0].policy.sources.clear();
    assert_eq!(observed.validate(), Err(Error::PolicyOff));
}

#[test]
fn ready_and_closed_have_distinct_strict_shapes() {
    let mut value = reply();
    value.outcome = Outcome::Ready;
    assert_eq!(value.validate(), Err(Error::Entries));
    value.id = None;
    assert_eq!(value.validate(), Err(Error::Entries));
    value.entries.clear();
    assert!(value.validate().is_ok());
    value.outcome = Outcome::Applied;
    assert_eq!(value.validate(), Err(Error::Entries));
    let mut value = reply();
    value.outcome = Outcome::Closed;
    assert_eq!(value.validate(), Err(Error::Entries));
    value.entries[0].phase = Phase::Closed;
    assert_eq!(value.validate(), Err(Error::Entries));
    value.entries[0].remaining_millis = 0;
    assert!(value.validate().is_ok());
    value.id = None;
    assert_eq!(value.validate(), Err(Error::Entries));
}

#[test]
fn observed_services_refuse_duplicates_excess_count_and_invalid_deadlines() {
    let original = reply();
    for field in ["service", "operation", "ingress"] {
        let mut value = original.clone();
        let mut entry = value.entries[0].clone();
        entry.service = OperationId::generate();
        entry.operation = OperationId::generate();
        entry.ingress += 1;
        let mut encoded = serde_json::to_value(entry).unwrap();
        encoded[field] = serde_json::to_value(&value.entries[0]).unwrap()[field].clone();
        value.entries.push(serde_json::from_value(encoded).unwrap());
        assert_eq!(value.validate(), Err(Error::Entries));
    }
    let mut value = original.clone();
    for index in 1..=MAX_SERVICES {
        value.entries.push(Entry {
            service: OperationId::generate(),
            operation: OperationId::generate(),
            ingress: RESERVED_INGRESS_START + u16::try_from(index).unwrap(),
            ..original.entries[0].clone()
        });
    }
    assert_eq!(value.validate(), Err(Error::Entries));
    for (phase, remaining) in [
        (Phase::Active, 0),
        (Phase::Active, MAX_LIFETIME_MILLIS + 1),
        (Phase::Expired, 1),
        (Phase::Closed, 1),
    ] {
        let mut value = original.clone();
        value.entries[0].phase = phase;
        value.entries[0].remaining_millis = remaining;
        assert_eq!(value.validate(), Err(Error::Entries));
    }
    for port in [0, 22, 20032, u16::MAX] {
        let mut value = original.clone();
        value.entries[0].ingress = port;
        assert_eq!(value.validate(), Err(Error::Entries));
    }
    let mut value = original;
    value.elapsed_millis = u64::MAX;
    assert_eq!(value.validate(), Err(Error::Entries));
}

#[test]
fn reply_rejects_unexpected_fields_and_repeated_entry_keys() {
    let original = reply();
    let mut value = serde_json::to_value(&original).unwrap();
    value["entries"][0]["provider_release_receipt"] = Value::Bool(true);
    assert!(Reply::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    let encoded = serde_json::to_string(&original).unwrap();
    let duplicate = encoded.replacen("\"ingress\":20000", "\"ingress\":20000,\"ingress\":20000", 1);
    assert_ne!(encoded, duplicate);
    assert!(Reply::from_json(duplicate.as_bytes()).is_err());
}

#[test]
fn worker_observations_preserve_no_tailnet_and_never_turn_unverified_identity_into_running() {
    let identity = WorkerIdentity {
        self_id: "nOriginalWorker".into(),
        full_dns_name: "worker.example.ts.net".into(),
        magic_dns_suffix: "example.ts.net".into(),
        addresses: vec!["100.64.1.2".parse().unwrap(), "fd7a:115c:a1e0::2".parse().unwrap()],
    };
    let mut value = reply();
    assert!(value.validate().is_ok()); // Direct ingress can use a cloud with Tailnet None.
    value.worker = WorkerObservation::Running {
        identity: identity.clone(),
    };
    assert!(value.validate().is_ok());
    value.worker = WorkerObservation::Unverified {
        identity: identity.clone(),
    };
    assert_eq!(value.validate(), Err(Error::Worker));
    value.outcome = Outcome::Uncertain;
    assert!(value.validate().is_ok());
    for (field, bad) in [
        ("self_id", json!("")),
        ("self_id", json!("agent supplied / ID")),
        ("full_dns_name", json!("Worker.example.ts.net")),
        ("full_dns_name", json!("worker.example.ts.net.")),
        ("full_dns_name", json!("worker.otherexample.ts.net")),
        ("magic_dns_suffix", json!("other.ts.net")),
        ("addresses", json!([])),
        ("addresses", json!(["127.0.0.1"])),
        ("addresses", json!(["0.0.0.0"])),
        ("addresses", json!(["ff02::1"])),
        ("addresses", json!(["100.64.1.2", "100.64.1.2"])),
        ("addresses", json!(["100.64.1.3", "100.64.1.2"])),
        (
            "addresses",
            json!((1..=17).map(|n| format!("100.64.1.{n}")).collect::<Vec<_>>()),
        ),
    ] {
        let mut invalid = serde_json::to_value(&value).unwrap();
        invalid["worker"]["identity"][field] = bad;
        assert!(Reply::from_json(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let mut invalid = serde_json::to_value(&value).unwrap();
    invalid["worker"]["identity"]["pid"] = json!(123);
    assert!(Reply::from_json(&serde_json::to_vec(&invalid).unwrap()).is_err());
    let mut missing = serde_json::to_value(reply()).unwrap();
    missing.as_object_mut().unwrap().remove("worker");
    assert!(Reply::from_json(&serde_json::to_vec(&missing).unwrap()).is_err());
}

#[test]
fn tailnet_members_are_comparison_data_bound_to_exact_recipient_not_public_policy() {
    let mut request = open();
    let identity = WorkerIdentity {
        self_id: "nOriginal".into(),
        full_dns_name: "root.example.ts.net".into(),
        magic_dns_suffix: "example.ts.net".into(),
        addresses: vec!["100.64.0.1".parse().unwrap()],
    };
    let Command::Open { policy, members, .. } = &mut request.command else {
        unreachable!()
    };
    policy.mode = ExposureMode::TailnetOnly;
    policy.sources.clear();
    assert!(members.is_none());
    assert!(request.validate().is_err());
    let Command::Open { members, .. } = &mut request.command else {
        unreachable!()
    };
    *members = Some(MemberFacts {
        recipient: identity,
        stable_ids: vec!["nOriginal".into(), "nPeer".into()],
    });
    request.validate().unwrap();
    let Command::Open { policy, .. } = &mut request.command else {
        unreachable!()
    };
    policy.mode = ExposureMode::Public;
    assert!(request.validate().is_err());
}

#[test]
fn actual_virtual_tcp_endpoint_is_distinct_from_public_and_https_routes() {
    let mut reply = reply();
    reply.worker = WorkerObservation::Running {
        identity: WorkerIdentity {
            self_id: "nOriginal".into(),
            full_dns_name: "root.example.ts.net".into(),
            magic_dns_suffix: "example.ts.net".into(),
            addresses: vec!["100.64.0.1".parse().unwrap()],
        },
    };
    let entry = &mut reply.entries[0];
    entry.tailnet_ingress = Some(20001);
    entry.url = Some("tcp://root.example.ts.net:20001".into());
    reply.validate().unwrap();
    reply.entries[0].tailnet_ingress = Some(reply.entries[0].ingress);
    assert!(reply.validate().is_err());
    reply.entries[0].policy.mode = ExposureMode::TailnetOnly;
    reply.entries[0].policy.sources.clear();
    reply.entries[0].url = Some(format!("tcp://root.example.ts.net:{}", reply.entries[0].ingress));
    reply.validate().unwrap();
    reply.entries[0].tailnet_ingress = None;
    assert!(reply.validate().is_err());
    let entry = &mut reply.entries[0];
    entry.policy.mode = ExposureMode::Public;
    entry.route = Route::FunnelHttps { https: 8443 };
    entry.ingress = 8443;
    entry.url = Some("https://root.example.ts.net:8443/".into());
    reply.validate().unwrap();
    reply.entries[0].ingress = 20000;
    assert!(reply.validate().is_err());
}

#[test]
fn lost_daemon_retains_original_route_facts_without_positive_readiness() {
    let mut reply = reply();
    let identity = WorkerIdentity {
        self_id: "nOriginal".into(),
        full_dns_name: "root.example.ts.net".into(),
        magic_dns_suffix: "example.ts.net".into(),
        addresses: vec!["100.64.0.1".parse().unwrap()],
    };
    reply.worker = WorkerObservation::Unverified { identity };
    reply.outcome = Outcome::Uncertain;
    reply.entries[0].tailnet_ingress = Some(20001);
    reply.entries[0].url = Some("tcp://root.example.ts.net:20001".into());
    reply.validate().unwrap();
    reply.outcome = Outcome::Applied;
    assert!(reply.validate().is_err());
    reply.outcome = Outcome::Closed;
    let entry = &mut reply.entries[0];
    entry.phase = Phase::Closed;
    entry.remaining_millis = 0;
    entry.policy.mode = ExposureMode::Public;
    entry.policy.sources.clear();
    entry.route = Route::FunnelHttps { https: 443 };
    entry.ingress = 443;
    entry.tailnet_ingress = None;
    entry.url = Some("https://root.example.ts.net/".into());
    reply.validate().unwrap();
    reply.worker = WorkerObservation::Unavailable {};
    assert!(reply.validate().is_err());
}
