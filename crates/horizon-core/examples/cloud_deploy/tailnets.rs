//! Saved-network inventory and pre-create selection through the shared lifecycle guard.
use horizon_core::cloud_runtime::{
    Error, Result,
    tailnet::{self, ApiCredentialObservation, ApiCredentialStatus, Catalog, Tailnet},
};
use serde::Serialize;
use std::{collections::BTreeMap, path::Path};

#[derive(Serialize)]
struct Report {
    tailnets: Vec<Tailnet>,
    tailnet_api_credentials: BTreeMap<String, ApiCredentialObservation>,
}

pub(super) fn run_args(args: &[String]) -> Result<()> {
    if args.first().is_some_and(|command| command == "select-tailnet") {
        if args.len() != 4 || args[3].is_empty() {
            return Err(Error::Invalid(
                "Usage: cloud_deploy select-tailnet CATALOG_ROOT CLOUD_ID TAILNET_ID",
            ));
        }
        return tailnet::select(Path::new(&args[1]), &args[2], Some(&args[3]));
    }
    if !(2..=3).contains(&args.len()) {
        return Err(Error::Invalid("Usage: cloud_deploy tailnets STATE_ROOT [TAILNET_ID]"));
    }
    run(Path::new(&args[1]), args.get(2).map(String::as_str))
}

fn run(root: &Path, selected: Option<&str>) -> Result<()> {
    let store = tailnet::store(root);
    let catalog = store
        .load()
        .map_err(|_| Error::Invalid("Saved tailnets could not be read"))?;
    let report = describe(catalog, selected, |id| store.api_credential_status(id))?;
    let json = serde_json::to_string_pretty(&report).map_err(|_| Error::Json)?;
    println!("{json}");
    Ok(())
}

