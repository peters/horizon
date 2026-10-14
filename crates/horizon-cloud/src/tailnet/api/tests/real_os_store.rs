//! Synthetic real Linux Secret Service lane; only the isolated fixture may run it.
use crate::tailnet::{ApiCredential, ApiCredentialKind, ApiCredentialStatus, KEYCHAIN_SERVICE, Store};
use keyring_core::CredentialStore;
use std::{path::Path, sync::Arc};
use zeroize::Zeroizing;

const AUTH: &str = "tskey-auth-synthetic12345678901234567890";

struct Networks {
    store: Store,
    a: String,
    b: String,
}

fn create_networks(root: &Path) -> Result<Networks, String> {
    if root != Path::new("/fixture/state/horizon/cloud") {
        return Err("fixture root refused".into());
    }
    let store = Store::new(root.to_path_buf());
    let first = store.save(None, "API test A", AUTH).map_err(|e| e.to_string())?;
    let a = first.tailnets.first().ok_or("fixture tailnet missing")?.id.clone();
    let next = store.save(None, "API test B", AUTH).map_err(|e| e.to_string())?;
    let b = next
        .tailnets
        .iter()
        .find(|network| network.id != a)
        .ok_or("second fixture missing")?
        .id
        .clone();
    Ok(Networks { store, a, b })
}

