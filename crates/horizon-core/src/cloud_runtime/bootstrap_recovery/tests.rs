use super::*;
use crate::cloud_runtime::owner::tests::{MemoryVault, create, fixture, open};
use horizon_cloud_protocol::SharingMode;
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

const HOST_KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";

fn target(owner: &Owner, root: &Path) -> Target {
    let key = root.join("ssh-key");
    assert!(
        std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .status()
            .unwrap()
            .success()
    );
    let hosts = root.join("known-hosts");
    fs::write(&hosts, format!("horizon-cloud-worker1 ssh-ed25519 {HOST_KEY}\n")).unwrap();
    fs::set_permissions(&hosts, fs::Permissions::from_mode(0o600)).unwrap();
    Target {
        startup: Startup {
            version: 1,
            controller: owner.binding().unwrap(),
            token: OperationId::generate(),
            sharing: SharingMode::TrustedShared,
            worker_operation: "original-operation".into(),
            volume_id: "volume1".into(),
            data_center_id: "region1".into(),
        },
        worker_id: "worker1".into(),
        connection: Connection {
            host: "127.0.0.1".into(),
            port: 22,
            identity: key,
            known_hosts: hosts,
            host_key_alias: "horizon-cloud-worker1".into(),
        },
    }
}

fn reply(target: &Target, request: &[u8]) -> Vec<u8> {
    let request: RecoveryRequest = serde_json::from_slice(request).unwrap();
    let signed = SignedIntent::parse(request.message.as_bytes()).unwrap();
    let intent = signed
        .verify(&target.startup.controller, request.payload.as_bytes())
        .unwrap();
    serde_json::to_vec(&RecoveryReceipt {
        version: 1,
        startup: target.startup.clone(),
        worker_id: target.worker_id.clone(),
        operation: intent.operation(),
        fingerprint: intent.fingerprint().unwrap(),
    })
    .unwrap()
}

fn saved(owner: &Owner) -> Record {
    serde_json::from_value(owner.load().unwrap()[KEY].clone()).unwrap()
}

#[test]
fn lost_reply_reopen_and_completed_retry_reuse_exact_signed_bytes() {
    let (temp, root, vault) = fixture();
    let mut owner = create(&root, &vault);
    let target = target(&owner, temp.path());
    let mut first = Vec::new();
    assert!(
        recover_with(&mut owner, &target, &mut |_, request| {
            first = request.to_vec();
            Err(Error::Invalid)
        })
        .is_err()
    );
    assert!(!saved(&owner).completed);
    assert_eq!(owner.load().unwrap()["state"], "prepared");
    drop(owner);
    let mut owner = open(&root, &vault).unwrap();
    for _ in 0..2 {
        recover_with(&mut owner, &target, &mut |_, request| {
            assert_eq!(request, first);
            Ok(reply(&target, request))
        })
        .unwrap();
        assert!(saved(&owner).completed);
    }
    assert!(recover_with(&mut owner, &target, &mut |_, _| Err(Error::Invalid)).is_err());
}

#[test]
fn uncertain_request_save_prevents_transport() {
    let (temp, root, vault) = fixture();
    let mut owner = create(&root, &vault);
    let target = target(&owner, temp.path());
    vault.fail_after(Some(0));
    assert!(recover_with(&mut owner, &target, &mut |_, _| panic!("unanchored request sent")).is_err());
    assert!(owner.load().is_err());
}

#[test]
fn uncertain_completion_never_reports_success_and_reuses_request_after_reopen() {
    for writes in [2, 3] {
        let (temp, root, vault) = fixture();
        let mut owner = create(&root, &vault);
        let target = target(&owner, temp.path());
        vault.fail_after(Some(writes));
        let mut first = Vec::new();
        assert!(
            recover_with(&mut owner, &target, &mut |_, request| {
                first = request.to_vec();
                Ok(reply(&target, request))
            })
            .is_err()
        );
        assert!(!first.is_empty());
        drop(owner);
        vault.fail_after(None);
        let mut owner = open(&root, &vault).unwrap();
        recover_with(&mut owner, &target, &mut |_, request| {
            assert_eq!(request, first);
            Ok(reply(&target, request))
        })
        .unwrap();
    }
}

