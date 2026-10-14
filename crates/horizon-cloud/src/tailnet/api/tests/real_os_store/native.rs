//! Native fixture rows and exact cleanup; no production API or real credential input.
use super::{AUTH, ApiCredential, ApiCredentialKind, ApiCredentialStatus, Store};
use crate::tailnet::valid_id;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, io::Read, path::Path};

const LEDGER: &str = "native-fixture-ids.json";
const MAX_LEDGER: u64 = 4096;

fn bounded_file(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "native fixture file missing")?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("native fixture file refused".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "native fixture file unreadable")?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "native fixture file unreadable")?;
    if bytes.len() as u64 > limit {
        return Err("native fixture file oversized".into());
    }
    Ok(bytes)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: u8,
    ids: Vec<String>,
}
impl Ledger {
    fn admits(&self, ids: impl Iterator<Item = String>) -> Result<(), String> {
        if self.version != 1
            || self.ids.len() > 3
            || self.ids.iter().any(|id| !valid_id(id))
            || self.ids.iter().collect::<BTreeSet<_>>().len() != self.ids.len()
            || ids.into_iter().any(|id| !self.ids.contains(&id))
        {
            return Err("native ownership ledger mismatch".into());
        }
        Ok(())
    }
    fn save(&self, root: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec(self).map_err(|_| "native ledger encoding failed")?;
        let temporary = root.join("native-fixture-ids.new");
        fs::write(&temporary, bytes).map_err(|_| "native ledger write failed")?;
        fs::rename(temporary, root.join(LEDGER)).map_err(|_| "native ledger publish failed".into())
    }
    fn read(root: &Path) -> Result<Self, String> {
        let bytes = bounded_file(&root.join(LEDGER), MAX_LEDGER)?;
        serde_json::from_slice(&bytes).map_err(|_| "native ledger invalid".into())
    }
}

pub(super) fn prepare(root: &Path) -> Result<(), String> {
    let store = Store::new(root.to_path_buf());
    if [
        LEDGER,
        "native-fixture-ids.new",
        "tailnet-api-bindings.json",
        "tailnet-api.pending.json",
        "tailnets.pending.json",
    ]
    .iter()
    .any(|file| root.join(file).exists())
        || !store.load().map_err(|e| e.to_string())?.tailnets.is_empty()
    {
        return Err("native fixture is not fresh".into());
    }
    let mut ledger = Ledger {
        version: 1,
        ids: Vec::new(),
    };
    ledger.save(root)?;
    for name in ["API test A", "API test B", "API test C"] {
        let catalog = store.save(None, name, AUTH).map_err(|e| e.to_string())?;
        let network = catalog
            .tailnets
            .iter()
            .find(|n| !ledger.ids.contains(&n.id))
            .ok_or("native row missing")?;
        ledger.ids.push(network.id.clone());
        ledger.save(root)?;
    }
    let catalog = store.load().map_err(|e| e.to_string())?;
    let mut statuses = std::collections::BTreeMap::new();
    for network in &catalog.tailnets {
        let expected = match network.name.as_str() {
            "API test A" => ApiCredentialStatus::Missing,
            "API test B" => {
                store
                    .save_api_credential(
                        &network.id,
                        &ApiCredential::personal_token("tskey-api-synthetic-only-b").map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)
            }
            "API test C" => {
                store
                    .save_api_credential(
                        &network.id,
                        &ApiCredential::oauth("synthetic-client-id-c", "synthetic-client-secret-c")
                            .map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                ApiCredentialStatus::Stored(ApiCredentialKind::OAuth)
            }
            _ => return Err("native row name mismatch".into()),
        };
        if store.api_credential_status(&network.id).map_err(|e| e.to_string())? != expected {
            return Err("native row status mismatch".into());
        }
        statuses.insert(&network.id, expected);
    }
    println!(
        "\n{}",
        serde_json::json!({"phase":"native_prepare","state":"passed","tailnets":catalog.tailnets,"direct_store_status":statuses,"mcp_claimed":false})
    );
    Ok(())
}

pub(super) fn cleanup(root: &Path) -> Result<(), String> {
    let ledger = Ledger::read(root)?;
    if ["tailnet-api.pending.json", "tailnets.pending.json"]
        .iter()
        .any(|file| root.join(file).exists())
    {
        return Err("native cleanup pending mutation unresolved".into());
    }
    let bytes = bounded_file(&root.join("tailnets.json"), 64 * 1024)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "native catalog invalid")?;
    let rows = value
        .get("tailnets")
        .and_then(serde_json::Value::as_array)
        .ok_or("native catalog invalid")?;
    let ids = rows
        .iter()
        .map(|row| {
            row.get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or("native row invalid")
        })
        .collect::<Result<Vec<_>, _>>()?;
    ledger.admits(ids.into_iter())?;
    let store = Store::new(root.to_path_buf());
    let catalog = store.load().map_err(|e| e.to_string())?;
    ledger.admits(catalog.tailnets.iter().map(|network| network.id.clone()))?;
    for network in catalog.tailnets {
        store.delete(&network.id).map_err(|e| e.to_string())?;
    }
    if !store.load().map_err(|e| e.to_string())?.tailnets.is_empty() {
        return Err("native cleanup catalog not empty".into());
    }
    let metadata_path = root.join("tailnet-api-bindings.json");
    let metadata = match fs::symlink_metadata(&metadata_path) {
        Ok(_) => Some(bounded_file(&metadata_path, 64 * 1024)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("native cleanup metadata unreadable".into()),
    };
    if let Some(bytes) = metadata {
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "native cleanup metadata invalid")?;
        if value
            .get("bindings")
            .and_then(serde_json::Value::as_object)
            .is_none_or(|records| !records.is_empty())
        {
            return Err("native cleanup retained API records".into());
        }
    }
    println!(
        "\n{}",
        serde_json::json!({"phase":"native_cleanup","state":"passed","owned_catalog_empty":true,"mcp_claimed":false})
    );
    Ok(())
}