fn exercise(root: &Path) -> Result<(), String> {
    let Networks { store, a, b } = create_networks(root)?;
    let keys: Arc<CredentialStore> =
        zbus_secret_service_keyring_store::Store::new().map_err(|_| "fixture keyring unavailable")?;
    let auth_slot = store.credential_slot(&a).map_err(|e| e.to_string())?;
    let before = Zeroizing::new(
        keys.build(KEYCHAIN_SERVICE, &auth_slot, None)
            .map_err(|_| "fixture entry unavailable")?
            .get_secret()
            .map_err(|_| "fixture auth unreadable")?,
    );
    if &**before != AUTH.as_bytes() {
        return Err("synthetic auth mismatch".into());
    }
    if store.api_credential_status(&a).map_err(|e| e.to_string())? != ApiCredentialStatus::Missing {
        return Err("initial status mismatch".into());
    }
    let personal = ApiCredential::personal_token("tskey-api-synthetic-only-a").map_err(|e| e.to_string())?;
    store.save_api_credential(&a, &personal).map_err(|e| e.to_string())?;
    if store.api_credential_status(&a).map_err(|e| e.to_string())?
        != ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)
    {
        return Err("personal status mismatch".into());
    }
    let oauth = ApiCredential::oauth("synthetic-client-id", "synthetic-client-secret").map_err(|e| e.to_string())?;
    store.save_api_credential(&a, &oauth).map_err(|e| e.to_string())?;
    store.save_api_credential(&b, &personal).map_err(|e| e.to_string())?;
    if store.api_credential_status(&a).map_err(|e| e.to_string())?
        != ApiCredentialStatus::Stored(ApiCredentialKind::OAuth)
    {
        return Err("oauth status mismatch".into());
    }
    store.remove_api_credential(&a).map_err(|e| e.to_string())?;
    if store.api_credential_status(&a).map_err(|e| e.to_string())? != ApiCredentialStatus::Missing {
        return Err("removed status mismatch".into());
    }
    if store.api_credential_status(&b).map_err(|e| e.to_string())?
        != ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)
    {
        return Err("other status changed".into());
    }
    let after = Zeroizing::new(
        keys.build(KEYCHAIN_SERVICE, &auth_slot, None)
            .map_err(|_| "fixture entry unavailable")?
            .get_secret()
            .map_err(|_| "fixture auth unreadable")?,
    );
    if *before != *after || store.credential_slot(&a).map_err(|e| e.to_string())? != auth_slot {
        return Err("auth binding changed".into());
    }
    for file in ["tailnets.json", "tailnet-api-bindings.json", "tailnet-api.pending.json"] {
        if let Ok(bytes) = std::fs::read(root.join(file)) {
            for value in [
                AUTH.as_bytes(),
                b"tskey-api-synthetic-only-a",
                b"synthetic-client-secret",
            ] {
                if bytes.windows(value.len()).any(|slice| slice == value) {
                    return Err("secret in metadata".into());
                }
            }
        }
    }
    println!(
        "\n{}",
        serde_json::json!({"state":"passed","real_os_store":true,"checks":["missing","personal_token_save","oauth_replace","independent_second_tailnet","api_remove","auth_preserved","nonsecret_metadata","bindings_retained_for_restart_probe"]})
    );
    Ok(())
}
fn probe(root: &Path, unavailable: bool, cleanup: bool) -> Result<(), String> {
    if root != Path::new("/fixture/state/horizon/cloud") {
        return Err("fixture root refused".into());
    }
    let store = Store::new(root.to_path_buf());
    let catalog = store.load().map_err(|e| e.to_string())?;
    if catalog.tailnets.len() != 2
        || catalog
            .tailnets
            .iter()
            .any(|n| !matches!(n.name.as_str(), "API test A" | "API test B"))
    {
        return Err("fixture ownership mismatch".into());
    }
    for network in &catalog.tailnets {
        let status = store.api_credential_status(&network.id);
        if unavailable && network.name == "API test B" {
            if !matches!(status, Err(crate::tailnet::Error::Keychain)) {
                return Err("unavailable was not a keychain error".into());
            }
        } else {
            let expected = if network.name == "API test A" {
                ApiCredentialStatus::Missing
            } else {
                ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)
            };
            if status.map_err(|e| e.to_string())? != expected {
                return Err("persisted kind mismatch".into());
            }
        }
    }
    if cleanup {
        for network in &catalog.tailnets {
            store.delete(&network.id).map_err(|e| e.to_string())?;
        }
        if !store.load().map_err(|e| e.to_string())?.tailnets.is_empty() {
            return Err("fixture cleanup incomplete".into());
        }
    }
    println!(
        "\n{}",
        serde_json::json!({"state":"passed","operation":if unavailable {"unavailable"} else if cleanup {"cleanup"} else {"restart_probe"},"nonsecret_kind_only":true})
    );
    Ok(())
}
#[test]
#[ignore = "requires isolated synthetic Secret Service; see F05 in the credential-store procedure"]
fn qualified_synthetic_store() {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    assert!(
        std::env::var("DBUS_SESSION_BUS_ADDRESS").as_deref() == Ok("unix:path=/fixture/state/runtime/bus"),
        "private fixture bus required"
    );
    assert!(
        std::env::var("DBUS_SYSTEM_BUS_ADDRESS").as_deref() == Ok("unix:path=/fixture/state/runtime/no-system-bus"),
        "private fixture system-bus refusal required"
    );
    assert!(!std::path::Path::new("/run/dbus/system_bus_socket").exists());
    assert!(
        std::fs::symlink_metadata("/fixture/state/runtime/bus")
            .unwrap()
            .file_type()
            .is_socket()
    );
    for directory in [
        "/fixture/state",
        "/fixture/state/runtime",
        "/fixture/state/horizon/cloud",
    ] {
        let metadata = std::fs::symlink_metadata(directory).unwrap();
        assert!(metadata.is_dir());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    }
    let root = Path::new("/fixture/state/horizon/cloud");
    let result = match std::env::var("HORIZON_TAILNET_FIXTURE_PHASE").as_deref() {
        Ok("exercise") => exercise(root),
        Ok("probe") => probe(root, false, false),
        Ok("unavailable") => probe(root, true, false),
        Ok("cleanup") => probe(root, false, true),
        _ => Err("fixture phase refused".into()),
    };
    assert!(result.is_ok(), "{}", result.err().unwrap_or_default());
}
