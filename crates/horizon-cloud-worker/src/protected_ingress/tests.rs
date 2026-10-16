use super::{
    decode,
    files::{Stamp, readonly},
    kernel::{identity_map, root_status, start},
    match_allocation,
};
use horizon_cloud_protocol::exposure::ingress::MAX_RECEIPT_BYTES;
use rustix::fs::StatVfsMountFlags;
use serde_json::json;

fn encoded() -> Vec<u8> {
    let mut published = vec![json!({"protocol":"tcp","host_ip":"0.0.0.0","host_port":22,"container_port":22})];
    for (host, container) in (20000..=20031).zip(600..=631) {
        published.push(json!({"protocol":"tcp","host_ip":"0.0.0.0","host_port":host,"container_port":container}));
    }
    serde_json::to_vec(&json!({
        "version":1,"allocation":"original-allocation","generation":"00000000-0000-0000-0000-000000000001",
        "container":{"id":"a".repeat(64),"requested_image":format!("registry.example/worker@sha256:{}","b".repeat(64)),
            "image_id":format!("sha256:{}","c".repeat(64)),"init_start_ticks":123,"netns":{"device":4,"inode":456}},
        "unprivileged_port_start":1024,"published":published
    }))
    .unwrap()
}

#[test]
fn canonical_receipt_decoding_never_substitutes_a_requested_allocation() {
    let bytes = encoded();
    let receipt = decode(&bytes).unwrap();
    assert_eq!(receipt.facts().container.init_start_ticks, 123);
    assert!(match_allocation(receipt.facts(), "original-allocation").is_ok());
    assert!(match_allocation(receipt.facts(), "another-allocation").is_err());
    assert!(decode(&vec![b' '; MAX_RECEIPT_BYTES + 1]).is_err());
    let text = String::from_utf8(bytes).unwrap();
    let duplicate = text.replace("\"version\":1", "\"version\":1,\"version\":1");
    assert_ne!(duplicate, text);
    assert!(decode(duplicate.as_bytes()).is_err());
    let mut altered: serde_json::Value = serde_json::from_str(&text).unwrap();
    altered["extra_authority"] = true.into();
    assert!(decode(&serde_json::to_vec(&altered).unwrap()).is_err());
}

fn stamp(directory: bool) -> Stamp {
    Stamp {
        device: 1,
        inode: 2,
        mode: if directory { 0o040_700 } else { 0o100_400 },
        uid: 0,
        gid: 0,
        links: if directory { 2 } else { 1 },
        size: if directory { 0 } else { 100 },
        modified: (1, 2),
        changed: (3, 4),
    }
}

#[test]
fn writable_shared_linked_special_empty_and_oversized_receipt_metadata_is_refused() {
    stamp(false).validate(false).unwrap();
    stamp(true).validate(true).unwrap();
    let mutations: [fn(&mut Stamp); 10] = [
        |s: &mut Stamp| s.uid = 10001,
        |s: &mut Stamp| s.links = 0,
        |s: &mut Stamp| s.links = 2,
        |s: &mut Stamp| s.mode = 0o100_600,
        |s: &mut Stamp| s.mode = 0o100_440,
        |s: &mut Stamp| s.mode = 0o104_400,
        |s: &mut Stamp| s.mode = 0o010_400,
        |s: &mut Stamp| s.mode = 0o120_400,
        |s: &mut Stamp| s.size = 0,
        |s: &mut Stamp| s.size = MAX_RECEIPT_BYTES as u64 + 1,
    ];
    for mutate in mutations {
        let mut candidate = stamp(false);
        mutate(&mut candidate);
        assert!(candidate.validate(false).is_err());
    }
    for mode in [0o040_755, 0o040_770, 0o041_700, 0o100_700] {
        let mut candidate = stamp(true);
        candidate.mode = mode;
        assert!(candidate.validate(true).is_err());
    }
    assert!(readonly(StatVfsMountFlags::empty()).is_err());
    readonly(StatVfsMountFlags::RDONLY | StatVfsMountFlags::NOSUID).unwrap();
}

#[test]
fn actual_thread_status_requires_root_ids_matching_process_and_effective_bind_capability() {
    let original = "Tgid:\t7\nUid:\t0\t0\t0\t0\nCapEff:\t0000000000000400\n";
    root_status(original, 7).unwrap();
    for modified in [
        original.replace("0\t0\t0\t0", "10001\t0\t0\t0"),
        original.replace("0\t0\t0\t0", "0\t10001\t0\t0"),
        original.replace("0\t0\t0\t0", "0\t0\t10001\t0"),
        original.replace("0\t0\t0\t0", "0\t0\t0\t10001"),
        original.replace("0000000000000400", "0000000000000000"),
        original.replace("0000000000000400", "invalid"),
        original.replace("Tgid:\t7", "Tgid:\t8"),
        original.to_owned() + "CapEff:\t400\n",
    ] {
        assert!(root_status(&modified, 7).is_err());
    }
}

#[test]
fn rootless_and_remapped_identity_maps_cannot_mint_original_host_root_custody() {
    identity_map("         0          0 4294967295\n").unwrap();
    for text in [
        "",
        "0 0 1",
        "0 10001 1",
        "0 0 65536",
        "0 0 4294967295\n0 0 1",
        "+0 0 4294967295",
    ] {
        assert!(identity_map(text).is_err());
    }
}

#[test]
fn init_start_parser_handles_command_parentheses_and_refuses_dead_or_wrong_processes() {
    let original = format!("1 (init ) command) S {}123 0\n", "0 ".repeat(18));
    assert_eq!(start(&original, 1).unwrap(), 123);
    assert!(start(&original, 2).is_err());
    for state in ["Z", "X", "x", "?"] {
        assert!(start(&original.replace(") S ", &format!(") {state} ")), 1).is_err());
    }
    assert!(start(&original.replace("123", "0"), 1).is_err());
    assert!(start(&original.replace("123", "not-a-number"), 1).is_err());
    assert!(start("1 (init) S 0", 1).is_err());
}

#[test]
#[ignore = "H03: only the reviewed original task-owned VM/container with its genuine readonly host mount; never desktop"]
fn actual_original_root_receipt_readback() {
    let mut live = super::LiveReceipt::capture().unwrap();
    let original = live.facts().unwrap().clone();
    live.revalidate().unwrap();
    assert_eq!(live.facts().unwrap(), &original);
    println!(
        "H03_PASSIVE_RECEIPT {}",
        serde_json::to_string(live.facts().unwrap()).unwrap()
    );
}
