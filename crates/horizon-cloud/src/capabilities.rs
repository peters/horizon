//! Portable worker features, independent of any panel or tool implementation.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Codex,
    Claude,
    Grok,
}

impl Agent {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Grok => "grok",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum BrowserEngine {
    Chromium,
    Firefox,
}

impl BrowserEngine {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chromium => "chromium",
            Self::Firefox => "firefox",
        }
    }
}

/// An omitted capabilities section preserves the original worker contract.
/// An explicit section selects only its entries; `{}` is a shell-only worker.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    #[serde(default)]
    pub agents: BTreeSet<Agent>,
    #[serde(default)]
    pub browsers: BTreeSet<BrowserEngine>,
    #[serde(default)]
    pub desktop: bool,
    /// Remote device targets require an explicit machine-local credential grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browserstack: Option<BrowserStack>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BrowserStack {
    #[serde(default = "BrowserStack::default_provider")]
    pub provider: String,
    /// Optional preferred starting targets, never an allocation allowlist.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub targets: BTreeSet<String>,
    /// Only these worker-loopback ports are exposed through the private tunnel.
    #[serde(default)]
    pub local_ports: BTreeSet<u16>,
}

impl BrowserStack {
    pub const DEFAULT_PROVIDER: &str = "browserstack";
    #[must_use]
    pub fn default_provider() -> String {
        Self::DEFAULT_PROVIDER.into()
    }
}
impl Default for BrowserStack {
    fn default() -> Self {
        Self {
            provider: Self::default_provider(),
            targets: BTreeSet::new(),
            local_ports: BTreeSet::new(),
        }
    }
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            agents: [Agent::Codex, Agent::Claude, Agent::Grok].into(),
            browsers: [BrowserEngine::Chromium].into(),
            desktop: true,
            browserstack: None,
        }
    }
}

impl Capabilities {
    /// # Errors
    /// Rejects invalid remote-browser target names and local ports.
    pub fn validate(&self) -> Result<(), crate::ProfileError> {
        if let Some(browserstack) = &self.browserstack
            && (browserstack.provider.is_empty()
                || browserstack.provider.len() > 64
                || !browserstack
                    .provider
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                || browserstack.targets.len() > 16
                || browserstack.targets.iter().any(|name| {
                    name.is_empty()
                        || name.len() > 64
                        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                })
                || browserstack.local_ports.contains(&0)
                || browserstack.local_ports.len() > 16)
        {
            return Err(crate::ProfileError::Invalid(
                "BrowserStack requires named targets and valid worker-local ports",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn browser_tools(&self) -> bool {
        !self.browsers.is_empty() || self.browserstack.is_some()
    }

    #[must_use]
    pub fn permits_agent(&self, name: &str) -> bool {
        name == "shell" || self.agents.iter().any(|agent| agent.as_str() == name)
    }

    #[must_use]
    pub fn agents_argument(&self) -> String {
        self.agents
            .iter()
            .map(|agent| agent.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }

    #[must_use]
    pub fn browsers_argument(&self) -> String {
        self.browsers
            .iter()
            .map(|browser| browser.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_empty_is_minimal_and_selections_round_trip() {
        let empty: Capabilities = serde_json::from_str("{}").unwrap();
        assert!(empty.agents.is_empty() && empty.browsers.is_empty() && !empty.desktop);
        assert!(empty.permits_agent("shell"));
        assert!(!empty.permits_agent("codex"));
        let native: Capabilities = serde_json::from_str(r#"{"agents":["codex","claude"],"desktop":true}"#).unwrap();
        assert!(native.browsers.is_empty());
        assert_eq!(native.agents_argument(), "codex,claude");
        assert_eq!(
            native,
            serde_json::from_str(&serde_json::to_string(&native).unwrap()).unwrap()
        );
    }

    #[test]
    fn unsupported_capabilities_fail_without_silent_fallback() {
        for input in [
            r#"{"agents":["unknown"]}"#,
            r#"{"browsers":["safari"]}"#,
            r#"{"deskop":true}"#,
        ] {
            assert!(serde_json::from_str::<Capabilities>(input).is_err());
        }
    }
}
