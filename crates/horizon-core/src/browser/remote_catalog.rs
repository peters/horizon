//! Account-bound discovery without credential-store or network work on the render thread.
use super::remote_usage::PreparedUsage;
use crate::remote_browser_credential::CredentialWorkbench;
pub use horizon_browser::provider_catalog::{
    CatalogCache, CatalogDevice, CatalogError, CatalogPage, CatalogQuery, target_provider,
};
use horizon_browser::{provider_catalog, remote::RemoteProviderProfile};

pub fn refresh(
    cache: &mut CatalogCache,
    name: &str,
    profile: &RemoteProviderProfile,
    credentials: &CredentialWorkbench,
) {
    cache.invalidate_credentials(credentials.generation());
    if !cache.needs_refresh(name, profile) {
        return;
    }
    let prepared = PreparedUsage::new(profile, credentials).map_err(|_| CatalogError::Credentials);
    start(cache, name, profile, prepared);
}

fn start(
    cache: &mut CatalogCache,
    name: &str,
    profile: &RemoteProviderProfile,
    prepared: Result<PreparedUsage, CatalogError>,
) {
    let provider = name.to_owned();
    let cloned = profile.clone();
    cache.start(name, profile, move |progress| {
        provider_catalog::validate_provider(&cloned)?;
        // An OS keychain read happens here, inside the credential stage's budget.
        let authorization = prepared?.authorization().map_err(|_| CatalogError::Credentials)?;
        provider_catalog::fetch(&provider, &cloned, authorization.header_value(), progress)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_browser_credential::FakeCredentialStore;
    use horizon_browser::provider_catalog::CatalogStage;
    use std::{sync::mpsc::channel, time::Duration};

    #[test]
    fn a_blocked_keychain_read_is_shared_then_reported_as_a_credential_stall() {
        let profile: RemoteProviderProfile = serde_json::from_str(
            r#"{"adapter":"browserstack","endpoint":"https://hub-cloud.browserstack.com/wd/hub",
                "authentication":{"kind":"bearer","token_ref":"key"},
                "credential_bindings":{"key":{"store":"os_keychain","slot":"fixture"}}}"#,
        )
        .unwrap();
        let credentials = CredentialWorkbench::with_opener(Box::new(|| Ok(Box::new(FakeCredentialStore::new()))));
        let (entered, reading) = channel();
        let (release, resume) = channel::<()>();
        let prepared = PreparedUsage::new(&profile, &credentials)
            .unwrap()
            .with_keychain(Box::new(move || {
                entered.send(()).unwrap();
                let _ = resume.recv();
                Ok(Box::new(FakeCredentialStore::new()))
            }));
        let mut cache = CatalogCache::default();
        start(&mut cache, "account", &profile, Ok(prepared));
        reading.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(cache.stage("account"), Some(CatalogStage::Credentials));
        let query = CatalogQuery {
            provider: "account".into(),
            ..Default::default()
        };
        for _ in 0..5 {
            refresh(&mut cache, "account", &profile, &credentials);
            assert!(matches!(cache.page(&profile, &query), Ok(None)), "the read is shared");
        }
        cache.advance_clock(Duration::from_secs(10));
        refresh(&mut cache, "account", &profile, &credentials);
        assert!(matches!(
            cache.page(&profile, &query),
            Err(CatalogError::CredentialsTimedOut)
        ));
        release.send(()).unwrap();
        let end = std::time::Instant::now() + Duration::from_secs(5);
        // The late answer replaces the stall once it arrives.
        while !matches!(cache.page(&profile, &query), Err(CatalogError::Credentials)) {
            cache.poll();
            assert!(std::time::Instant::now() < end, "released read did not finish");
            std::thread::yield_now();
        }
        assert_eq!(cache.stage("account"), None);
    }
}
