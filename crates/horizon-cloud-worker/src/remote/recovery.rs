//! Rebind private saved identities to the granted provider; never allocate here.
use super::{Allocations, Configuration, Held, configuration_for};
use horizon_browser::{
    RemoteAllocation, RemoteAuthorizationHeader,
    remote::{DeviceRequirement, RemoteAdapterKind, RemoteTargetProfile},
};
use std::{collections::BTreeMap, io, sync::Arc};

impl Allocations {
    pub(crate) fn reconcile_retained_for_host(&mut self, capabilities: &horizon_cloud::Capabilities) {
        self.restore_retained(capabilities);
        self.reconcile_restored();
    }
    fn reconcile_restored(&self) {
        for held in self.held.values().filter(|held| held.restored) {
            held.allocation.reconcile();
        }
    }
    pub(super) fn restore_retained(&mut self, capabilities: &horizon_cloud::Capabilities) {
        if self.orphans.is_empty() {
            return;
        }
        let Ok(config) = configuration_for(capabilities) else {
            return;
        };
        self.restore_with(&config);
    }
    fn restore_with(&mut self, config: &Configuration) {
        let mut restored = Vec::new();
        for (id, journal) in &self.orphans {
            let Some(owner) = &journal.owner else { continue };
            let result = (|| -> io::Result<_> {
                let provider = config
                    .remote
                    .providers
                    .get(&journal.provider)
                    .filter(|provider| provider.adapter == RemoteAdapterKind::Browserstack)
                    .ok_or_else(|| io::Error::other("Original remote provider is unavailable"))?;
                let header = config
                    .authorization
                    .get(&journal.provider)
                    .ok_or_else(|| io::Error::other("Original remote credential is unavailable"))?;
                let quota = config
                    .quota_keys
                    .get(&journal.provider)
                    .ok_or_else(|| io::Error::other("Original remote account identity is unavailable"))?;
                // Recovery uses only transport policy, never these inert target fields.
                let target = RemoteTargetProfile {
                    provider: journal.provider.clone(),
                    browser_name: "chrome".into(),
                    platform_name: "recovery".into(),
                    device: DeviceRequirement::default(),
                    capability_extensions: BTreeMap::new(),
                };
                let authorization = RemoteAuthorizationHeader::new(header.as_str().into()).map_err(io::Error::other)?;
                let request = horizon_browser::remote_config::configured_remote_request(
                    provider,
                    &target,
                    "recovery",
                    Some(Arc::new(authorization)),
                    quota.clone(),
                )
                .map_err(io::Error::other)?;
                RemoteAllocation::restore_journal(&self.root.join("identities").join(id), &request)
            })();
            let Ok(allocation) = result else { continue };
            if allocation.reference() != journal.reference {
                continue;
            }
            allocation.record_admission(horizon_browser_control::manifest::host_instance(), owner, "cloud");
            restored.push((id.clone(), journal.provider.clone(), owner.clone(), allocation));
        }
        for (id, provider, owner, allocation) in restored {
            self.orphans.remove(&id);
            self.held.insert(
                allocation.reference().into(),
                Held {
                    id,
                    provider,
                    owner,
                    allocation,
                    restored: true,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };

    #[test]
    fn restart_recovery_preserves_the_hold_until_exact_provider_release() {
        for owner in ["owner", ""] {
            verify_restart(owner);
        }
    }
    fn verify_restart(owner: &str) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/wd/hub", listener.local_addr().unwrap());
        let config: Configuration = serde_json::from_value(json!({
            "version":1,"remote":{"providers":{"account":{"adapter":"browserstack","endpoint":endpoint}},"targets":{}},
            "authorization":{"account":"Basic fixture"},"quota_keys":{"account":"quota"},
            "local_identifier":"horizon-fixture","local_ports":[]
        }))
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("identities")).unwrap();
        std::fs::write(
            root.path().join("phone"),
            json!({"provider":"account","reference":"reference","owner":owner}).to_string(),
        )
        .unwrap();
        let identity = root.path().join("identities/phone");
        std::fs::write(&identity, json!({"version":1,"endpoint":endpoint,"quota_key":"quota","reference":"reference","session":"exact","released":false}).to_string()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&identity, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let mut allocations = Allocations::new(root.path().into()).unwrap();
        allocations.restore_with(&config);
        assert!(allocations.orphans.is_empty());
        assert!(allocations.ensure_recoverable().is_err());
        assert!(allocations.confirm_closed("phone").is_err());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut bytes = [0; 4096];
            let n = stream.read(&mut bytes).unwrap();
            assert!(String::from_utf8_lossy(&bytes[..n]).starts_with("GET /wd/hub/session/exact/url "));
            let body = r#"{"value":{"error":"invalid session id"}}"#;
            write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let held = &allocations.held["reference"];
        let host = horizon_browser_control::manifest::host_instance();
        assert!(!held.allocation.reconcile_for(host, "other-owner", "cloud", false));
        if owner.is_empty() {
            assert!(!held.allocation.reconcile_for(host, "agent", "cloud", false));
            allocations.reconcile_restored();
        } else {
            assert!(held.allocation.reconcile_for(host, "owner", "cloud", true));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !held.allocation.is_released() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        server.join().unwrap();
        allocations.confirm_closed("phone").unwrap();
        assert!(allocations.ensure_recoverable().is_ok());
        assert!(!identity.exists());
        assert!(!root.path().join("phone").exists());
        let provider = &config.remote.providers["account"];
        let target = RemoteTargetProfile {
            provider: "account".into(),
            browser_name: "chrome".into(),
            platform_name: "test".into(),
            device: DeviceRequirement::default(),
            capability_extensions: BTreeMap::new(),
        };
        let request =
            horizon_browser::remote_config::configured_remote_request(provider, &target, "test", None, "quota".into())
                .unwrap();
        std::fs::create_dir(root.path().join("identities/failed.pending")).unwrap();
        assert!(allocations.insert("failed", "owner", &request).is_err());
        assert!(!root.path().join("failed").exists());
        assert!(
            Allocations::new(root.path().into())
                .unwrap()
                .ensure_recoverable()
                .is_ok()
        );
        allocations.insert("cancelled", "owner", &request).unwrap();
        allocations.cancel_start("cancelled");
        assert!(!root.path().join("cancelled").exists());
        assert!(!root.path().join("identities/cancelled").exists());
    }
}