fn describe(
    catalog: Catalog,
    selected: Option<&str>,
    mut read: impl FnMut(&str) -> horizon_cloud::tailnet::Result<ApiCredentialStatus>,
) -> Result<Report> {
    if selected.is_some_and(|id| !catalog.tailnets.iter().any(|network| network.id == id)) {
        return Err(Error::Invalid("Choose an ID from the saved tailnet catalog"));
    }
    let tailnets: Vec<_> = catalog
        .tailnets
        .into_iter()
        .filter(|network| selected.is_none_or(|id| network.id == id))
        .collect();
    let tailnet_api_credentials = tailnets
        .iter()
        .map(|network| {
            let observation = match read(&network.id) {
                Ok(status) => ApiCredentialObservation::Ready(status),
                Err(_) => ApiCredentialObservation::Unavailable,
            };
            (network.id.clone(), observation)
        })
        .collect();
    Ok(Report {
        tailnets,
        tailnet_api_credentials,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::cloud_runtime::tailnet::ApiCredentialKind;

    fn catalog() -> Catalog {
        Catalog {
            tailnets: ["stored", "missing", "locked"]
                .into_iter()
                .map(|id| Tailnet {
                    id: id.into(),
                    name: format!("Synthetic {id}"),
                })
                .collect(),
        }
    }

    #[test]
    fn unavailable_store_does_not_hide_other_networks_or_mean_missing() {
        let report = describe(catalog(), None, |id| match id {
            "stored" => Ok(ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)),
            "missing" => Ok(ApiCredentialStatus::Missing),
            _ => Err(horizon_cloud::tailnet::Error::Keychain),
        })
        .unwrap();
        assert_eq!(report.tailnets.len(), 3);
        assert_eq!(
            report.tailnet_api_credentials["stored"],
            ApiCredentialObservation::Ready(ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken))
        );
        assert_eq!(
            report.tailnet_api_credentials["missing"],
            ApiCredentialObservation::Ready(ApiCredentialStatus::Missing)
        );
        assert_eq!(
            report.tailnet_api_credentials["locked"],
            ApiCredentialObservation::Unavailable
        );
    }

    #[test]
    fn unknown_id_is_refused_before_any_credential_lookup() {
        let result = describe(catalog(), Some("unknown"), |_| panic!("Unvalidated credential lookup"));
        assert!(matches!(result, Err(Error::Invalid(_))));
    }

    #[test]
    fn selected_network_reads_only_its_own_binding_and_reports_kind() {
        let mut reads = Vec::new();
        let report = describe(catalog(), Some("stored"), |id| {
            reads.push(id.to_owned());
            Ok(ApiCredentialStatus::Stored(ApiCredentialKind::OAuth))
        })
        .unwrap();
        assert_eq!(reads, ["stored"]);
        assert_eq!(report.tailnets.len(), 1);
        assert_eq!(report.tailnet_api_credentials.len(), 1);
        assert_eq!(
            report.tailnet_api_credentials["stored"],
            ApiCredentialObservation::Ready(ApiCredentialStatus::Stored(ApiCredentialKind::OAuth))
        );
    }

    fn select(root: &Path, cloud: &str, saved: &str) -> Result<()> {
        // Exercise the actual CLI route, including its pre-settings dispatch.
        crate::run_args(&[
            "select-tailnet".into(),
            root.display().to_string(),
            cloud.into(),
            saved.into(),
        ])
    }

    #[cfg(unix)]
    fn selection_catalog() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("tailnets.json"),
            serde_json::to_vec(&catalog()).unwrap(),
        )
        .unwrap();
        root
    }

    #[test]
    #[cfg(unix)]
    fn precreate_selection_uses_the_original_catalog_without_credentials() {
        let root = selection_catalog();
        let catalog_path = root.path().join("tailnets.json");
        let original = std::fs::read(&catalog_path).unwrap();
        select(root.path(), "new-cloud", "stored").unwrap();
        let cloud = root.path().join("new-cloud");
        assert_eq!(
            tailnet::Selection::load(&cloud).unwrap().tailnet.as_deref(),
            Some("stored")
        );
        assert!(!cloud.join("deployment.json").exists());
        assert_eq!(std::fs::read(catalog_path).unwrap(), original);
    }

    #[test]
    #[cfg(unix)]
    fn selection_refuses_unknown_networks_and_escaping_cloud_ids() {
        let root = selection_catalog();
        let catalog_path = root.path().join("tailnets.json");
        let original = std::fs::read(&catalog_path).unwrap();
        assert!(select(root.path(), "new-cloud", "unknown").is_err());
        assert!(!root.path().join("new-cloud/tailnet.json").exists());
        assert!(matches!(
            select(root.path(), "../escaped", "stored"),
            Err(Error::Invalid("Invalid cloud identity"))
        ));
        assert_eq!(std::fs::read(catalog_path).unwrap(), original);
    }

    #[test]
    #[cfg(unix)]
    fn selection_retains_the_original_cloud_lock() {
        let root = selection_catalog();
        select(root.path(), "new-cloud", "stored").unwrap();
        let cloud = root.path().join("new-cloud");
        let original = std::fs::read(cloud.join("tailnet.json")).unwrap();
        let held = horizon_core::cloud_runtime::state::Store::lock(&cloud).unwrap();
        assert!(matches!(select(root.path(), "new-cloud", "missing"), Err(Error::Busy)));
        assert_eq!(std::fs::read(cloud.join("tailnet.json")).unwrap(), original);
        drop(held);
        select(root.path(), "new-cloud", "missing").unwrap();
        assert_eq!(
            tailnet::Selection::load(&cloud).unwrap().tailnet.as_deref(),
            Some("missing")
        );
    }

    #[test]
    #[cfg(unix)]
    fn allocated_cloud_keeps_its_selection_and_deployment_bytes() {
        use horizon_core::cloud_runtime::state::{Deployment, Store};
        let root = selection_catalog();
        select(root.path(), "new-cloud", "stored").unwrap();
        let cloud = root.path().join("new-cloud");
        // Same valid allocation geometry as companions::lifecycle's existing fixture.
        let state: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"new-cloud","repository":"/synthetic","revision":"a".repeat(40),
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
            "stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},"spec":null,"sessions":[],
            "source_ready":true,"worker":{"id":"worker1","name":"w","imageName":"i","desiredStatus":"RUNNING",
            "publicIp":"203.0.113.7","portMappings":{"22":22022}}
        }))
        .unwrap();
        Store::lock(&cloud).unwrap().save(&state).unwrap();
        let selection = std::fs::read(cloud.join("tailnet.json")).unwrap();
        let deployment = std::fs::read(cloud.join("deployment.json")).unwrap();
        select(root.path(), "new-cloud", "stored").unwrap();
        assert!(select(root.path(), "new-cloud", "missing").is_err());
        assert_eq!(std::fs::read(cloud.join("tailnet.json")).unwrap(), selection);
        assert_eq!(std::fs::read(cloud.join("deployment.json")).unwrap(), deployment);
    }

    #[test]
    #[cfg(not(unix))]
    fn unsupported_hosts_refuse_before_creating_a_cloud() {
        let root = tempfile::tempdir().unwrap();
        let absent = root.path().join("catalog");
        assert!(select(&absent, "new-cloud", "stored").is_err());
        assert!(!absent.exists());
    }
}