#[test]
fn every_receipt_field_and_encoding_is_checked_before_completion() {
    let (temp, root, vault) = fixture();
    let mut owner = create(&root, &vault);
    let target = target(&owner, temp.path());
    let mut changed_startup = target.startup.clone();
    changed_startup.token = OperationId::generate();
    for (field, changed) in [
        ("version", json!(2)),
        ("startup", json!(changed_startup)),
        ("worker_id", json!("another-worker")),
        ("operation", json!(OperationId::generate())),
        ("fingerprint", json!(vec![1_u8; 32])),
    ] {
        assert!(
            recover_with(&mut owner, &target, &mut |_, request| {
                let mut value: serde_json::Value = serde_json::from_slice(&reply(&target, request)).unwrap();
                value[field] = changed.clone();
                Ok(serde_json::to_vec(&value).unwrap())
            })
            .is_err()
        );
        assert!(!saved(&owner).completed);
    }
    for response in [b"not JSON".to_vec(), vec![b' '; LIMIT + 1], b"\xff".to_vec()] {
        assert!(recover_with(&mut owner, &target, &mut |_, _| Ok(response.clone())).is_err());
        assert!(!saved(&owner).completed);
    }
}

#[test]
fn changed_context_pin_or_ssh_identity_is_rejected_without_transport() {
    for change in 0..4 {
        let (temp, root, vault) = fixture();
        let mut owner = create(&root, &vault);
        let mut target = target(&owner, temp.path());
        assert!(recover_with(&mut owner, &target, &mut |_, _| Err(Error::Invalid)).is_err());
        match change {
            0 => target.startup.token = OperationId::generate(),
            1 => fs::write(
                &target.connection.known_hosts,
                "horizon-cloud-worker1 ssh-ed25519 CHANGED\n",
            )
            .unwrap(),
            2 => fs::write(&target.connection.identity, "changed identity").unwrap(),
            _ => target.worker_id = "worker2".into(),
        }
        assert!(recover_with(&mut owner, &target, &mut |_, _| panic!("changed binding sent")).is_err());
        assert!(!saved(&owner).completed);
    }
}

#[test]
fn missing_unrelated_and_unsafe_pins_are_rejected_without_creating_intent() {
    for change in 0..4 {
        let (temp, root, vault) = fixture();
        let mut owner = create(&root, &vault);
        let target = target(&owner, temp.path());
        match change {
            0 => fs::remove_file(&target.connection.known_hosts).unwrap(),
            1 => fs::write(&target.connection.known_hosts, "unrelated-host ssh-ed25519 AAAA\n").unwrap(),
            2 => fs::set_permissions(&target.connection.known_hosts, fs::Permissions::from_mode(0o666)).unwrap(),
            _ => fs::set_permissions(&target.connection.identity, fs::Permissions::from_mode(0o644)).unwrap(),
        }
        assert!(recover_with(&mut owner, &target, &mut |_, _| panic!("untrusted SSH target")).is_err());
        assert!(owner.load().unwrap().get(KEY).is_none());
    }
}

#[test]
fn malformed_host_keys_never_anchor_and_corrected_pins_can_recover() {
    for invalid in [
        "ssh-ed25519 AAAAfixture".to_owned(),
        "ssh-ed25519 !!!!".to_owned(),
        format!("ssh-rsa {HOST_KEY}"),
        format!("ssh-ed25519 {}", &HOST_KEY[..HOST_KEY.len() - 4]),
    ] {
        let (temp, root, vault) = fixture();
        let mut owner = create(&root, &vault);
        let target = target(&owner, temp.path());
        fs::write(
            &target.connection.known_hosts,
            format!("horizon-cloud-worker1 {invalid}\n"),
        )
        .unwrap();
        assert!(recover_with(&mut owner, &target, &mut |_, _| panic!("malformed pin sent")).is_err());
        assert!(owner.load().unwrap().get(KEY).is_none());
        fs::write(
            &target.connection.known_hosts,
            format!("horizon-cloud-worker1 ssh-ed25519 {HOST_KEY}\n"),
        )
        .unwrap();
        recover_with(&mut owner, &target, &mut |_, request| Ok(reply(&target, request))).unwrap();
        assert!(saved(&owner).completed);
    }
}

#[test]
fn additional_host_trust_rules_never_reach_the_transport_or_journal() {
    for rule in [
        format!("* ssh-ed25519 {HOST_KEY}"),
        format!("horizon-cloud-* ssh-ed25519 {HOST_KEY}"),
        format!("horizon-cloud-worker1,other ssh-ed25519 {HOST_KEY}"),
        format!("@cert-authority * ssh-ed25519 {HOST_KEY}"),
        format!("@revoked horizon-cloud-worker1 ssh-ed25519 {HOST_KEY}"),
        format!("other ssh-ed25519 {HOST_KEY}"),
    ] {
        let (temp, root, vault) = fixture();
        let mut owner = create(&root, &vault);
        let target = target(&owner, temp.path());
        let original = fs::read(&target.connection.known_hosts).unwrap();
        fs::write(
            &target.connection.known_hosts,
            format!("horizon-cloud-worker1 ssh-ed25519 {HOST_KEY}\n{rule}\n"),
        )
        .unwrap();
        assert!(recover_with(&mut owner, &target, &mut |_, _| panic!("broader trust sent")).is_err());
        assert!(owner.load().unwrap().get(KEY).is_none());
        fs::write(&target.connection.known_hosts, original).unwrap();
        recover_with(&mut owner, &target, &mut |connection, request| {
            assert_eq!(
                fs::read_to_string(&connection.known_hosts).unwrap(),
                format!("horizon-cloud-worker1 ssh-ed25519 {HOST_KEY}\n")
            );
            Ok(reply(&target, request))
        })
        .unwrap();
    }
}