pub(super) fn check(root: &Path) -> Result<(), String> {
    let ledger = Ledger::read(root)?;
    if ledger.ids.len() != 3 {
        return Err("native boundary check needs three rows".into());
    }
    let path = root.join("tailnets.json");
    let original = bounded_file(&path, 64 * 1024)?;
    let mut value: serde_json::Value = serde_json::from_slice(&original).map_err(|_| "native catalog invalid")?;
    value
        .get_mut("tailnets")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or("native catalog invalid")?
        .push(serde_json::json!({"id":"unowned-synthetic-id","name":"Unknown synthetic row"}));
    fs::write(
        &path,
        serde_json::to_vec(&value).map_err(|_| "native catalog encoding failed")?,
    )
    .map_err(|_| "native catalog write failed")?;
    let refused = cleanup(root).is_err();
    fs::write(&path, original).map_err(|_| "native catalog restore failed")?;
    if !refused {
        return Err("native cleanup accepted an unknown identity".into());
    }
    let store = Store::new(root.to_path_buf());
    let expected = [
        ApiCredentialStatus::Missing,
        ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken),
        ApiCredentialStatus::Stored(ApiCredentialKind::OAuth),
    ];
    for (id, expected) in ledger.ids.iter().zip(expected) {
        if store.api_credential_status(id).map_err(|e| e.to_string())? != expected {
            return Err("unknown-ID refusal changed an owned binding".into());
        }
    }
    store.delete(&ledger.ids[0]).map_err(|e| e.to_string())?;
    store
        .save(Some(&ledger.ids[1]), "Renamed fixture B", AUTH)
        .map_err(|e| e.to_string())?;
    if store.api_credential_status(&ledger.ids[1]).map_err(|e| e.to_string())?
        != ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)
    {
        return Err("native name update changed the API kind".into());
    }
    println!(
        "\n{}",
        serde_json::json!({"phase":"native_check","state":"passed","checks":["unknown_ID_refused_before_delete","known_row_removed","name_update_preserved_kind"],"mcp_claimed":false})
    );
    Ok(())
}

#[test]
fn ledger_allows_renamed_or_removed_rows_but_rejects_unknown_identity() {
    let ledger = Ledger {
        version: 1,
        ids: vec!["a".into(), "b".into(), "c".into()],
    };
    assert!(ledger.admits(["b".into()].into_iter()).is_ok());
    assert!(ledger.admits(std::iter::empty()).is_ok());
    assert!(ledger.admits(["unknown".into()].into_iter()).is_err());
    let duplicate = Ledger {
        version: 1,
        ids: vec!["a".into(), "a".into()],
    };
    assert!(duplicate.admits(std::iter::empty()).is_err());
    let oversized = Ledger {
        version: 1,
        ids: vec!["a".into(); 4],
    };
    assert!(oversized.admits(std::iter::empty()).is_err());
}
