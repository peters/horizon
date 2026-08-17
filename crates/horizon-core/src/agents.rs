use crate::panel::PanelKind;

/// Coarse activity state of an agent panel, derived from what the agent TUI
/// renders in the terminal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentStatus {
    /// No working indicator visible; the agent is at its prompt (or the panel
    /// is not an agent panel).
    #[default]
    Idle,
    /// The agent TUI is rendering its working indicator (streaming a response,
    /// running a command, compacting, retrying, ...).
    Working,
}

/// Number of visible bottom screen rows scanned for agent working indicators.
/// Agent TUIs pin their status line a few rows above the bottom input/footer
/// area, so a small bottom window catches the line without scanning the whole
/// screen.
pub const AGENT_WORKING_SCAN_ROWS: usize = 6;

/// Status words agent TUIs print next to their working spinner. Covers the
/// working/thinking labels of the built-in agents (Pi, Codex, Claude Code,
/// Gemini, `OpenCode`, Grok) plus the retry/reuse/compaction variants.
const WORKING_KEYWORDS: [&str; 7] = [
    "Working",
    "Thinking",
    "Compacting",
    "Retrying",
    "Reusing",
    "Summarizing",
    "Context overflow",
];

/// Interrupt/stop hints the working status line always carries, so custom
/// working messages without a status word still count.
const WORKING_HINTS: [&str; 3] = ["to interrupt", "to stop", "to cancel"];

fn is_spinner_glyph(ch: char) -> bool {
    // Braille spinner frames (pi-tui, ink, codex) and star spinners (Claude
    // Code uses \u{273B}).
    ('\u{2800}'..='\u{28FF}').contains(&ch) || ('\u{2737}'..='\u{273B}').contains(&ch)
}

