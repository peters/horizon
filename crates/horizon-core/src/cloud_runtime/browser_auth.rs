//! Repository-authorized transfer of selected remote-browser bindings over SSH.
use super::{Error, Result, command::Runner, ssh::Connection};
use crate::remote_browser_credential::{
    CredentialStores, EnvironmentCredentialStore, KeyringCredentialStore, SessionCredentialStore, resolve_authorization,
};
use horizon_browser::remote::{RemoteAdapterKind, RemoteBrowserConfig};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub local_repository: PathBuf,
    pub configuration_file: PathBuf,
    pub providers: BTreeSet<String>,
}
impl Binding {
    /// # Errors
    /// Repository content cannot choose a credential file or expand a grant.
    pub fn validate(&self) -> Result<()> {
        if !self.local_repository.is_absolute() || !self.configuration_file.is_absolute() || self.providers.is_empty() {
            return Err(Error::Invalid(
                "BrowserStack grants require absolute machine-local paths and named accounts",
            ));
        }
        Ok(())
    }
}

/// The private runtime payload is never serialized into deployment state.
#[derive(Serialize)]
struct Payload<'a> {
    version: u32,
    remote: &'a RemoteBrowserConfig,
    authorization: std::collections::BTreeMap<&'a String, &'a str>,
    quota_keys: &'a std::collections::BTreeMap<String, String>,
    local_identifier: &'a str,
    local_ports: &'a BTreeSet<u16>,
}

pub struct Prepared {
    payload: tempfile::NamedTempFile,
    targets: BTreeSet<String>,
}
impl Prepared {
    /// # Errors
    /// Missing declarations are a no-op; missing grants or credentials fail before compute allocation.
    pub fn for_repository(
        bindings: &[Binding],
        repository: &Path,
        selected: Option<&horizon_cloud::BrowserStack>,
        cloud_id: &str,
    ) -> Result<Option<Self>> {
        let Some(selected) = selected else {
            return Ok(None);
        };
        let repository = repository.canonicalize()?;
        let mut matched = None;
        for binding in bindings {
            binding.validate()?;
            if binding.local_repository.canonicalize().ok().as_ref() == Some(&repository)
                && matched.replace(binding).is_some()
            {
                return Err(Error::Invalid("Multiple BrowserStack grants match this repository"));
            }
        }
        let binding = matched.ok_or(Error::Invalid("BrowserStack is declared but credential transfer is not authorized for this repository in local cloud settings"))?;
        if !binding.providers.contains(&selected.provider) {
            return Err(Error::Invalid(
                "BrowserStack account exceeds this repository's local credential grant",
            ));
        }
        let config = crate::Config::from_yaml(&std::fs::read_to_string(&binding.configuration_file)?)
            .map_err(|_| Error::Invalid("Invalid machine-local browser configuration"))?;
        let mut remote = config.browser.remote;
        remote.targets.retain(|_, target| target.provider == selected.provider);
        remote.providers.retain(|name, _| name == &selected.provider);
        if !selected.targets.iter().all(|name| remote.targets.contains_key(name)) {
            return Err(Error::Invalid(
                "A requested BrowserStack starting target is not configured in Horizon",
            ));
        }
        if remote.providers.len() != 1
            || remote
                .providers
                .values()
                .any(|p| p.adapter != RemoteAdapterKind::Browserstack)
        {
            return Err(Error::Invalid("The declared BrowserStack account is not configured"));
        }
        remote
            .validate_definition()
            .map_err(|_| Error::Invalid("Invalid BrowserStack target configuration"))?;
        let mut environment = EnvironmentCredentialStore::new();
        let names = remote
            .providers
            .values()
            .flat_map(|p| p.credential_bindings.values())
            .filter_map(|binding| binding.environment_variable().map(str::to_owned))
            .collect();
        environment.capture_from_process(&names);
        let session = SessionCredentialStore::new();
        let keychain = remote
            .providers
            .values()
            .flat_map(|provider| provider.credential_bindings.values())
            .any(|binding| binding.store == horizon_browser::remote::CredentialStoreKind::OsKeychain)
            .then(|| KeyringCredentialStore::open().ok())
            .flatten();
        let stores = CredentialStores {
            session: &session,
            os_keychain: keychain
                .as_ref()
                .map(|s| s as &dyn crate::remote_browser_credential::RemoteCredentialStore),
            environment: Some(&environment),
        };
        Self::from_config(remote, selected, cloud_id, &stores).map(Some)
    }

