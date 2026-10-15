//! Opt-in component checks in an owned disposable Linux guest, never a host preflight.
use super::super::super::scope;
use super::super::tailnet::{Client, Identity};
use super::*;
use std::{
    fs::File,
    io::{Read, Write},
    net::{Ipv4Addr, Shutdown},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

const INPUT: &str = "/run/horizon-m1-engine-qualification/input.json";
const PORT: u16 = 18080;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    disposable_guest: bool,
    daemon_pid: u32,
    network_namespace_inode: u64,
    self_id: String,
    peer_id: String,
    peer_name: String,
    peer_ipv4: Ipv4Addr,
    wrong_route_ipv4: Option<Ipv4Addr>,
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Case {
    PeerRelayAndSelfRefusal,
    WrongRouteRefusal,
}

#[derive(serde::Serialize)]
struct Receipt {
    case: Case,
    passed: bool,
    production_preflight: bool,
    real_gateway_relay: bool,
    owner_or_worker_qualification: bool,
}

fn fixture() -> Fixture {
    let path = Path::new(INPUT);
    for part in path.ancestors() {
        let metadata = std::fs::symlink_metadata(part).expect("owned guest fixture path required");
        assert!(
            !metadata.file_type().is_symlink() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
            "guest fixture components must be root-owned and not untrusted-writable"
        );
        assert!(
            part == path || metadata.is_dir(),
            "guest fixture parent must be a directory"
        );
    }
    let mut file = File::open(path).expect("explicit root-created guest fixture required");
    let metadata = file.metadata().expect("guest fixture metadata");
    assert!(
        metadata.is_file()
            && metadata.uid() == 0
            && metadata.permissions().mode() & 0o777 == 0o600
            && metadata.nlink() == 1
            && metadata.len() <= 4096,
        "guest fixture must be a private root-owned bounded regular file"
    );
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(4097)
        .read_to_end(&mut bytes)
        .expect("guest fixture read");
    assert!(bytes.len() <= 4096, "guest fixture exceeded limit");
    let fixture: Fixture = serde_json::from_slice(&bytes).expect("typed nonsecret guest fixture");
    assert!(
        fixture.disposable_guest && fixture.daemon_pid > 1,
        "explicit disposable guest required"
    );
    assert!(
        fixture.self_id != fixture.peer_id,
        "two separately enrolled peers required"
    );
    assert!(
        fixture.peer_name.contains('.')
            && fixture.peer_name == fixture.peer_name.trim_end_matches('.').to_ascii_lowercase(),
        "full peer MagicDNS name required"
    );
    let current = std::fs::metadata("/proc/self/ns/net").expect("current guest network namespace");
    let daemon = std::fs::metadata(format!("/proc/{}/ns/net", fixture.daemon_pid))
        .expect("declared owned daemon network namespace");
    assert!(
        current.ino() == fixture.network_namespace_inode && daemon.ino() == current.ino(),
        "test and declared owned daemon must share the recorded network namespace"
    );
    let status =
        std::fs::read_to_string(format!("/proc/{}/status", fixture.daemon_pid)).expect("owned daemon UID metadata");
    assert!(
        status
            .lines()
            .find_map(|line| line.strip_prefix("Uid:"))
            .is_some_and(|uids| uids.split_whitespace().collect::<Vec<_>>() == ["0", "0", "0", "0"]),
        "declared owned daemon must run as root"
    );
    fixture
}

fn desktop(fixture: &Fixture, client: &Client) -> Identity {
    let desktop = client
        .desktop()
        .expect("production Running kernel Self read must succeed");
    assert!(
        desktop.id == fixture.self_id,
        "actual Self must match original guest node"
    );
    assert!(desktop.id != fixture.peer_id, "target must be a distinct peer");
    desktop
}

fn bound(prepared: &Prepared, fixture: &Fixture) -> bool {
    matches!(&prepared.binding, Binding::Tailnet { desktop, peer, name, address }
        if desktop == &fixture.self_id && peer == &fixture.peer_id
            && name == &fixture.peer_name && *address == SocketAddr::from((fixture.peer_ipv4, PORT)))
}

fn emit(case: Case) {
    println!(
        "M1_REAL_KERNEL_RECEIPT {}",
        serde_json::to_string(&Receipt {
            case,
            passed: true,
            production_preflight: true,
            real_gateway_relay: matches!(case, Case::PeerRelayAndSelfRefusal),
            owner_or_worker_qualification: false,
        })
        .expect("fixed nonsecret receipt")
    );
}

#[test]
#[ignore = "requires explicit Root-owned disposable guest with two real kernel peers; never run on the host"]
fn real_kernel_peer_uses_production_endpoint_gate() {
    let fixture = fixture();
    let client = Client::existing().expect("production trusted installed CLI and fixed socket required");
    let before = desktop(&fixture, &client);
    let named = Prepared::tailnet(Endpoint::new(&fixture.peer_name, PORT).expect("full peer name"))
        .expect("production DNS/route/WhoIs/post-Self checks must succeed");
    assert!(
        bound(&named, &fixture),
        "actual named binding must match original peer identity"
    );
    let literal = Prepared::tailnet(Endpoint::new(&fixture.peer_ipv4.to_string(), PORT).expect("peer IPv4"))
        .expect("production literal peer checks must succeed");
    assert!(
        bound(&literal, &fixture),
        "actual literal binding must match original peer identity"
    );
    let gateway = Gateway::start(named).expect("production recheck and gateway start");
    let address = SocketAddr::from((fixture.peer_ipv4, PORT));
    assert!(
        gateway
            .gate
            .prepared
            .endpoint
            .matches(&Destination::Name(fixture.peer_name.clone(), PORT), address),
        "approved name must retain the exact literal destination"
    );
    assert!(
        gateway
            .gate
            .admit(&Destination::Address((fixture.peer_ipv4, PORT + 1).into()))
            .is_err(),
        "another service port must refuse"
    );
    let mut stream = super::relay::connect(&gateway, address);
    stream
        .write_all(b"M1-REAL-KERNEL-REQUEST\n")
        .expect("synthetic peer request");
    stream
        .shutdown(Shutdown::Write)
        .expect("production relay request half-close");
    let mut reply = Vec::new();
    stream
        .take(21)
        .read_to_end(&mut reply)
        .expect("bounded synthetic peer response");
    assert!(
        reply == b"M1-REAL-KERNEL-PEER\n",
        "exact peer marker after request EOF required"
    );
    gateway
        .revoke(&gateway.status().receipt)
        .expect("exact component receipt close");
    drop(gateway);
    for self_ip in &before.addresses {
        assert!(
            matches!(
                Prepared::tailnet(Endpoint::new(&self_ip.to_string(), PORT).expect("actual Self IPv4")),
                Err(Error::UnsupportedSource)
            ),
            "actual Self node must never substitute for a target peer"
        );
    }
    assert!(
        desktop(&fixture, &client) == before,
        "Self must remain unchanged after the real relay"
    );
    emit(Case::PeerRelayAndSelfRefusal);
}

#[test]
#[ignore = "requires a Root-owned guest address with an actually wrong route; never edits routes itself"]
fn real_kernel_wrong_source_route_refuses_before_peer_admission() {
    let fixture = fixture();
    let client = Client::existing().expect("production trusted installed CLI and fixed socket required");
    let before = desktop(&fixture, &client);
    let ip = fixture
        .wrong_route_ipv4
        .expect("Root must supply a real wrong-route counterexample");
    assert!(
        {
            let octets = ip.octets();
            octets[0] == 100 && (64..=127).contains(&octets[1])
        },
        "counterexample must reach the tailnet route guard"
    );
    assert!(!before.addresses.contains(&ip), "counterexample must not be Self");
    let address = SocketAddr::from((ip, PORT));
    assert!(
        !scope::source_for(address).is_some_and(|source| before.addresses.contains(&source)),
        "actual observed route must be absent or select a non-Self source"
    );
    assert!(
        matches!(
            Prepared::tailnet(Endpoint::new(&ip.to_string(), PORT).expect("tailnet route address")),
            Err(Error::UnsupportedSource)
        ),
        "production route guard must refuse this real route"
    );
    assert!(
        desktop(&fixture, &client) == before,
        "Self must remain unchanged after refusal"
    );
    emit(Case::WrongRouteRefusal);
}