#[test]
fn every_entry_must_validate_before_a_multi_key_pin_can_be_anchored() {
    for malformed_first in [false, true] {
        let (temp, root, vault) = fixture();
        let mut owner = create(&root, &vault);
        let target = target(&owner, temp.path());
        let second = fs::read_to_string(target.connection.identity.with_extension("pub")).unwrap();
        let valid = format!("horizon-cloud-worker1 ssh-ed25519 {HOST_KEY}\nhorizon-cloud-worker1 {second}");
        let invalid = "horizon-cloud-worker1 ssh-ed25519 !!!!\n";
        let mixed = if malformed_first {
            format!("{invalid}{valid}")
        } else {
            format!("{valid}{invalid}")
        };
        fs::write(&target.connection.known_hosts, mixed).unwrap();
        assert!(recover_with(&mut owner, &target, &mut |_, _| panic!("partially validated pin sent")).is_err());
        assert!(owner.load().unwrap().get(KEY).is_none());
        fs::write(&target.connection.known_hosts, valid).unwrap();
        recover_with(&mut owner, &target, &mut |connection, request| {
            assert_eq!(fs::read_to_string(&connection.known_hosts).unwrap().lines().count(), 2);
            Ok(reply(&target, request))
        })
        .unwrap();
    }
}

#[test]
fn invalid_private_keys_never_anchor_and_corrected_identity_can_recover() {
    let (temp, root, vault) = fixture();
    let mut owner = create(&root, &vault);
    let target = target(&owner, temp.path());
    let original = fs::read(&target.connection.identity).unwrap();
    for bytes in [
        b"invalid key".to_vec(),
        original[..original.len() / 2].to_vec(),
        format!("ssh-ed25519 {HOST_KEY}\n").into_bytes(),
    ] {
        fs::write(&target.connection.identity, bytes).unwrap();
        assert!(recover_with(&mut owner, &target, &mut |_, _| panic!("invalid identity sent")).is_err());
        assert!(owner.load().unwrap().get(KEY).is_none());
    }
    fs::write(&target.connection.identity, &original).unwrap();
    assert!(
        std::process::Command::new("ssh-keygen")
            .args(["-q", "-p", "-P", "", "-N", "fixture-passphrase", "-f"])
            .arg(&target.connection.identity)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    assert!(recover_with(&mut owner, &target, &mut |_, _| panic!("encrypted identity sent")).is_err());
    assert!(owner.load().unwrap().get(KEY).is_none());
    fs::write(&target.connection.identity, original).unwrap();
    recover_with(&mut owner, &target, &mut |_, request| Ok(reply(&target, request))).unwrap();
    assert!(saved(&owner).completed);
}

#[test]
fn copied_or_rolled_back_owner_and_foreign_controller_never_send() {
    let (temp, root, vault) = fixture();
    let mut owner = create(&root, &vault);
    let target = target(&owner, temp.path());
    let other_root = temp.path().join("other-owner");
    let mut other = create(&other_root, &MemoryVault::default());
    assert!(recover_with(&mut other, &target, &mut |_, _| panic!("foreign controller")).is_err());
    let old = fs::read(root.join("journal.json")).unwrap();
    owner.save(json!({"state":"advanced"})).unwrap();
    fs::write(root.join("journal.json"), old).unwrap();
    assert!(recover_with(&mut owner, &target, &mut |_, _| panic!("rolled back journal")).is_err());
}

#[test]
#[ignore = "requires the isolated SSH fixture in scripts/cloud-recovery-smoke.py"]
fn native_ssh_worker_recovery() {
    let root = std::path::PathBuf::from(std::env::var_os("HORIZON_RECOVERY_FIXTURE").expect("fixture root"));
    assert!(root.is_absolute());
    let config: serde_json::Value = serde_json::from_slice(&fs::read(root.join("fixture.json")).unwrap()).unwrap();
    let vault = MemoryVault::default();
    let owner_path = root.join("owner");
    let mut owner = create(&owner_path, &vault);
    let target = Target {
        startup: Startup {
            version: 1,
            controller: owner.binding().unwrap(),
            token: OperationId::generate(),
            sharing: SharingMode::TrustedShared,
            worker_operation: "native-operation".into(),
            volume_id: "native-volume".into(),
            data_center_id: "native-region".into(),
        },
        worker_id: "worker1".into(),
        connection: Connection {
            host: "127.0.0.1".into(),
            port: config["port"].as_u64().unwrap().try_into().unwrap(),
            identity: root.join("id_ed25519"),
            known_hosts: root.join("known_hosts"),
            host_key_alias: "horizon-cloud-worker1".into(),
        },
    };
    let allocation = root.join("workspace/.horizon-allocation");
    fs::create_dir(&allocation).unwrap();
    fs::set_permissions(&allocation, fs::Permissions::from_mode(0o700)).unwrap();
    let bootstrap =
        json!({"version":1,"startup":target.startup,"worker_id":"worker1","phase":"initializing","recovery":null});
    for (name, contents) in [
        ("bootstrap.json", serde_json::to_vec(&bootstrap).unwrap()),
        ("allocation.lock", Vec::new()),
    ] {
        fs::write(allocation.join(name), contents).unwrap();
        fs::set_permissions(allocation.join(name), fs::Permissions::from_mode(0o600)).unwrap();
    }
    fs::write(
        root.join("runtime.json"),
        serde_json::to_vec(&json!({
            "HORIZON_WORKER_STARTUP":serde_json::to_string(&target.startup).unwrap(),
            "RUNPOD_POD_ID":"worker1","RUNPOD_VOLUME_ID":"native-volume",
            "RUNPOD_DC_ID":"native-region","HORIZON_CLOUD_OPERATION":"native-operation"
        }))
        .unwrap(),
    )
    .unwrap();
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| panic!("private exchange leaked an event"),
        secrets: Vec::new(),
    };
    let mut first = Vec::new();
    assert!(
        recover_with(&mut owner, &target, &mut |connection, request| {
            first = request.to_vec();
            runner.private_exchange(
                &mut connection.pinned_command("horizon-cloud-worker recover-allocation"),
                request,
                Duration::from_secs(10),
            )?;
            Err(Error::Invalid) // The worker committed, but the controller lost the reply.
        })
        .is_err()
    );
    assert!(!saved(&owner).completed);
    assert!(allocation.join("membership.json").exists());
    drop(owner);
    let mut owner = open(&owner_path, &vault).unwrap();
    let receipt = recover(&mut owner, &target, &cancel, Duration::from_secs(10)).unwrap();
    assert_eq!(saved(&owner).request.as_bytes(), first);
    assert!(saved(&owner).completed);
    assert_eq!(receipt.worker_id, "worker1");
    let manifest = fs::read(allocation.join("membership.json")).unwrap();
    recover(&mut owner, &target, &cancel, Duration::from_secs(10)).unwrap();
    assert_eq!(fs::read(allocation.join("membership.json")).unwrap(), manifest);
    fs::remove_file(allocation.join("membership.json")).unwrap();
    assert!(recover(&mut owner, &target, &cancel, Duration::from_secs(10)).is_err());
    assert!(!allocation.join("membership.json").exists());
    fs::write(
        root.join("assertions.json"),
        serde_json::to_vec(&json!({"lost_reply_recovered":true,"same_signed_request":true,"completed_retry_verified":true,"missing_manifest_fenced":true})).unwrap(),
    )
    .unwrap();
}

