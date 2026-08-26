//! Browser-automation disclosure policy and pre-document compatibility shim.

/// How the engine treats common script-visible browser-automation signals.
///
/// Minimization is a compatibility and privacy hardening measure, not an
/// undetectability guarantee. A page can still infer automation from browser,
/// protocol, timing, network, or environment characteristics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationDisclosurePolicy {
    /// Preserve every automation signal chosen by the browser or driver.
    BrowserDefault,
    /// Minimize common standards-exposed signals before page author scripts.
    #[default]
    MinimizeCommonSignals,
}

/// Disclosure behavior established for an active backend session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationDisclosureStatus {
    /// The caller selected [`AutomationDisclosurePolicy::BrowserDefault`].
    BrowserDefault,
    /// The backend installed its common-signal minimization before navigation.
    CommonSignalsMinimized,
    /// The selected backend cannot establish pre-document minimization.
    UnsupportedByBackend,
}

impl AutomationDisclosurePolicy {
    pub(crate) const fn ready_status(self, backend: crate::BackendKind) -> AutomationDisclosureStatus {
        match (self, backend) {
            (Self::BrowserDefault, _) => AutomationDisclosureStatus::BrowserDefault,
            (Self::MinimizeCommonSignals, crate::BackendKind::SafariWebDriver) => {
                AutomationDisclosureStatus::UnsupportedByBackend
            }
            (Self::MinimizeCommonSignals, _) => AutomationDisclosureStatus::CommonSignalsMinimized,
        }
    }
}

/// A callable `WebDriver` `BiDi` preload function. It changes only the standard
/// `navigator.webdriver` value and deliberately avoids broad fingerprint
/// spoofing that would create internally inconsistent browser properties.
pub(crate) const COMMON_SIGNAL_PRELOAD_FUNCTION: &str = r#"() => {
    const prototype = globalThis.Navigator && globalThis.Navigator.prototype;
    if (!prototype) return;
    const descriptor = Object.getOwnPropertyDescriptor(prototype, "webdriver");
    if (descriptor && !descriptor.configurable) return;
    Object.defineProperty(prototype, "webdriver", {
        configurable: true,
        enumerable: descriptor ? descriptor.enumerable : true,
        get: () => false
    });
}"#;

/// Read Chromium's own Client Hint values before applying a user-agent
/// override. Reusing browser-owned data avoids inventing a second, potentially
/// contradictory platform identity.
pub(crate) const CHROMIUM_USER_AGENT_METADATA_EXPRESSION: &str = r#"navigator.userAgentData
    ? navigator.userAgentData.getHighEntropyValues([
        "architecture",
        "bitness",
        "fullVersionList",
        "model",
        "platformVersion",
        "wow64"
    ])
    : null"#;

/// Chromium exposes `navigator.userAgentData` only in trustworthy contexts.
/// A temporary hidden target uses this network-free page to read the browser's
/// own metadata before any caller-supplied page is allowed to execute.
pub(crate) const CHROMIUM_DISCLOSURE_BOOTSTRAP_URL: &str = "chrome://version/";

pub(crate) fn cdp_preload_source() -> String {
    format!("({COMMON_SIGNAL_PRELOAD_FUNCTION})()")
}

pub(crate) fn chromium_user_agent_needs_override(browser_version: &serde_json::Value) -> Result<bool, &'static str> {
    browser_version
        .get("userAgent")
        .and_then(serde_json::Value::as_str)
        .map(|user_agent| user_agent.contains("HeadlessChrome/"))
        .ok_or("Browser.getVersion omitted userAgent")
}

