use serde::{Deserialize, Serialize};

use crate::{
    panel::{PanelKind, PanelOptions, PanelResume},
    ssh::SshConnection,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PresetConfig {
    pub name: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub kind: PanelKind,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub resume: PanelResume,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_connection: Option<SshConnection>,
}

impl PresetConfig {
    /// Convert this preset into `PanelOptions` for panel creation. Browser
    /// presets carry the active browser config so a configured `command`,
    /// `extra_args`, `quality`, `every_nth_frame`, or `profile_root` is
    /// honored instead of silently falling back to defaults.
    #[must_use]
    pub fn to_panel_options(&self, browser_config: &crate::browser::BrowserConfig) -> PanelOptions {
        PanelOptions {
            name: Some(self.name.clone()),
            command: self.command.clone(),
            args: self.args.clone(),
            ssh_connection: self.ssh_connection.clone(),
            kind: self.kind,
            resume: self.resume.clone(),
            browser_config: (self.kind == PanelKind::Browser).then(|| browser_config.clone()),
            ..PanelOptions::default()
        }
    }

    #[must_use]
    pub fn requires_workspace_cwd(&self) -> bool {
        // Browser panels never receive a cwd (Chrome is launched from the
        // Horizon process), so a cwd-less workspace must not trigger a
        // directory picker for them.
        !matches!(self.kind, PanelKind::Ssh | PanelKind::Browser)
    }
}

pub(crate) fn default_opencode_presets() -> [PresetConfig; 1] {
    [default_opencode_preset()]
}

pub(crate) fn default_opencode_preset() -> PresetConfig {
    PresetConfig {
        name: "OpenCode".to_string(),
        alias: Some("oc".to_string()),
        kind: PanelKind::OpenCode,
        command: None,
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    }
}

pub(crate) fn default_gemini_presets() -> [PresetConfig; 1] {
    [PresetConfig {
        name: "Gemini CLI".to_string(),
        alias: Some("gm".to_string()),
        kind: PanelKind::Gemini,
        command: None,
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    }]
}

pub(crate) fn default_kilo_presets() -> [PresetConfig; 1] {
    [default_kilo_preset()]
}

pub(crate) fn default_kilo_preset() -> PresetConfig {
    PresetConfig {
        name: "KiloCode".to_string(),
        alias: Some("kc".to_string()),
        kind: PanelKind::KiloCode,
        command: None,
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    }
}

pub(crate) fn default_pi_preset() -> PresetConfig {
    PresetConfig {
        name: "Pi".to_string(),
        alias: Some("pi".to_string()),
        kind: PanelKind::Pi,
        command: None,
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    }
}

/// Single Grok Build preset. The CLI starts a fresh session on every launch
/// unless `--resume` is passed, so menu launches always start fresh like the
/// other coding-agent defaults.
pub(crate) fn default_grok_preset() -> PresetConfig {
    PresetConfig {
        name: "Grok".to_string(),
        alias: Some("gb".to_string()),
        kind: PanelKind::Grok,
        command: None,
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    }
}

pub(crate) fn default_grok_presets() -> [PresetConfig; 1] {
    [default_grok_preset()]
}

fn insert_missing_agent_presets(presets: &mut Vec<PresetConfig>, defaults: impl IntoIterator<Item = PresetConfig>) {
    for default_preset in defaults {
        let expected_name = default_preset.name.to_ascii_lowercase();
        let expected_alias = default_preset.alias.as_deref().map(str::to_ascii_lowercase);
        let exists = presets.iter().any(|preset| {
            preset.name.eq_ignore_ascii_case(&default_preset.name)
                || preset
                    .alias
                    .as_deref()
                    .zip(expected_alias.as_deref())
                    .is_some_and(|(alias, expected)| alias.eq_ignore_ascii_case(expected))
                || (preset.kind == default_preset.kind && preset.resume == default_preset.resume)
                || preset.name.to_ascii_lowercase() == expected_name
        });

        if !exists {
            presets.push(default_preset);
        }
    }
}

pub(crate) fn insert_missing_opencode_presets(presets: &mut Vec<PresetConfig>) {
    insert_missing_agent_presets(presets, default_opencode_presets());
}

pub(crate) fn insert_missing_gemini_presets(presets: &mut Vec<PresetConfig>) {
    insert_missing_agent_presets(presets, default_gemini_presets());
}

pub(crate) fn insert_missing_kilo_presets(presets: &mut Vec<PresetConfig>) {
    insert_missing_agent_presets(presets, default_kilo_presets());
}

pub(crate) fn insert_missing_pi_presets(presets: &mut Vec<PresetConfig>) {
    let default_preset = default_pi_preset();
    let exists = presets.iter().any(|preset| {
        preset.name.eq_ignore_ascii_case(&default_preset.name)
            || preset
                .alias
                .as_deref()
                .is_some_and(|alias| alias.eq_ignore_ascii_case("pi"))
            || preset.kind == PanelKind::Pi
    });

    if !exists {
        presets.push(default_preset);
    }
}

pub(crate) fn insert_missing_grok_presets(presets: &mut Vec<PresetConfig>) {
    let default_preset = default_grok_preset();
    let exists = presets.iter().any(|preset| {
        preset.name.eq_ignore_ascii_case(&default_preset.name)
            || preset
                .alias
                .as_deref()
                .is_some_and(|alias| alias.eq_ignore_ascii_case("gb"))
            || preset.kind == PanelKind::Grok
    });

    if !exists {
        presets.push(default_preset);
    }
}

pub(crate) fn default_browser_preset() -> PresetConfig {
    PresetConfig {
        name: "Browser".to_string(),
        alias: Some("web".to_string()),
        kind: PanelKind::Browser,
        command: None,
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    }
}

pub(crate) fn insert_missing_browser_preset(presets: &mut Vec<PresetConfig>) {
    let default_preset = default_browser_preset();
    let exists = presets.iter().any(|preset| {
        preset.name.eq_ignore_ascii_case(&default_preset.name)
            || preset
                .alias
                .as_deref()
                .is_some_and(|alias| alias.eq_ignore_ascii_case("web"))
            || preset.kind == PanelKind::Browser
    });

    if !exists {
        presets.push(default_preset);
    }
}

/// Single Codex preset. Codex 0.128's default invocation is auto mode
/// (`--sandbox workspace-write --ask-for-approval on-request`), so
/// `--no-alt-screen` is the only flag we need to set. Menu launches always
/// start a fresh session — same as every other coding-agent default.
pub(crate) fn default_codex_preset() -> PresetConfig {
    PresetConfig {
        name: "Codex".to_string(),
        alias: Some("cx".to_string()),
        kind: PanelKind::Codex,
        command: None,
        args: vec!["--no-alt-screen".to_string()],
        resume: PanelResume::Fresh,
        ssh_connection: None,
    }
}

/// Single Claude Code preset. `--permission-mode auto` (Claude Code v2.1.83+)
/// routes actions through a separate classifier model; safer than the old
/// `--dangerously-skip-permissions` and the right default for hands-off use.
/// Menu launches always start a fresh session.
pub(crate) fn default_claude_preset() -> PresetConfig {
    PresetConfig {
        name: "Claude Code".to_string(),
        alias: Some("cc".to_string()),
        kind: PanelKind::Claude,
        command: None,
        args: vec!["--permission-mode".to_string(), "auto".to_string()],
        resume: PanelResume::Fresh,
        ssh_connection: None,
    }
}

pub(super) fn default_presets() -> Vec<PresetConfig> {
    let mut presets = vec![
        PresetConfig {
            name: "Shell".to_string(),
            alias: Some("sh".to_string()),
            kind: PanelKind::Shell,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
        default_codex_preset(),
        default_claude_preset(),
    ];
    presets.extend(default_opencode_presets());
    presets.extend(default_gemini_presets());
    presets.extend(default_kilo_presets());
    insert_missing_pi_presets(&mut presets);
    presets.extend(default_grok_presets());
    presets.extend([
        PresetConfig {
            name: "Git Changes".to_string(),
            alias: Some("gc".to_string()),
            kind: PanelKind::GitChanges,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
        PresetConfig {
            name: "Markdown".to_string(),
            alias: Some("md".to_string()),
            kind: PanelKind::Editor,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
        PresetConfig {
            name: "Usage".to_string(),
            alias: Some("u".to_string()),
            kind: PanelKind::Usage,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
    ]);
    insert_missing_browser_preset(&mut presets);
    presets
}