#[test]
fn transport_uses_private_verified_snapshots_when_original_files_are_replaced() {
    let (temp, root, vault) = fixture();
    let mut owner = create(&root, &vault);
    let target = target(&owner, temp.path());
    let key = fs::read(&target.connection.identity).unwrap();
    let pin = fs::read(&target.connection.known_hosts).unwrap();
    let mut snapshots = None;
    recover_with(&mut owner, &target, &mut |connection, request| {
        assert_ne!(connection.identity, target.connection.identity);
        assert_ne!(connection.known_hosts, target.connection.known_hosts);
        fs::write(&target.connection.identity, "replacement private key").unwrap();
        fs::write(&target.connection.known_hosts, "replacement host key").unwrap();
        assert_eq!(fs::read(&connection.identity).unwrap(), key);
        assert_eq!(fs::read(&connection.known_hosts).unwrap(), pin);
        assert_eq!(
            fs::metadata(&connection.identity).unwrap().permissions().mode() & 0o077,
            0
        );
        snapshots = Some((connection.identity.clone(), connection.known_hosts.clone()));
        Ok(reply(&target, request))
    })
    .unwrap();
    let (key_snapshot, pin_snapshot) = snapshots.unwrap();
    assert!(!key_snapshot.exists() && !pin_snapshot.exists());
    assert!(recover_with(&mut owner, &target, &mut |_, _| panic!("changed originals reused")).is_err());
}
