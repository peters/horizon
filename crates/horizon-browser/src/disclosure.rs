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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationDisclosureStatus {
    /// The caller selected [`AutomationDisclosurePolicy::BrowserDefault`].
    BrowserDefault,
    /// Native minimization is active. Firefox cleared its automation flags, or
    /// Chromium started with the standard automation flag suppressed.
    CommonSignalsMinimized,
    /// Firefox installed the script getter because the native flag clear was
    /// unavailable. Sign-in pages can reject that getter.
    PreloadFallback,
    /// The selected backend cannot establish pre-document minimization.
    UnsupportedByBackend,
    /// An older manifest omitted this field. This is not an established result.
    #[default]
    Unreported,
}

impl AutomationDisclosureStatus {
    /// Stable public name. UI, CLI, and MCP use this spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BrowserDefault => "browser_default",
            Self::CommonSignalsMinimized => "common_signals_minimized",
            Self::PreloadFallback => "preload_fallback",
            Self::UnsupportedByBackend => "unsupported_by_backend",
            Self::Unreported => "unreported",
        }
    }
}

/// Status a session can publish after startup. Minimized local Firefox reports
/// the native clear and the preload getter as different outcomes. Remote
/// Firefox is classic `WebDriver`, so minimization is unsupported there.
#[must_use]
pub(crate) fn established_disclosure_status(
    policy: AutomationDisclosurePolicy,
    backend: crate::BackendKind,
    firefox_bidi: bool,
    native_cleared: bool,
) -> AutomationDisclosureStatus {
    if backend == crate::BackendKind::FirefoxBidi
        && !firefox_bidi
        && policy == AutomationDisclosurePolicy::MinimizeCommonSignals
    {
        return AutomationDisclosureStatus::UnsupportedByBackend;
    }
    if firefox_bidi && policy == AutomationDisclosurePolicy::MinimizeCommonSignals {
        if native_cleared {
            AutomationDisclosureStatus::CommonSignalsMinimized
        } else {
            AutomationDisclosureStatus::PreloadFallback
        }
    } else {
        policy.ready_status(backend)
    }
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

/// Chrome-privileged Firefox script that clears the content-process automation
/// flags read by `Navigator::Webdriver()`. The native getter stays in place and
/// returns false. A page-world replacement getter is itself a detection signal,
/// so this is the preferred minimization path.
///
/// Current Firefox reads `IsBrowserAutomationRunning` and also publishes the
/// legacy `Active` keys. Those keys still back `Marionette.running` and
/// `RemoteAgent.running`, so the script returns after the first present pair.
/// Firefox ESR 140 reads only `Active` (`Navigator::Webdriver` calls
/// `GetRunning()`), and that pair is the fallback. Shared data accepts any
/// key, so a missing key must not be created: writing it and reading it back
/// would report success while the native getter stays true. A pair is changed
/// only when both keys are already booleans. No recognized pair means the
/// preload fallback has to run.
pub(crate) const FIREFOX_NATIVE_AUTOMATION_FLAG_SCRIPT: &str = r#"const pairs = [
    [
        "Marionette:IsBrowserAutomationRunning",
        "RemoteAgent:IsBrowserAutomationRunning"
    ],
    [
        "Marionette:Active",
        "RemoteAgent:Active"
    ]
];
for (const keys of pairs) {
    const present = keys.every((key) => typeof Services.ppmm.sharedData.get(key) === "boolean");
    if (!present) {
        continue;
    }
    for (const key of keys) {
        Services.ppmm.sharedData.set(key, false);
    }
    Services.ppmm.sharedData.flush();
    return keys.every((key) => Services.ppmm.sharedData.get(key) === false);
}
return false;"#;

/// True when the chrome-context script reported both automation flags cleared.
#[must_use]
pub(crate) fn firefox_native_automation_flag_cleared(response: &serde_json::Value) -> bool {
    response.get("value").and_then(serde_json::Value::as_bool) == Some(true)
}

/// Fallback Firefox preload used only when the native automation flag cannot
/// be cleared. Chromium relies on `--disable-blink-features=AutomationControlled`
/// instead of this getter, because a script-defined `navigator.webdriver`
/// accessor is itself a detection signal. The function changes only the
/// standard value and deliberately avoids broad fingerprint spoofing.
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
        "userAgentMetadata": rewrite_headless_chrome_brands(metadata),
    })))
}

