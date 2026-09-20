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
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            agents: [Agent::Codex, Agent::Claude, Agent::Grok].into(),
            browsers: [BrowserEngine::Chromium].into(),
            desktop: true,
        }
    }
}

impl Capabilities {
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
