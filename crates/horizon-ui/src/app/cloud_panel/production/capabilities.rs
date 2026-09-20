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

#[cfg(test)]
mod tests {
    use super::*;
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
