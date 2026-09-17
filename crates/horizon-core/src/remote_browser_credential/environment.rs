//! Snapshot explicitly named environment credentials into an in-memory sink.
//! Values are never written back, logged, or serialized. Normal process
//! environment inheritance is unchanged.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use horizon_browser::remote::CredentialStoreKind;
use zeroize::Zeroizing;

use super::{CredentialLocator, RemoteCredentialError, RemoteCredentialStore, Sealed, SecretSink, validate_secret};

/// In-memory snapshot of named process-environment values. Read-only.
#[derive(Default)]
pub struct EnvironmentCredentialStore {
    values: BTreeMap<String, Zeroizing<Vec<u8>>>,
}

impl EnvironmentCredentialStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    fn from_values(values: BTreeMap<String, Vec<u8>>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|(name, bytes)| (name, Zeroizing::new(bytes)))
                .collect(),
        }
    }

    /// Copy currently set variables into this store. Names already snapshotted
    /// are left unchanged. Names no longer listed are dropped (and zeroized).
    pub fn capture_from_process(&mut self, names: &BTreeSet<String>) {
        self.values.retain(|name, _| names.contains(name));
        for name in names {
            if self.values.contains_key(name) {
                continue;
            }
            if let Some(value) = std::env::var_os(name) {
                self.values
                    .insert(name.clone(), Zeroizing::new(value.into_encoded_bytes()));
            }
        }
    }

    fn lookup<'a>(&'a self, locator: &CredentialLocator) -> Option<&'a Zeroizing<Vec<u8>>> {
        locator.slot.as_ref().and_then(|name| self.values.get(name))
    }
}

impl Sealed for EnvironmentCredentialStore {}

impl RemoteCredentialStore for EnvironmentCredentialStore {
    fn kind(&self) -> CredentialStoreKind {
        CredentialStoreKind::Environment
    }

    fn put(&mut self, _: &CredentialLocator, _: &[u8]) -> Result<(), RemoteCredentialError> {
        Err(RemoteCredentialError::StoreUnavailable)
    }

    fn delete(&mut self, _: &CredentialLocator) -> Result<(), RemoteCredentialError> {
        Err(RemoteCredentialError::StoreUnavailable)
    }

    fn contains(&self, locator: &CredentialLocator) -> Result<bool, RemoteCredentialError> {
        Ok(self.lookup(locator).is_some_and(|value| validate_secret(value).is_ok()))
    }

    fn with_secret(&self, locator: &CredentialLocator, sink: &mut dyn SecretSink) -> Result<(), RemoteCredentialError> {
        let value = self.lookup(locator).ok_or(RemoteCredentialError::Missing)?;
        validate_secret(value)?;
        sink.accept(value)
    }
}

impl fmt::Debug for EnvironmentCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnvironmentCredentialStore")
            .field("entries", &self.values.len())
            .finish()
    }
}

