//! An Azure subscription this machine may create CPU workers in. The bearer token
//! and the subscription stay on this machine and never reach a worker.
use super::{Error, RegistryPull, Result, read_secret, validate_private_key_file};
use horizon_cloud::{
    Credential,
    azure::{
        catalog::{REGIONS, VM_SIZES},
        url::{valid_location, valid_subscription_id},
    },
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Azure {
    /// A private file that holds one ARM bearer token, such as the access token
    /// `az account get-access-token` prints. It is read for each deployment.
    pub token_file: PathBuf,
    /// The subscription, as a lowercase UUID.
    pub subscription_id: String,
    /// VM sizes from Horizon's catalog to try, in order, such as `Standard_D4s_v3`.
    pub vm_sizes: Vec<String>,
    /// Regions from Horizon's catalog, such as `northeurope`. A new cloud is made in
    /// the first; a cloud stays in the region it was made in.
    pub regions: Vec<String>,
    /// A read-only pull credential for a private worker image. It reaches the VM's
    /// custom data, which stays readable to the host for the VM's life, so use a
    /// short-lived token scoped to the image repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_pull: Option<RegistryPull>,
}

impl Azure {
    /// # Errors
    /// Requires an absolute token path, a subscription UUID, and sizes and regions
    /// that Horizon's catalog names.
    pub fn validate(&self) -> Result<()> {
        if !self.token_file.is_absolute() {
            return Err(Error::Invalid(
                "Credential bindings must use absolute machine-local paths",
            ));
        }
        if !valid_subscription_id(&self.subscription_id) {
            return Err(Error::Invalid("The Azure subscription must be a lowercase UUID"));
        }
        if self.vm_sizes.is_empty()
            || !self
                .vm_sizes
                .iter()
                .all(|name| VM_SIZES.iter().any(|size| size.name == name))
        {
            return Err(Error::Invalid(
                "Azure settings need VM sizes from Horizon's catalog, such as Standard_D4s_v3",
            ));
        }
        if self.regions.is_empty()
            || !self
                .regions
                .iter()
                .all(|name| valid_location(name) && REGIONS.iter().any(|region| region.name == name))
        {
            return Err(Error::Invalid(
                "Azure settings need regions from Horizon's catalog, such as northeurope",
            ));
        }
        if self.registry_pull.as_ref().is_some_and(|pull| !pull.valid()) {
            return Err(Error::Invalid(
                "The Azure registry pull needs a registry host such as example.azurecr.io, a username without ':' and an absolute password file",
            ));
        }
        Ok(())
    }

    /// # Errors
    /// Requires valid settings and a private, readable token file.
    pub fn credential(&self) -> Result<Credential> {
        self.validate()?;
        validate_private_key_file(&self.token_file)?;
        // Checked while borrowed, so no copy of the token outlives this call
        // unwiped, even when it is refused.
        let read = read_secret(&self.token_file)?;
        let token = std::str::from_utf8(&read)
            .map_err(|_| Error::Invalid("The Azure token file must hold one printable token"))?
            .trim();
        if token.is_empty() || token.bytes().any(|b| b <= 32 || b >= 127) {
            return Err(Error::Invalid("The Azure token file must hold one printable token"));
        }
        Credential::new(token.to_owned()).map_err(Error::from)
    }

    /// These settings narrowed to the one VM size and region chosen for a cloud, so
    /// provisioning tries nothing else. A placement that names no VM size from
    /// Horizon's Azure catalog is another provider's and changes nothing.
    /// # Errors
    /// Refuses a chosen size or region that these settings do not allow, before any
    /// work starts.
    pub fn narrow(&mut self, placement: &crate::cloud_panel::Placement) -> Result<()> {
        let sizes = &placement.cpu_types;
        if sizes.is_empty() || !sizes.iter().all(|name| VM_SIZES.iter().any(|size| size.name == name)) {
            return Ok(());
        }
        let regions = &placement.data_centers;
        if !sizes.iter().all(|size| self.vm_sizes.contains(size))
            || regions.is_empty()
            || !regions.iter().all(|region| self.regions.contains(region))
        {
            return Err(Error::Invalid(
                "The Azure VM size or region chosen for this cloud is not in the azure cloud settings; add it there or choose another",
            ));
        }
        self.vm_sizes.clone_from(sizes);
        self.regions.clone_from(regions);
        Ok(())
    }

    /// The pull login for the worker host, when a private image needs one.
    /// # Errors
    /// Requires a private, readable password file.
    pub fn registry_login(&self) -> Result<Option<horizon_cloud::host::RegistryLogin>> {
        self.registry_pull.as_ref().map(RegistryPull::login).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private(path: &std::path::Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    fn azure(token_file: &std::path::Path) -> Azure {
        serde_json::from_value(serde_json::json!({
            "token_file": token_file, "subscription_id": "00000000-0000-4000-8000-000000000001",
            "vm_sizes": ["Standard_D4s_v3", "Standard_D8s_v3"], "regions": ["northeurope"]
        }))
        .unwrap()
    }

    #[test]
    fn azure_settings_name_catalog_sizes_and_regions_and_a_private_token() {
        let root = tempfile::tempdir().unwrap();
        let token = root.path().join("arm-token");
        private(&token, "synthetic-arm-token\n");
        let settings = azure(&token);
        settings.validate().unwrap();
        assert!(settings.credential().is_ok());
        assert!(settings.registry_login().unwrap().is_none());
        let invalid: [fn(&mut Azure); 6] = [
            |azure| azure.token_file = "relative".into(),
            |azure| azure.subscription_id = "00000000-0000-4000-8000-00000000000A".into(),
            |azure| azure.vm_sizes = vec!["Standard_NC6".into()],
            |azure| azure.vm_sizes.clear(),
            |azure| azure.regions = vec!["mars".into()],
            |azure| {
                azure.registry_pull = Some(RegistryPull {
                    server: "Registry.example".into(),
                    username: "reader".into(),
                    password_file: "/absolute".into(),
                });
            },
        ];
        for (index, change) in invalid.iter().enumerate() {
            let mut candidate = settings.clone();
            change(&mut candidate);
            assert!(candidate.validate().is_err(), "case {index}");
            assert!(candidate.credential().is_err(), "case {index}");
        }
        private(&token, "two tokens\n");
        assert!(settings.credential().is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            private(&token, "synthetic-arm-token");
            std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(
                settings.credential().is_err(),
                "a token file others can read is refused"
            );
        }
    }

    #[test]
    fn a_chosen_size_and_region_narrow_the_settings_and_one_they_leave_out_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let settings = azure(&root.path().join("arm-token"));
        let placement = |sizes: &[&str], regions: &[&str]| crate::cloud_panel::Placement {
            cpu_types: sizes.iter().map(|&name| name.to_owned()).collect(),
            data_centers: regions.iter().map(|&name| name.to_owned()).collect(),
            ..crate::cloud_panel::Placement::default()
        };
        for other in [
            placement(&[], &[]),
            placement(&[], &["EU-RO-1"]),
            placement(&["cx33"], &["hel1"]),
        ] {
            let mut unchanged = settings.clone();
            unchanged.narrow(&other).unwrap();
            assert_eq!(unchanged, settings, "another provider's placement changes nothing");
        }
        let mut chosen = settings.clone();
        chosen
            .narrow(&placement(&["Standard_D8s_v3"], &["northeurope"]))
            .unwrap();
        assert_eq!(chosen.vm_sizes, ["Standard_D8s_v3"]);
        assert_eq!(chosen.regions, ["northeurope"]);
        for refused in [
            placement(&["Standard_D2s_v3"], &["northeurope"]),
            placement(&["Standard_D4s_v3"], &["westeurope"]),
            placement(&["Standard_D4s_v3"], &[]),
        ] {
            let mut candidate = settings.clone();
            assert!(candidate.narrow(&refused).is_err(), "{refused:?}");
            assert_eq!(candidate, settings);
        }
    }
}
