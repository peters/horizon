use super::*;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use uuid::Uuid;

fn facts() -> ReceiptFacts {
    let mut published = vec![PublishedPort {
        protocol: Protocol::Tcp,
        host_ip: Ipv4Addr::UNSPECIFIED.into(),
        host_port: 22,
        container_port: 22,
    }];
    for (host_port, container_port) in (20000..=20031).zip(600..=631) {
        published.push(PublishedPort {
            protocol: Protocol::Tcp,
            host_ip: Ipv4Addr::UNSPECIFIED.into(),
            host_port,
            container_port,
        });
    }
    ReceiptFacts {
        version: RECEIPT_VERSION,
        allocation: "allocation-original".into(),
        generation: Uuid::from_u128(1),
        container: Container {
            id: "a".repeat(64),
            requested_image: format!("registry.example/worker@sha256:{}", "b".repeat(64)),
            image_id: format!("sha256:{}", "c".repeat(64)),
            init_start_ticks: 123,
            netns: Namespace { device: 4, inode: 456 },
        },
        unprivileged_port_start: 1024,
        published,
    }
}

fn parse(facts: &ReceiptFacts) -> Result<Receipt, Error> {
    Receipt::from_json(&serde_json::to_vec(facts).expect("synthetic receipt"))
}

#[test]
fn geometry_is_bijective_disjoint_privileged_and_never_changes_logical_ports() {
    for (index, logical) in (20000..=20031).enumerate() {
        let offset = u16::try_from(index).expect("fixed pool");
        assert_eq!(listener_port(logical), Some(600 + offset));
        assert_eq!(backend_port(logical), Some(632 + offset));
        assert_eq!(super::super::producer_backend_port(logical), Some(20032 + offset));
    }
    for port in [0, 22, 599, 600, 631, 632, 663, 1024, 19999, 20032, u16::MAX] {
        assert_eq!(listener_port(port), None);
        assert_eq!(backend_port(port), None);
    }
    assert!(!is_physical_port(599));
    assert!(is_physical_port(600));
    assert!(is_physical_port(663));
    assert!(!is_physical_port(664));
}

#[test]
fn exact_complete_single_or_dual_family_publications_are_passive_facts() {
    let mut candidate = facts();
    let parsed = parse(&candidate).expect("complete IPv4 layout");
    assert_eq!(parsed.facts(), &candidate);
    assert_eq!(
        Receipt::from_json(&serde_json::to_vec(&parsed).expect("receipt")),
        Ok(parsed)
    );
    let v6: Vec<_> = candidate
        .published
        .iter()
        .map(|binding| PublishedPort {
            host_ip: Ipv6Addr::UNSPECIFIED.into(),
            ..*binding
        })
        .collect();
    candidate.published.extend(v6.clone());
    candidate.published.reverse();
    assert!(parse(&candidate).is_ok());
    candidate.published = v6;
    assert!(parse(&candidate).is_ok());
}

#[test]
fn missing_extra_duplicate_and_partial_address_families_are_refused() {
    let original = facts();
    let mut missing = original.clone();
    missing.published.pop();
    assert_eq!(parse(&missing), Err(Error::Publication));
    let mut extra = original.clone();
    extra.published.push(PublishedPort {
        host_port: 2375,
        container_port: 2375,
        ..extra.published[0]
    });
    assert_eq!(parse(&extra), Err(Error::Publication));
    let mut duplicate = original.clone();
    duplicate.published[32] = duplicate.published[31];
    assert_eq!(parse(&duplicate), Err(Error::Publication));
    let mut partial = original;
    partial.published[32].host_ip = Ipv6Addr::UNSPECIFIED.into();
    assert_eq!(parse(&partial), Err(Error::Publication));
}

#[test]
fn backend_management_high_listener_and_nonwildcard_publications_are_refused() {
    for (host, container) in [(20000, 632), (20000, 22), (20000, 20000), (22, 600), (5900, 600)] {
        let mut candidate = facts();
        candidate.published[1].host_port = host;
        candidate.published[1].container_port = container;
        assert_eq!(parse(&candidate), Err(Error::Publication));
    }
    for host_ip in [IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST)] {
        let mut candidate = facts();
        for binding in &mut candidate.published {
            binding.host_ip = host_ip;
        }
        assert_eq!(parse(&candidate), Err(Error::Publication));
    }
}