impl Drop for EnvironmentCredentialStore {
    fn drop(&mut self) {
        self.values.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use horizon_browser::remote::{
        ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
        RemoteAuthentication, RemoteBrowserConfig, RemoteProviderProfile, RemoteSessionLimits,
    };

    use super::EnvironmentCredentialStore;
    use crate::remote_browser_credential::{
        CredentialLocator, CredentialState, CredentialStores, CredentialWorkbench, FakeCredentialStore,
        RemoteCredentialError, RemoteCredentialStore, SessionCredentialStore, readiness, resolve_authorization,
    };

    fn endpoint() -> ControlEndpoint {
        ControlEndpoint::parse("https://grid.example.net/wd/hub").expect("endpoint")
    }

    fn environment_profile() -> RemoteProviderProfile {
        let mut credential_bindings = BTreeMap::new();
        credential_bindings.insert(
            CredentialReference::from("user"),
            CredentialBinding {
                store: CredentialStoreKind::Environment,
                slot: Some("REMOTE_BROWSER_USERNAME".into()),
            },
        );
        credential_bindings.insert(
            CredentialReference::from("key"),
            CredentialBinding {
                store: CredentialStoreKind::Environment,
                slot: Some("REMOTE_BROWSER_ACCESS_KEY".into()),
            },
        );
        RemoteProviderProfile {
            adapter: RemoteAdapterKind::Webdriver,
            endpoint: endpoint(),
            authentication: RemoteAuthentication::Basic {
                username_ref: CredentialReference::from("user"),
                password_ref: CredentialReference::from("key"),
            },
            credential_bindings,
            limits: RemoteSessionLimits::default(),
        }
    }

    fn locator(profile: &RemoteProviderProfile, reference: &str) -> CredentialLocator {
        let reference = CredentialReference::from(reference);
        CredentialLocator::new(&profile.endpoint, &reference, &profile.credential_bindings[&reference])
    }

    fn environment_store_with(user: &[u8], key: &[u8]) -> EnvironmentCredentialStore {
        let mut values = BTreeMap::new();
        values.insert("REMOTE_BROWSER_USERNAME".into(), user.to_vec());
        values.insert("REMOTE_BROWSER_ACCESS_KEY".into(), key.to_vec());
        EnvironmentCredentialStore::from_values(values)
    }

    #[test]
    fn environment_bindings_resolve_from_the_named_snapshot_and_never_expose_values() {
        let profile = environment_profile();
        let environment = environment_store_with(b"alice", b"s3cret-key");
        let stores = CredentialStores {
            session: &SessionCredentialStore::new(),
            os_keychain: None,
            environment: Some(&environment),
        };
        let authorization = resolve_authorization(&profile, &stores)
            .expect("resolves")
            .expect("basic header");
        assert_eq!(authorization.header_value(), "Basic YWxpY2U6czNjcmV0LWtleQ==");
        let debug = format!("{authorization:?}");
        assert!(debug.contains("<redacted>"), "{debug}");
        assert!(!debug.contains("s3cret"), "{debug}");
        assert!(
            !format!("{environment:?}").contains("s3cret"),
            "store debug is value-free"
        );
        let report = readiness(&profile, &stores);
        assert!(
            report
                .iter()
                .all(|entry| entry.store == CredentialStoreKind::Environment)
        );
        assert!(report.iter().all(|entry| entry.state == CredentialState::Present));
    }

    #[test]
    fn environment_bindings_do_not_fall_back_to_session_or_keychain() {
        let profile = environment_profile();
        let mut session = SessionCredentialStore::new();
        session.put(&locator(&profile, "user"), b"alice").expect("session user");
        session
            .put(&locator(&profile, "key"), b"s3cret-key")
            .expect("session key");
        let mut keychain = FakeCredentialStore::new();
        keychain.put(&locator(&profile, "user"), b"alice").expect("os user");
        keychain.put(&locator(&profile, "key"), b"s3cret-key").expect("os key");
        let environment = EnvironmentCredentialStore::new();
        let stores = CredentialStores {
            session: &session,
            os_keychain: Some(&keychain),
            environment: Some(&environment),
        };
        let report = readiness(&profile, &stores);
        assert!(report.iter().all(|entry| entry.state == CredentialState::Missing));
        let error = resolve_authorization(&profile, &stores).expect_err("no env snapshot");
        assert_eq!(error.reference.as_str(), "user");
        assert_eq!(error.variable.as_deref(), Some("REMOTE_BROWSER_USERNAME"));
        assert_eq!(error.error, RemoteCredentialError::Missing);
        assert!(error.to_string().contains("REMOTE_BROWSER_USERNAME"), "{error}");
        assert!(!error.to_string().contains("alice"), "{error}");
        assert!(!error.to_string().contains("s3cret"), "{error}");
    }

    #[test]
    fn session_bindings_do_not_read_the_environment_store() {
        let mut profile = environment_profile();
        for binding in profile.credential_bindings.values_mut() {
            binding.store = CredentialStoreKind::Session;
            binding.slot = None;
        }
        let environment = environment_store_with(b"alice", b"s3cret-key");
        let stores = CredentialStores {
            session: &SessionCredentialStore::new(),
            os_keychain: None,
            environment: Some(&environment),
        };
        assert!(
            readiness(&profile, &stores)
                .iter()
                .all(|entry| entry.state == CredentialState::Missing)
        );
        assert_eq!(
            resolve_authorization(&profile, &stores)
                .expect_err("session empty")
                .error,
            RemoteCredentialError::Missing
        );
    }

    #[test]
    fn missing_and_empty_environment_values_name_the_variable_not_the_value() {
        let profile = environment_profile();
        let mut values = BTreeMap::new();
        values.insert("REMOTE_BROWSER_USERNAME".into(), b"alice".to_vec());
        values.insert("REMOTE_BROWSER_ACCESS_KEY".into(), Vec::new());
        let environment = EnvironmentCredentialStore::from_values(values);
        let stores = CredentialStores {
            session: &SessionCredentialStore::new(),
            os_keychain: None,
            environment: Some(&environment),
        };
        let report = readiness(&profile, &stores);
        assert_eq!(report[0].state, CredentialState::Present);
        assert_eq!(report[1].state, CredentialState::Missing);
        let error = resolve_authorization(&profile, &stores).expect_err("empty key");
        assert_eq!(error.variable.as_deref(), Some("REMOTE_BROWSER_ACCESS_KEY"));
        assert_eq!(error.error, RemoteCredentialError::InvalidValue);
        assert!(!error.to_string().contains("alice"), "{error}");
    }

    #[test]
    fn launch_environment_bindings_are_loaded_and_unused_snapshots_are_dropped() {
        const CHILD: &str = "HORIZON_TEST_ENV_CREDENTIAL_CHILD";
        let Ok(suffix) = std::env::var(CHILD) else {
            let suffix = std::process::id().to_string();
            let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
                .args([
                    "--exact",
                    "remote_browser_credential::environment::tests::launch_environment_bindings_are_loaded_and_unused_snapshots_are_dropped",
                ])
                .env(CHILD, &suffix)
                .env(format!("HORIZON_TEST_REMOTE_USER_{suffix}"), "alice")
                .env(format!("HORIZON_TEST_REMOTE_KEY_{suffix}"), "s3cret-key")
                .output()
                .expect("isolated launch environment");
            assert!(output.status.success(), "{output:?}");
            return;
        };
        let mut profile = environment_profile();
        for (reference, name) in [("user", "USER"), ("key", "KEY")] {
            profile
                .credential_bindings
                .get_mut(&CredentialReference::from(reference))
                .expect("binding")
                .slot = Some(format!("HORIZON_TEST_REMOTE_{name}_{suffix}"));
        }
        let mut remote = RemoteBrowserConfig::default();
        remote.providers.insert("grid".into(), profile.clone());
        let mut workbench = CredentialWorkbench::with_opener(Box::new(|| {
            Ok(Box::new(FakeCredentialStore::new()) as Box<dyn RemoteCredentialStore + Send>)
        }));
        workbench.load_environment_bindings(&remote);
        let stores = CredentialStores {
            session: workbench.session_store(),
            os_keychain: None,
            environment: Some(workbench.environment_store()),
        };
        assert_eq!(
            resolve_authorization(&profile, &stores)
                .expect("launch variables resolve")
                .expect("basic header")
                .header_value(),
            "Basic YWxpY2U6czNjcmV0LWtleQ=="
        );
        remote.providers.clear();
        workbench.load_environment_bindings(&remote);
        assert!(
            workbench
                .readiness(&profile)
                .iter()
                .all(|entry| entry.state == CredentialState::Missing)
        );
        remote.providers.insert("grid".into(), profile.clone());
        workbench.load_environment_bindings(&remote);
        assert!(
            workbench
                .readiness(&profile)
                .iter()
                .all(|entry| entry.state == CredentialState::Present)
        );
    }

    #[test]
    fn workbench_reports_each_missing_environment_reference() {
        let mut profile = environment_profile();
        let user_var = format!("HORIZON_TEST_REMOTE_BROWSER_USER_{}", std::process::id());
        let key_var = format!("HORIZON_TEST_REMOTE_BROWSER_KEY_{}", std::process::id());
        profile
            .credential_bindings
            .get_mut(&CredentialReference::from("user"))
            .expect("user")
            .slot = Some(user_var.clone());
        profile
            .credential_bindings
            .get_mut(&CredentialReference::from("key"))
            .expect("key")
            .slot = Some(key_var.clone());
        let mut remote = RemoteBrowserConfig::default();
        remote.providers.insert("grid".into(), profile.clone());
        let mut workbench = CredentialWorkbench::with_opener(Box::new(|| {
            Ok(Box::new(FakeCredentialStore::new()) as Box<dyn RemoteCredentialStore + Send>)
        }));
        workbench.load_environment_bindings(&remote);
        let report = workbench.readiness(&profile);
        assert!(
            report.iter().all(|entry| {
                entry.store == CredentialStoreKind::Environment && entry.state == CredentialState::Missing
            }),
            "{report:?}"
        );
    }

    #[test]
    fn multiple_providers_and_mixed_sources_resolve_independently() {
        let first = environment_profile();
        let mut second = first.clone();
        second.endpoint = ControlEndpoint::parse("https://second.example.net/wd/hub").expect("endpoint");
        for binding in second.credential_bindings.values_mut() {
            binding.slot = binding.slot.as_ref().map(|name| format!("SECOND_{name}"));
        }
        let mut environment = environment_store_with(b"alice", b"first-key");
        environment.values.extend(
            EnvironmentCredentialStore::from_values(BTreeMap::from([
                ("SECOND_REMOTE_BROWSER_USERNAME".into(), b"bob".to_vec()),
                ("SECOND_REMOTE_BROWSER_ACCESS_KEY".into(), b"second-key".to_vec()),
            ]))
            .values
            .clone(),
        );
        let mut session = SessionCredentialStore::new();
        let mut keychain = FakeCredentialStore::new();
        let key = CredentialReference::from("key");
        let mut mixed = first.clone();
        mixed.credential_bindings.get_mut(&key).expect("key").store = CredentialStoreKind::Session;
        session
            .put(&locator(&mixed, "key"), b"session-key")
            .expect("session value");
        keychain
            .put(&locator(&mixed, "key"), b"keychain-key")
            .expect("keychain value");
        let stores = CredentialStores {
            session: &session,
            os_keychain: Some(&keychain),
            environment: Some(&environment),
        };
        for (profile, expected, origin) in [
            (&first, "Basic YWxpY2U6Zmlyc3Qta2V5", "https://grid.example.net"),
            (&second, "Basic Ym9iOnNlY29uZC1rZXk=", "https://second.example.net"),
            (&mixed, "Basic YWxpY2U6c2Vzc2lvbi1rZXk=", "https://grid.example.net"),
        ] {
            let header = resolve_authorization(profile, &stores).expect("ready").expect("basic");
            assert_eq!(header.header_value(), expected);
            assert_eq!(header.origin(), origin);
        }
        mixed.credential_bindings.get_mut(&key).expect("key").store = CredentialStoreKind::OsKeychain;
        assert_eq!(
            resolve_authorization(&mixed, &stores)
                .expect("ready")
                .expect("basic")
                .header_value(),
            "Basic YWxpY2U6a2V5Y2hhaW4ta2V5"
        );
        // Sharing a reference name never selects another provider's variable.
        environment.values.remove("SECOND_REMOTE_BROWSER_ACCESS_KEY");
        let stores = CredentialStores {
            session: &session,
            os_keychain: None,
            environment: Some(&environment),
        };
        assert!(resolve_authorization(&first, &stores).is_ok());
        let error = resolve_authorization(&second, &stores).expect_err("second key missing");
        assert_eq!(error.variable.as_deref(), Some("SECOND_REMOTE_BROWSER_ACCESS_KEY"));
    }

    #[test]
    fn invalid_environment_values_are_redacted_for_both_authentication_schemes() {
        let mut profile = environment_profile();
        for (user, key, reference, error) in [
            (
                b"bad:user".to_vec(),
                b"secret".to_vec(),
                "user",
                RemoteCredentialError::InvalidUsername,
            ),
            (
                b"alice".to_vec(),
                b"bad\r\nsecret".to_vec(),
                "key",
                RemoteCredentialError::NotHeaderSafe,
            ),
            (
                b"alice".to_vec(),
                vec![255],
                "key",
                RemoteCredentialError::NotHeaderSafe,
            ),
            (
                b"alice".to_vec(),
                vec![b'x'; super::super::MAX_SECRET_BYTES + 1],
                "key",
                RemoteCredentialError::InvalidValue,
            ),
        ] {
            let environment = environment_store_with(&user, &key);
            let stores = CredentialStores {
                session: &SessionCredentialStore::new(),
                os_keychain: None,
                environment: Some(&environment),
            };
            let actual = resolve_authorization(&profile, &stores).expect_err("invalid credential");
            assert_eq!(actual.reference.as_str(), reference);
            assert_eq!(actual.error, error);
            assert!(!format!("{actual:?} {actual}").contains("secret"));
        }
        profile.authentication = RemoteAuthentication::Bearer {
            token_ref: CredentialReference::from("key"),
        };
        let environment = environment_store_with(b"unused", b"invalid token-secret");
        let stores = CredentialStores {
            session: &SessionCredentialStore::new(),
            os_keychain: None,
            environment: Some(&environment),
        };
        let error = resolve_authorization(&profile, &stores).expect_err("invalid bearer");
        assert_eq!(error.error, RemoteCredentialError::InvalidBearerToken);
        assert_eq!(error.variable.as_deref(), Some("REMOTE_BROWSER_ACCESS_KEY"));
        assert!(!format!("{error:?} {error}").contains("token-secret"));
    }
}
