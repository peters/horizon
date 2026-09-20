use horizon_core::{
    browser::BackendKind,
    cloud_panel::{BrowserEngine, Capabilities},
    cloud_runtime::{Error, Result},
};

pub(super) fn browser_backend(
    capabilities: &Capabilities,
    observed: Option<BackendKind>,
    previous_or_template: Option<BackendKind>,
    restoring: bool,
) -> Result<BackendKind> {
    let allowed = |backend: &BackendKind| match backend {
        BackendKind::ChromiumCdp => capabilities.browsers.contains(&BrowserEngine::Chromium),
        BackendKind::FirefoxBidi => capabilities.browsers.contains(&BrowserEngine::Firefox),
        BackendKind::SafariWebDriver => false,
    };
    // A restored identity must never become a different browser before discovery.
    if let Some(backend) = observed.or(previous_or_template.filter(|_| restoring)) {
        return allowed(&backend)
            .then_some(backend)
            .ok_or(Error::Invalid("Saved browser engine is disabled by this cloud profile"));
    }
    previous_or_template
        .filter(allowed)
        .or_else(|| {
            capabilities.browsers.first().map(|engine| match engine {
                BrowserEngine::Chromium => BackendKind::ChromiumCdp,
                BrowserEngine::Firefox => BackendKind::FirefoxBidi,
            })
        })
        .ok_or(Error::Invalid("Browsers are disabled by this cloud profile"))
}

pub(super) fn prepare_browser(
    capabilities: &Capabilities,
    available_targets: &std::collections::BTreeSet<String>,
    observed: Option<&horizon_core::browser::CloudViewState>,
    options: &mut horizon_core::PanelOptions,
) -> Result<()> {
    if let Some(observed) = observed {
        options.remote_target.clone_from(&observed.remote_target);
    }
    if options.remote_target.is_none() && observed.is_none() && capabilities.browsers.is_empty() && !options.is_restore
    {
        options.remote_target = capabilities.browserstack.as_ref().and_then(|selected| {
            selected
                .targets
                .iter()
                .find(|name| available_targets.contains(*name))
                .or_else(|| available_targets.first())
                .cloned()
        });
    }
    let backend = if let Some(target) = &options.remote_target {
        let catalog_provider = horizon_core::browser::remote_catalog::target_provider(target);
        if capabilities.browserstack.as_ref().is_none_or(|selected| {
            !available_targets.contains(target) && catalog_provider != Some(selected.provider.as_str())
        }) {
            return Err(Error::Invalid(
                "Remote browser target is not configured for this cloud account",
            ));
        }
        observed.map_or(BackendKind::ChromiumCdp, |browser| browser.backend)
    } else {
        if capabilities.browsers.is_empty() && capabilities.browserstack.is_some() && !options.is_restore {
            return Err(Error::Invalid(
                "Ask an agent to select a remote device through its browser tools",
            ));
        }
        browser_backend(
            capabilities,
            observed.map(|b| b.backend),
            options.browser_config.as_ref().map(|config| config.backend),
            options.is_restore,
        )?
    };
    options.browser_config.get_or_insert_with(Default::default).backend = backend;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_only_browser_creation_requests_device_choice() {
        let capabilities: Capabilities = serde_json::from_str(r#"{"browserstack":{"provider":"account"}}"#).unwrap();
        let mut options = horizon_core::PanelOptions::default();
        let error = prepare_browser(
            &capabilities,
            &std::collections::BTreeSet::default(),
            None,
            &mut options,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Ask an agent to select a remote device"));
        assert!(!error.to_string().contains("disabled"));
    }
    #[test]
    fn remote_only_creation_and_restore_keep_the_declared_target() {
        let capabilities: Capabilities =
            serde_json::from_str(r#"{"browserstack":{"targets":["ios_phone","android_phone"]}}"#).unwrap();
        let available = ["ios_phone".into(), "android_phone".into(), "tablet".into()].into();
        let mut options = horizon_core::PanelOptions::default();
        prepare_browser(&capabilities, &available, None, &mut options).unwrap();
        assert_eq!(options.remote_target.as_deref(), Some("android_phone"));
        let observed = horizon_core::browser::CloudViewState {
            remote_target: Some("ios_phone".into()),
            backend: BackendKind::SafariWebDriver,
            ..Default::default()
        };
        options.is_restore = true;
        prepare_browser(&capabilities, &available, Some(&observed), &mut options).unwrap();
        assert_eq!(options.remote_target.as_deref(), Some("ios_phone"));
        assert_eq!(
            options.browser_config.as_ref().unwrap().backend,
            BackendKind::SafariWebDriver
        );
        options.remote_target = Some("tablet".into());
        prepare_browser(&capabilities, &available, None, &mut options).unwrap();
        assert_eq!(options.remote_target.as_deref(), Some("tablet"));
        let unavailable: Capabilities = serde_json::from_str("{}").unwrap();
        assert!(prepare_browser(&unavailable, &available, Some(&observed), &mut options).is_err());
    }
    #[test]
    fn restore_preserves_firefox_before_discovery_and_refuses_disabled_identity() {
        let mut capabilities: Capabilities = serde_json::from_str(r#"{"browsers":["chromium","firefox"]}"#).unwrap();
        assert_eq!(
            browser_backend(&capabilities, None, Some(BackendKind::FirefoxBidi), true).unwrap(),
            BackendKind::FirefoxBidi
        );
        capabilities.browsers.remove(&BrowserEngine::Firefox);
        assert!(browser_backend(&capabilities, None, Some(BackendKind::FirefoxBidi), true).is_err());
        assert!(browser_backend(&capabilities, Some(BackendKind::FirefoxBidi), None, false).is_err());
    }
    #[test]
    fn new_panel_inherits_enabled_profile_engine_when_global_template_is_unavailable() {
        let capabilities: Capabilities = serde_json::from_str(r#"{"browsers":["firefox"]}"#).unwrap();
        assert_eq!(
            browser_backend(&capabilities, None, Some(BackendKind::ChromiumCdp), false).unwrap(),
            BackendKind::FirefoxBidi
        );
        let empty: Capabilities = serde_json::from_str("{}").unwrap();
        assert!(browser_backend(&empty, None, None, false).is_err());
    }
}