/// Rename only the `HeadlessChrome` Client Hint brand. Platform, architecture,
/// versions, GREASE brands, and other native fields stay browser-owned.
fn rewrite_headless_chrome_brands(metadata: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    let mut rewritten = serde_json::Value::Object(metadata.clone());
    for field in ["brands", "fullVersionList"] {
        let Some(serde_json::Value::Array(list)) = rewritten.get_mut(field) else {
            continue;
        };
        for entry in list {
            if entry.get("brand").and_then(serde_json::Value::as_str) == Some("HeadlessChrome") {
                entry["brand"] = serde_json::Value::String("Chrome".to_string());
            }
        }
    }
    rewritten
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimized_firefox_reports_preload_fallback_until_the_native_flag_clears() {
        use AutomationDisclosurePolicy::{BrowserDefault, MinimizeCommonSignals};
        use AutomationDisclosureStatus::{
            BrowserDefault as DefaultStatus, CommonSignalsMinimized, PreloadFallback, UnsupportedByBackend,
        };

        let firefox = crate::BackendKind::FirefoxBidi;
        assert_eq!(
            established_disclosure_status(MinimizeCommonSignals, firefox, true, true),
            CommonSignalsMinimized
        );
        assert_eq!(
            established_disclosure_status(MinimizeCommonSignals, firefox, true, false),
            PreloadFallback
        );
        assert_eq!(
            established_disclosure_status(BrowserDefault, firefox, true, false),
            DefaultStatus
        );
        assert_eq!(
            established_disclosure_status(MinimizeCommonSignals, firefox, false, false),
            UnsupportedByBackend
        );
        assert_eq!(
            established_disclosure_status(BrowserDefault, firefox, false, false),
            DefaultStatus
        );
        assert_eq!(
            established_disclosure_status(MinimizeCommonSignals, crate::BackendKind::ChromiumCdp, false, false),
            CommonSignalsMinimized
        );
        assert_eq!(PreloadFallback.as_str(), "preload_fallback");
        assert_eq!(AutomationDisclosureStatus::Unreported.as_str(), "unreported");
        assert_eq!(
            serde_json::to_string(&PreloadFallback).ok().as_deref(),
            Some("\"preload_fallback\"")
        );
    }

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
    fn firefox_preload_only_patches_navigator_webdriver() {
        assert!(COMMON_SIGNAL_PRELOAD_FUNCTION.contains("webdriver"));
        assert!(COMMON_SIGNAL_PRELOAD_FUNCTION.contains("get: () => false"));
        assert!(!COMMON_SIGNAL_PRELOAD_FUNCTION.contains("userAgent"));
    }

    #[test]
    fn firefox_native_flag_script_only_clears_the_automation_keys() {
        let script = FIREFOX_NATIVE_AUTOMATION_FLAG_SCRIPT;
        assert!(script.contains("Marionette:IsBrowserAutomationRunning"));
        assert!(script.contains("RemoteAgent:IsBrowserAutomationRunning"));
        assert!(script.contains("Marionette:Active"));
        assert!(script.contains("RemoteAgent:Active"));
        let current = script.find("IsBrowserAutomationRunning").expect("current pair");
        let legacy = script.find("Marionette:Active").expect("esr pair");
        assert!(current < legacy, "current Firefox must be preferred over Active");
        let check = script
            .find("typeof Services.ppmm.sharedData.get(key) === \"boolean\"")
            .expect("boolean check");
        let write = script.find("sharedData.set(key, false)").expect("flag write");
        let stop = script
            .find("return keys.every((key) => Services.ppmm.sharedData.get(key) === false)")
            .expect("stop after the first present pair");
        assert!(check < write, "missing keys must not be created");
        assert!(write < stop, "a present pair must be checked before the next pair");
        assert!(!script.contains("cleared"), "a later pair must not also be cleared");
        assert!(!script.contains("userAgent"));
        assert!(!FIREFOX_NATIVE_AUTOMATION_FLAG_SCRIPT.contains("toString"));
        assert!(firefox_native_automation_flag_cleared(
            &serde_json::json!({ "value": true })
        ));
        assert!(!firefox_native_automation_flag_cleared(
            &serde_json::json!({ "value": false })
        ));
        assert!(!firefox_native_automation_flag_cleared(&serde_json::json!({})));
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
                    "brands": [
                        { "brand": "HeadlessChrome", "version": "151" },
                        { "brand": "Chromium", "version": "151" },
                        { "brand": "Not.A/Brand", "version": "24" }
                    ],
                    "fullVersionList": [
                        { "brand": "HeadlessChrome", "version": "151.0.7922.108" },
                        { "brand": "Chromium", "version": "151.0.7922.108" },
                        { "brand": "Not.A/Brand", "version": "24.0.0.0" }
                    ],
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
        assert_eq!(
            override_params["userAgentMetadata"]["brands"],
            serde_json::json!([
                { "brand": "Chrome", "version": "151" },
                { "brand": "Chromium", "version": "151" },
                { "brand": "Not.A/Brand", "version": "24" }
            ])
        );
        assert_eq!(
            override_params["userAgentMetadata"]["fullVersionList"],
            serde_json::json!([
                { "brand": "Chrome", "version": "151.0.7922.108" },
                { "brand": "Chromium", "version": "151.0.7922.108" },
                { "brand": "Not.A/Brand", "version": "24.0.0.0" }
            ])
        );
        assert_eq!(override_params["userAgentMetadata"]["platform"], "Linux");
        assert_eq!(override_params["userAgentMetadata"]["architecture"], "x86");
        assert!(!override_params.to_string().contains("HeadlessChrome"));
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