    fn from_config(
        mut remote: RemoteBrowserConfig,
        selected: &horizon_cloud::BrowserStack,
        cloud_id: &str,
        stores: &CredentialStores<'_>,
    ) -> Result<Self> {
        if !horizon_cloud::valid_id(cloud_id) {
            return Err(Error::Invalid("Invalid cloud identity"));
        }
        let mut headers = std::collections::BTreeMap::new();
        let mut quotas = std::collections::BTreeMap::new();
        for (name, provider) in &remote.providers {
            let auth = resolve_authorization(provider, stores)
                .map_err(|_| Error::Invalid("BrowserStack credentials are unavailable; bind them in Horizon's OS store or named environment before deployment"))?
                .ok_or(Error::Invalid("BrowserStack requires account authentication"))?;
            headers.insert(name.clone(), zeroize::Zeroizing::new(auth.header_value().to_owned()));
            quotas.insert(name.clone(), crate::browser::remote_slots::quota_key(provider));
        }
        let identifier = format!("horizon-{cloud_id}");
        for target in remote.targets.values_mut() {
            let options = target
                .capability_extensions
                .entry("bstack:options".into())
                .or_insert_with(|| serde_json::json!({}));
            let options = options
                .as_object_mut()
                .ok_or(Error::Invalid("Invalid BrowserStack options"))?;
            options.remove("localIdentifier");
            options.insert("local".into(), (!selected.local_ports.is_empty()).into());
            if !selected.local_ports.is_empty() {
                options.insert("localIdentifier".into(), identifier.clone().into());
            }
        }
        for provider in remote.providers.values_mut() {
            provider.credential_bindings.clear();
        }
        let mut file = tempfile::NamedTempFile::new()?;
        let authorization: std::collections::BTreeMap<_, _> =
            headers.iter().map(|(key, value)| (key, value.as_str())).collect();

        serde_json::to_writer(
            &mut file,
            &Payload {
                version: 1,
                remote: &remote,
                authorization,
                quota_keys: &quotas,
                local_identifier: &identifier,
                local_ports: &selected.local_ports,
            },
        )
        .map_err(|_| Error::Json)?;
        file.flush()?;
        Ok(Self {
            payload: file,
            targets: remote.targets.keys().cloned().collect(),
        })
    }

    #[must_use]
    pub fn targets(&self) -> &BTreeSet<String> {
        &self.targets
    }

    /// # Errors
    /// Sends only a private stdin payload; both command output streams are suppressed.
    pub fn install(&self, connection: &Connection, runner: &Runner<'_>) -> Result<()> {
        runner.private_payload(
            &mut connection.command("horizon-worker-browserstack install"),
            self.payload.path(),
        )?;
        runner.run(
            "Private browser tunnel readiness",
            &mut connection.command("horizon-worker-browserstack ready"),
            std::time::Duration::from_secs(75),
        )?;
        Ok(())
    }
}

