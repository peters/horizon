//! Recheck the exact visible target before existing Store removal; no secret source.
use super::super::requests::Target;
use horizon_core::cloud_runtime::tailnet;
/// No caller-supplied namespace or saved value enters the removal request. Recheck the visible
/// name and ID on the worker; Store owns the existing transaction lock and recovery.
pub(super) fn checked_store(target: &Target) -> Result<tailnet::Store, String> {
    let store = tailnet::store(&target.root);
    let catalog = store.load().map_err(|error| error.to_string())?;
    if !catalog
        .tailnets
        .iter()
        .any(|network| network.id == target.id && network.name == target.name)
    {
        return Err("The saved tailnet changed. Refresh and choose it again.".into());
    }
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_or_missing_catalog_targets_fail_before_os_store_or_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("tailnets.json");
        let bytes = br#"{"tailnets":[{"id":"work","name":"Renamed"}]}"#;
        std::fs::write(&file, bytes).unwrap();
        let target = Target {
            root: directory.path().into(),
            id: "work".into(),
            name: "Previous name".into(),
        };
        assert!(checked_store(&target).is_err());
        let missing = Target {
            id: "unknown".into(),
            name: "Renamed".into(),
            ..target
        };
        assert!(checked_store(&missing).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
        assert_eq!(
            std::fs::read_dir(directory.path()).unwrap().count(),
            1,
            "No journal, lock or binding was created"
        );
    }
}