/// Build the narrow CDP user-agent override needed by Chromium headless.
/// The metadata was read from this same browser immediately beforehand so the
/// engine does not invent brand, platform, architecture, or version values.
pub(crate) fn chromium_user_agent_override(
    browser_version: &serde_json::Value,
    evaluated_metadata: &serde_json::Value,
) -> Result<Option<serde_json::Value>, &'static str> {
    let user_agent = browser_version
        .get("userAgent")
        .and_then(serde_json::Value::as_str)
        .ok_or("Browser.getVersion omitted userAgent")?;
    if !user_agent.contains("HeadlessChrome/") {
        return Ok(None);
    }
    let metadata = evaluated_metadata
        .pointer("/result/value")
        .and_then(serde_json::Value::as_object)
        .ok_or("Chromium omitted native userAgentData metadata")?;
    for field in ["platform", "platformVersion", "architecture", "model"] {
        if !metadata.get(field).is_some_and(serde_json::Value::is_string) {
            return Err("Chromium returned incomplete native userAgentData metadata");
        }
    }
    for field in ["brands", "fullVersionList"] {
        if !metadata.get(field).is_some_and(serde_json::Value::is_array) {
            return Err("Chromium returned incomplete native userAgentData metadata");
        }
    }
    if !metadata.get("mobile").is_some_and(serde_json::Value::is_boolean) {
        return Err("Chromium returned incomplete native userAgentData metadata");
    }
    Ok(Some(serde_json::json!({
        "userAgent": user_agent.replace("HeadlessChrome/", "Chrome/"),
        "userAgentMetadata": metadata,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safari_reports_unsupported_minimization_without_overclaiming() {
        assert_eq!(
            AutomationDisclosurePolicy::MinimizeCommonSignals.ready_status(crate::BackendKind::SafariWebDriver),
            AutomationDisclosureStatus::UnsupportedByBackend
        );
        assert_eq!(
            AutomationDisclosurePolicy::BrowserDefault.ready_status(crate::BackendKind::SafariWebDriver),
            AutomationDisclosureStatus::BrowserDefault
        );
    }

    #[test]
    fn cdp_source_invokes_the_same_preload_function_used_by_bidi() {
        let source = cdp_preload_source();
        assert!(source.starts_with('('));
        assert!(source.ends_with(")()"));
        assert!(source.contains(COMMON_SIGNAL_PRELOAD_FUNCTION));
    }

    #[test]
    fn chromium_override_removes_only_the_headless_token_and_keeps_client_hints() {
        let version = serde_json::json!({
            "userAgent": "Mozilla/5.0 Chrome-ish HeadlessChrome/151.0.7922.108 Safari/537.36"
        });
        let metadata = serde_json::json!({
            "result": {
                "type": "object",
                "value": {
                    "architecture": "x86",
                    "bitness": "64",
                    "brands": [{ "brand": "Chromium", "version": "151" }],
                    "fullVersionList": [{ "brand": "Chromium", "version": "151.0.7922.108" }],
                    "mobile": false,
                    "model": "",
                    "platform": "Linux",
                    "platformVersion": "7.0.0",
                    "wow64": false
                }
            }
        });
        let override_params = chromium_user_agent_override(&version, &metadata)
            .unwrap_or_default()
            .unwrap_or_default();

        assert_eq!(
            override_params["userAgent"],
            "Mozilla/5.0 Chrome-ish Chrome/151.0.7922.108 Safari/537.36"
        );
        assert_eq!(override_params["userAgentMetadata"], metadata["result"]["value"]);
    }

    #[test]
    fn chromium_override_leaves_a_normal_user_agent_browser_owned() {
        let version = serde_json::json!({ "userAgent": "Mozilla/5.0 Chrome/151.0.7922.108" });

        assert_eq!(chromium_user_agent_needs_override(&version), Ok(false));
        assert_eq!(chromium_user_agent_override(&version, &serde_json::json!({})), Ok(None));
        assert!(chromium_user_agent_needs_override(&serde_json::json!({})).is_err());
        assert!(chromium_user_agent_override(&serde_json::json!({}), &serde_json::json!({})).is_err());
    }

    #[test]
    fn chromium_override_rejects_incomplete_native_metadata() {
        let version = serde_json::json!({ "userAgent": "HeadlessChrome/151.0.7922.108" });

        assert_eq!(chromium_user_agent_needs_override(&version), Ok(true));
        assert!(chromium_user_agent_override(&version, &serde_json::json!({})).is_err());
    }
}