/// # Errors
/// Refuses stop/delete when hosted devices have not been positively released.
pub fn revoke(connection: &Connection, runner: &Runner<'_>) -> Result<()> {
    runner.run(
        "Remote browser release and credential removal",
        &mut connection.command("horizon-worker-browserstack revoke"),
        std::time::Duration::from_secs(90),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_browser_credential::{CredentialLocator, RemoteCredentialStore};
    use horizon_browser::remote::CredentialReference;

    fn selected() -> horizon_cloud::BrowserStack {
        horizon_cloud::BrowserStack {
            provider: "account".into(),
            targets: ["phone".into()].into(),
            local_ports: [8080].into(),
        }
    }
    fn remote() -> RemoteBrowserConfig {
        serde_json::from_value(serde_json::json!({
            "providers":{"account":{"adapter":"browserstack","endpoint":"https://hub-cloud.browserstack.com/wd/hub",
                "authentication":{"kind":"basic","username_ref":"user","password_ref":"key"},
                "credential_bindings":{"user":{"store":"session"},"key":{"store":"session"}}}},
            "targets":{"phone":{"provider":"account","browser_name":"safari","platform_name":"ios",
                "device":{"kind":"physical","model":"iPhone 16","os_version":"18"}}}
        }))
        .unwrap()
    }
    #[test]
    fn omitted_declaration_reads_no_grants_files_or_credentials() {
        let missing = Path::new("/missing-repository");
        assert!(Prepared::for_repository(&[], missing, None, "test").unwrap().is_none());
        let repo = tempfile::tempdir().unwrap();
        assert!(Prepared::for_repository(&[], repo.path(), Some(&selected()), "test").is_err());
        let binding = Binding {
            local_repository: repo.path().into(),
            configuration_file: repo.path().join("missing.yml"),
            providers: ["another".into()].into(),
        };
        let error = Prepared::for_repository(&[binding], repo.path(), Some(&selected()), "test")
            .err()
            .unwrap();
        assert!(error.to_string().contains("exceed"));
    }
    #[test]
    fn private_payload_reuses_authorization_and_strips_machine_bindings() {
        let mut remote = remote();
        remote
            .targets
            .insert("another-device".into(), remote.targets["phone"].clone());
        let mut session = SessionCredentialStore::new();
        for (reference, secret) in [("user", "synthetic-user"), ("key", "synthetic-private-key")] {
            let reference = CredentialReference::from(reference);
            let provider = &remote.providers["account"];
            session
                .put(
                    &CredentialLocator::new(
                        &provider.endpoint,
                        &reference,
                        &provider.credential_bindings[&reference],
                    ),
                    secret.as_bytes(),
                )
                .unwrap();
        }
        let stores = CredentialStores {
            session: &session,
            os_keychain: None,
            environment: None,
        };
        let prepared = Prepared::from_config(remote, &selected(), "test", &stores).unwrap();
        assert!(prepared.targets().contains("another-device"));
        let raw = std::fs::read_to_string(prepared.payload.path()).unwrap();
        assert!(!raw.contains("synthetic-private-key"));
        assert!(!raw.contains("credential_bindings"));
        let payload: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(
            payload["authorization"]["account"]
                .as_str()
                .unwrap()
                .starts_with("Basic ")
        );
        assert_eq!(
            payload["remote"]["targets"]["phone"]["capability_extensions"]["bstack:options"]["localIdentifier"],
            "horizon-test"
        );
        assert_eq!(payload["local_ports"], serde_json::json!([8080]));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                prepared.payload.as_file().metadata().unwrap().permissions().mode() & 0o077,
                0
            );
        }
        let path = prepared.payload.path().to_path_buf();
        drop(prepared);
        assert!(!path.exists());
    }
    #[test]
    fn missing_account_values_fail_without_echoing_configuration() {
        let session = SessionCredentialStore::new();
        let stores = CredentialStores {
            session: &session,
            os_keychain: None,
            environment: None,
        };
        let error = Prepared::from_config(remote(), &selected(), "test", &stores)
            .err()
            .unwrap();
        assert!(error.to_string().contains("credentials are unavailable"));
        assert!(!error.to_string().contains("hub-cloud"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn catalog_only_serialized_payload_is_accepted_by_worker_bootstrap() {
        let mut remote = remote();
        remote.targets.clear();
        let mut selected = selected();
        selected.targets.clear();
        let mut session = SessionCredentialStore::new();
        for (reference, secret) in [("user", "synthetic-user"), ("key", "synthetic-key")] {
            let reference = CredentialReference::from(reference);
            let provider = &remote.providers["account"];
            session
                .put(
                    &CredentialLocator::new(
                        &provider.endpoint,
                        &reference,
                        &provider.credential_bindings[&reference],
                    ),
                    secret.as_bytes(),
                )
                .unwrap();
        }
        let stores = CredentialStores {
            session: &session,
            os_keychain: None,
            environment: None,
        };
        let prepared = Prepared::from_config(remote, &selected, "catalog-only", &stores).unwrap();
        let raw = std::fs::read(prepared.payload.path()).unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert!(payload["remote"].get("targets").is_none());
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cloud-worker/horizon-worker-browserstack");
        let result = std::process::Command::new("python3")
            .args(["-c", "import runpy,sys,json; worker=runpy.run_path(sys.argv[1]); worker['validate'](json.load(sys.stdin), {'provider':'account','local_ports':[8080]})"])
            .arg(script)
            .stdin(std::fs::File::open(prepared.payload.path()).unwrap())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "worker rejected the catalog-only serialized payload"
        );
    }
}
