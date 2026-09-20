use super::CloudGroup;
use crate::PanelKind;

impl CloudGroup {
    /// Prototype groups retain their local panel behavior. Managed groups use
    /// the immutable capability selection saved with their launch intent.
    #[must_use]
    pub fn unavailable_panel_reason(&self, kind: PanelKind) -> Option<&'static str> {
        let capabilities = &self.remote.as_ref()?.profile.capabilities;
        match kind {
            PanelKind::Codex if !capabilities.permits_agent("codex") => Some("Codex is disabled by this cloud profile"),
            PanelKind::Claude if !capabilities.permits_agent("claude") => {
                Some("Claude is disabled by this cloud profile")
            }
            PanelKind::Grok if !capabilities.permits_agent("grok") => Some("Grok is disabled by this cloud profile"),
            PanelKind::Browser if capabilities.browsers.is_empty() => {
                Some("Browsers are disabled by this cloud profile")
            }
            PanelKind::Device if !capabilities.desktop => {
                Some("Desktop viewing and control are disabled by this cloud profile")
            }
            PanelKind::Codex
            | PanelKind::Claude
            | PanelKind::Grok
            | PanelKind::Shell
            | PanelKind::Browser
            | PanelKind::Device => None,
            _ => Some("This panel type cannot run inside a cloud"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn minimal_profile_rejects_optional_panels_and_allows_shell() {
        let mut group = CloudGroup::new(1, "test".into(), "workspace".into(), ".".into(), [0.0, 0.0]);
        let mut profile = horizon_cloud::CloudConfig::parse(horizon_cloud::EXAMPLE)
            .unwrap()
            .profiles
            .remove("development")
            .unwrap();
        profile.capabilities = serde_json::from_str("{}").unwrap();
        group.remote = Some(super::super::CloudLaunch {
            deployment_started: false,
            id: "test".into(),
            revision: "a".repeat(40),
            profile_name: "test".into(),
            profile,
        });
        for kind in [
            PanelKind::Codex,
            PanelKind::Claude,
            PanelKind::Grok,
            PanelKind::Browser,
            PanelKind::Device,
        ] {
            assert!(group.unavailable_panel_reason(kind).is_some());
        }
        assert!(group.unavailable_panel_reason(PanelKind::Shell).is_none());
        group.remote.as_mut().unwrap().profile.capabilities.desktop = true;
        assert!(group.unavailable_panel_reason(PanelKind::Device).is_none());
        assert!(group.unavailable_panel_reason(PanelKind::Browser).is_some());
    }
}
