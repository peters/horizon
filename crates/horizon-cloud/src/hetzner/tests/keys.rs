use super::*;
use crate::hetzner::keys::SshKey;

const PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";

pub(super) fn key(id: u64) -> Value {
    json!({"id": id, "name": "horizon-cloud-op-1", "fingerprint": "00:11", "public_key": PUBLIC_KEY,
        "labels": {"horizon-operation": OPERATION}, "created": "2026-09-26T00:00:00Z"})
}

#[test]
fn a_key_is_registered_once_and_reused() {
    let (hetzner, requests, task) = provider(vec![
        (200, listing("ssh_keys", json!([]))),
        (201, json!({"ssh_key": key(5)})),
        (200, listing("ssh_keys", json!([key(5)]))),
    ]);
    let cancel = Cancellation::default();
    let with_comment = format!("{PUBLIC_KEY} worker@example.invalid");
    assert_eq!(hetzner.ensure_ssh_key(OPERATION, &with_comment, &cancel).unwrap().id, 5);
    assert_eq!(hetzner.ensure_ssh_key(OPERATION, PUBLIC_KEY, &cancel).unwrap().id, 5);
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /ssh_keys?label_selector=horizon-operation%3Dop-1&page=1"));
    assert_eq!(
        request_body(&requests[1]),
        json!({"name": "horizon-cloud-op-1", "public_key": with_comment, "labels": {"horizon-operation": OPERATION}})
    );
    assert_eq!(
        requests.iter().filter(|request| request.starts_with("POST ")).count(),
        1
    );
}

#[test]
fn conflicts_reconcile_or_refuse_and_other_material_is_rejected() {
    let mut other = key(5);
    other["public_key"] = json!("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB8eHRwbGhkYFxYVFBMSERAPDg0MCwoJCAcGBQQDAgEA");
    let (hetzner, _, task) = provider(vec![
        (200, listing("ssh_keys", json!([]))),
        (
            409,
            error("uniqueness_error", "SSH key with the same fingerprint already exists"),
        ),
        (200, listing("ssh_keys", json!([key(5)]))),
        (200, listing("ssh_keys", json!([]))),
        (
            409,
            error("uniqueness_error", "SSH key with the same fingerprint already exists"),
        ),
        (200, listing("ssh_keys", json!([]))),
        (200, listing("ssh_keys", json!([other]))),
    ]);
    let cancel = Cancellation::default();
    assert_eq!(hetzner.ensure_ssh_key(OPERATION, PUBLIC_KEY, &cancel).unwrap().id, 5);
    assert_eq!(
        hetzner
            .ensure_ssh_key(OPERATION, PUBLIC_KEY, &cancel)
            .unwrap_err()
            .to_string(),
        "This public key is already registered in the Hetzner project under another name"
    );
    assert!(matches!(
        hetzner.ensure_ssh_key(OPERATION, PUBLIC_KEY, &cancel),
        Err(CloudError::IdentityMismatch)
    ));
    assert!(hetzner.ensure_ssh_key(OPERATION, "ssh-rsa AAAA", &cancel).is_err());
    task.join().unwrap();
}

#[test]
fn deleting_the_key_proves_none_remains() {
    let mut foreign = key(6);
    foreign["name"] = json!("someone-else");
    let (hetzner, requests, task) = provider(vec![
        (200, listing("ssh_keys", json!([key(5)]))),
        (204, Value::Null),
        (200, listing("ssh_keys", json!([]))),
        // Ours first, then a foreign key under the same label: nothing is deleted.
        (200, listing("ssh_keys", json!([key(5), foreign]))),
    ]);
    let cancel = Cancellation::default();
    hetzner.delete_ssh_key(OPERATION, &cancel).unwrap();
    assert!(matches!(
        hetzner.delete_ssh_key(OPERATION, &cancel),
        Err(CloudError::IdentityMismatch)
    ));
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[1].starts_with("DELETE /ssh_keys/5 "));
    assert_eq!(
        requests.iter().filter(|request| request.starts_with("DELETE ")).count(),
        1
    );
    let key: SshKey = serde_json::from_value(key(5)).unwrap();
    assert!(key.verify("op-2").is_err());
}
