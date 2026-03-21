use crate::panel::PanelKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentResumeMode {
    ExactSubcommand {
        subcommand: &'static str,
    },
    ExactFlag {
        flag: &'static str,
        fresh_session_flag: Option<&'static str>,
    },
    ContinueFlag {
        flag: &'static str,
    },
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentIntegrationKind {
    None,
    ClaudePluginDir,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentDefinition {
    pub id: &'static str,
    pub display_name: &'static str,
    pub icon_label: &'static str,
    pub accent_rgb: [u8; 3],
    pub default_command: &'static str,
    pub resume_mode: AgentResumeMode,
    pub integration: AgentIntegrationKind,
    pub kitty_keyboard: bool,
}

impl AgentDefinition {
    #[must_use]
    pub const fn supports_session_binding(self) -> bool {
        matches!(
            self.resume_mode,
            AgentResumeMode::ExactSubcommand { .. } | AgentResumeMode::ExactFlag { .. }
        )
    }
}

const CODEX: AgentDefinition = AgentDefinition {
    id: "codex",
    display_name: "Codex",
    icon_label: "CX",
    accent_rgb: [116, 162, 247],
    default_command: "codex",
    resume_mode: AgentResumeMode::ExactSubcommand { subcommand: "resume" },
    integration: AgentIntegrationKind::None,
    kitty_keyboard: false,
};

const CLAUDE: AgentDefinition = AgentDefinition {
    id: "claude",
    display_name: "Claude",
    icon_label: "CC",
    accent_rgb: [203, 166, 247],
    default_command: "claude",
    resume_mode: AgentResumeMode::ExactFlag {
        flag: "--resume",
        fresh_session_flag: Some("--session-id"),
    },
    integration: AgentIntegrationKind::ClaudePluginDir,
    kitty_keyboard: true,
};

const OPENCODE: AgentDefinition = AgentDefinition {
    id: "open_code",
    display_name: "OpenCode",
    icon_label: "OC",
    accent_rgb: [102, 214, 173],
    default_command: "opencode",
    resume_mode: AgentResumeMode::ExactFlag {
        flag: "--session",
        fresh_session_flag: None,
    },
    integration: AgentIntegrationKind::None,
    kitty_keyboard: true,
};

const GEMINI: AgentDefinition = AgentDefinition {
    id: "gemini",
    display_name: "Gemini",
    icon_label: "GM",
    accent_rgb: [137, 220, 235],
    default_command: "gemini",
    resume_mode: AgentResumeMode::None,
    integration: AgentIntegrationKind::None,
    kitty_keyboard: false,
};

const KILO_CODE: AgentDefinition = AgentDefinition {
    id: "kilo_code",
    display_name: "KiloCode",
    icon_label: "KC",
    accent_rgb: [235, 160, 172],
    default_command: "kilo",
    resume_mode: AgentResumeMode::ContinueFlag { flag: "--continue" },
    integration: AgentIntegrationKind::None,
    kitty_keyboard: true,
};

pub const BUILTIN_AGENT_KINDS: [PanelKind; 5] = [
    PanelKind::Codex,
    PanelKind::Claude,
    PanelKind::OpenCode,
    PanelKind::Gemini,
    PanelKind::KiloCode,
];

#[must_use]
pub const fn all_agent_kinds() -> &'static [PanelKind] {
    &BUILTIN_AGENT_KINDS
}

#[must_use]
pub const fn agent_definition(kind: PanelKind) -> Option<AgentDefinition> {
    match kind {
        PanelKind::Codex => Some(CODEX),
        PanelKind::Claude => Some(CLAUDE),
        PanelKind::OpenCode => Some(OPENCODE),
        PanelKind::Gemini => Some(GEMINI),
        PanelKind::KiloCode => Some(KILO_CODE),
        PanelKind::Shell
        | PanelKind::Ssh
        | PanelKind::Command
        | PanelKind::Editor
        | PanelKind::GitChanges
        | PanelKind::Usage => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentResumeMode, PanelKind, agent_definition};

    #[test]
    fn exact_session_binding_support_is_reserved_for_catalog_backed_agents() {
        assert!(
            agent_definition(PanelKind::Codex)
                .expect("codex agent")
                .supports_session_binding()
        );
        assert!(
            agent_definition(PanelKind::Claude)
                .expect("claude agent")
                .supports_session_binding()
        );
        assert!(
            agent_definition(PanelKind::OpenCode)
                .expect("opencode agent")
                .supports_session_binding()
        );
        assert!(
            !agent_definition(PanelKind::Gemini)
                .expect("gemini agent")
                .supports_session_binding()
        );
        assert!(
            !agent_definition(PanelKind::KiloCode)
                .expect("kilo agent")
                .supports_session_binding()
        );
    }

    #[test]
    fn kilo_uses_workspace_continue_resume_mode() {
        assert_eq!(
            agent_definition(PanelKind::KiloCode).expect("kilo agent").resume_mode,
            AgentResumeMode::ContinueFlag { flag: "--continue" }
        );
    }
}