#[test]
fn unsafe_claimed_kernel_boundary_is_refused_without_attesting_valid_claims() {
    for boundary in [0, 600, 663, 1023] {
        let mut candidate = facts();
        candidate.unprivileged_port_start = boundary;
        assert_eq!(parse(&candidate), Err(Error::Kernel));
    }
    let mut candidate = facts();
    candidate.unprivileged_port_start = 2048;
    assert!(parse(&candidate).is_ok());
}

#[test]
fn identity_changes_are_retained_for_original_consumer_comparison() {
    let original = parse(&facts()).expect("synthetic");
    for field in 0..7 {
        let mut successor = facts();
        match field {
            0 => successor.allocation = "other-allocation".into(),
            1 => successor.generation = Uuid::from_u128(2),
            2 => successor.container.id = "d".repeat(64),
            3 => successor.container.requested_image = format!("registry.example/worker@sha256:{}", "d".repeat(64)),
            4 => successor.container.image_id = format!("sha256:{}", "d".repeat(64)),
            5 => successor.container.init_start_ticks += 1,
            _ => successor.container.netns.inode += 1,
        }
        assert_ne!(parse(&successor).expect("syntactically valid successor"), original);
    }
}

#[test]
fn malformed_identity_and_unknown_version_are_refused() {
    for field in 0..9 {
        let mut candidate = facts();
        match field {
            0 => candidate.version = 2,
            1 => candidate.allocation.clear(),
            2 => candidate.generation = Uuid::nil(),
            3 => candidate.container.id = "A".repeat(64),
            4 => candidate.container.requested_image = "registry.example/worker:latest".into(),
            5 => candidate.container.image_id = "sha256:abc".into(),
            6 => candidate.container.init_start_ticks = 0,
            7 => candidate.container.netns.inode = 0,
            _ => candidate.container.id = "a".repeat(65),
        }
        assert_eq!(parse(&candidate), Err(Error::Identity));
    }
}

#[test]
fn bounded_json_refuses_unknown_duplicate_nested_fields_and_non_tcp() {
    let text = serde_json::to_string(&facts()).expect("synthetic");
    for malformed in [
        text.replacen("\"version\":1", "\"version\":1,\"version\":1", 1),
        text.replacen("\"version\":1", "\"version\":1,\"approved\":true", 1),
        text.replacen("\"device\":4", "\"device\":4,\"approved\":true", 1),
        text.replacen("\"tcp\"", "\"udp\"", 1),
        text.replacen("\"tcp\"", "\"http\"", 1),
        format!("{text}\n{text}"),
    ] {
        assert_eq!(Receipt::from_json(malformed.as_bytes()), Err(Error::Encoding));
    }
    let mut exact = text.into_bytes();
    exact.resize(MAX_RECEIPT_BYTES, b' ');
    assert!(Receipt::from_json(&exact).is_ok());
    exact.push(b' ');
    assert_eq!(Receipt::from_json(&exact), Err(Error::Encoding));
}

#[cfg(target_os = "linux")]
#[test]
fn actual_host_encoder_and_port_plan_match_the_canonical_receipt_contract() {
    // Import definitions with __main__ disabled. No host preflight/start/create runs.
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../horizon-cloud/src/host/ingress.py"
    ));
    let program = format!(
        "namespace={{'__name__':'pure_contract_fixture'}}\nexec({},namespace)\n{}",
        serde_json::to_string(source).unwrap(),
        r"
owner=namespace['Original']('registry.example/worker@sha256:'+'a'*64,'allocation-original',2,'/synthetic-volume/workspace')
owner.cid='b'*64
ports={'22/tcp':[dict(HostIp='0.0.0.0',HostPort='22')]}
for i in range(namespace['COUNT']):
    ports[str(namespace['LISTENER_START']+i)+'/tcp']=[dict(HostIp='0.0.0.0',HostPort=str(namespace['EXTERNAL_START']+i))]
data=owner.encode_receipt(dict(image_id='sha256:'+'c'*64),(123,4,456),1024,namespace['publications'](ports))
namespace['sys'].stdout.buffer.write(data)
"
    );
    let output = std::process::Command::new("timeout")
        .args(["3s", "python3", "-I", "-c", &program])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let receipt = Receipt::from_json(&output.stdout).unwrap();
    assert_eq!(receipt.facts().allocation, "allocation-original");
    assert_eq!(receipt.facts().published.len(), 33);
    for logical in 20000..=20031 {
        let actual = receipt
            .facts()
            .published
            .iter()
            .find(|p| p.host_port == logical)
            .unwrap();
        assert_eq!(Some(actual.container_port), listener_port(logical));
        assert_ne!(Some(actual.container_port), backend_port(logical));
    }
}