/// A terminal line counts as a working indicator when it starts with an
/// animated spinner glyph and carries a working keyword or the usual
/// interrupt/stop/cancel hint. Requiring the spinner prefix keeps ordinary
/// output text that merely mentions "Working" from flipping the status.
#[must_use]
pub fn is_agent_working_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(first) = trimmed.chars().next() else {
        return false;
    };
    if !is_spinner_glyph(first) {
        return false;
    }
    WORKING_KEYWORDS.iter().any(|keyword| trimmed.contains(keyword))
        || WORKING_HINTS.iter().any(|hint| trimmed.contains(hint))
}

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
pub enum AgentSessionValidationMode {
    None,
    ParentThreadRoots,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentDefinition {
    pub id: &'static str,
    pub display_name: &'static str,
    pub icon_label: &'static str,
    pub accent_rgb: [u8; 3],
    pub default_command: &'static str,
    pub resume_mode: AgentResumeMode,
    pub session_validation: AgentSessionValidationMode,
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

    #[must_use]
    pub const fn requires_exact_session_validation(self) -> bool {
        matches!(self.session_validation, AgentSessionValidationMode::ParentThreadRoots)
    }
}

const CODEX: AgentDefinition = AgentDefinition {
    id: "codex",
    display_name: "Codex",
    icon_label: "CX",
    accent_rgb: [116, 162, 247],
    default_command: "codex",
    resume_mode: AgentResumeMode::ExactSubcommand { subcommand: "resume" },
    session_validation: AgentSessionValidationMode::ParentThreadRoots,
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
    session_validation: AgentSessionValidationMode::None,
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
    session_validation: AgentSessionValidationMode::None,
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
    session_validation: AgentSessionValidationMode::None,
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
    session_validation: AgentSessionValidationMode::None,
    integration: AgentIntegrationKind::None,
    kitty_keyboard: true,
};

const PI: AgentDefinition = AgentDefinition {
    id: "pi",
    display_name: "Pi",
    icon_label: "PI",
    accent_rgb: [250, 179, 135],
    default_command: "pi",
    resume_mode: AgentResumeMode::ExactFlag {
        flag: "--session",
        fresh_session_flag: None,
    },
    session_validation: AgentSessionValidationMode::None,
    integration: AgentIntegrationKind::None,
    kitty_keyboard: true,
};

/// xAI's Grok Build CLI (`grok`). Resumes by exact session id via
/// `--resume <session-id>`; it has no flag to pre-assign ids for fresh
/// sessions, and its ratatui TUI is driven without the kitty keyboard
/// protocol (same posture as Codex).
const GROK: AgentDefinition = AgentDefinition {
    id: "grok",
    display_name: "Grok",
    icon_label: "GB",
    accent_rgb: [249, 249, 249],
    default_command: "grok",
    resume_mode: AgentResumeMode::ExactFlag {
        flag: "--resume",
        fresh_session_flag: None,
    },
    session_validation: AgentSessionValidationMode::None,
    integration: AgentIntegrationKind::None,
    kitty_keyboard: false,
};

pub const BUILTIN_AGENT_KINDS: [PanelKind; 7] = [
    PanelKind::Codex,
    PanelKind::Claude,
    PanelKind::OpenCode,
    PanelKind::Gemini,
    PanelKind::KiloCode,
    PanelKind::Pi,
    PanelKind::Grok,
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
        PanelKind::Pi => Some(PI),
        PanelKind::Grok => Some(GROK),
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
    use super::{AgentResumeMode, PanelKind, agent_definition, is_agent_working_line};

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
            agent_definition(PanelKind::Pi)
                .expect("pi agent")
                .supports_session_binding()
        );
        assert!(
            agent_definition(PanelKind::Grok)
                .expect("grok agent")
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

    #[test]
    fn exact_parent_thread_validation_is_an_explicit_provider_capability() {
        assert!(
            agent_definition(PanelKind::Codex)
                .expect("codex agent")
                .requires_exact_session_validation()
        );
        for kind in [PanelKind::Claude, PanelKind::OpenCode, PanelKind::Pi, PanelKind::Grok] {
            assert!(
                !agent_definition(kind)
                    .expect("catalog-backed agent")
                    .requires_exact_session_validation()
            );
        }
    }

    #[test]
    fn working_line_requires_spinner_glyph_and_working_marker() {
        // Pi: braille spinner + default working message.
        assert!(is_agent_working_line("\u{280b} Working... (esc to interrupt)"));
        // Codex-style: braille spinner + elapsed time.
        assert!(is_agent_working_line("\u{2809} Working (5s) · esc to stop"));
        // Claude Code star spinner + unicode ellipsis.
        assert!(is_agent_working_line("\u{273B} Working…"));
        // Thinking / compaction / retry variants.
        assert!(is_agent_working_line("\u{2801} Thinking..."));
        assert!(is_agent_working_line("\u{280F} Compacting context..."));
        // Retrying / reusing variants.
        assert!(is_agent_working_line(
            "\u{2808} Retrying (1/5) in 3s... (esc to cancel)"
        ));
        assert!(is_agent_working_line("\u{280b} Reusing context..."));
        // Custom working message: no keyword, but the interrupt hint remains.
        assert!(is_agent_working_line("\u{2808} Deploying to prod (esc to interrupt)"));
        // Leading whitespace is fine.
        assert!(is_agent_working_line("  \u{280b} Working..."));

        // Plain output text mentioning "Working" has no spinner prefix.
        assert!(!is_agent_working_line("Working on the fix"));
        // Spinner glyph without a working marker (prompt line, done line).
        assert!(!is_agent_working_line("\u{280b} > ready"));
        assert!(!is_agent_working_line("\u{2713} Done"));
        assert!(!is_agent_working_line(""));
    }

    #[test]
    fn pi_definition_uses_exact_session_flag() {
        let definition = agent_definition(PanelKind::Pi).expect("pi agent");

        assert_eq!(definition.id, "pi");
        assert_eq!(definition.display_name, "Pi");
        assert_eq!(definition.icon_label, "PI");
        assert_eq!(definition.default_command, "pi");
        assert_eq!(
            definition.resume_mode,
            AgentResumeMode::ExactFlag {
                flag: "--session",
                fresh_session_flag: None,
            }
        );
        assert!(definition.kitty_keyboard);
    }

    #[test]
    fn grok_definition_uses_exact_resume_flag() {
        let definition = agent_definition(PanelKind::Grok).expect("grok agent");

        assert_eq!(definition.id, "grok");
        assert_eq!(definition.display_name, "Grok");
        assert_eq!(definition.icon_label, "GB");
        assert_eq!(definition.default_command, "grok");
        assert_eq!(
            definition.resume_mode,
            AgentResumeMode::ExactFlag {
                flag: "--resume",
                fresh_session_flag: None,
            }
        );
        assert!(!definition.kitty_keyboard);
    }
}
