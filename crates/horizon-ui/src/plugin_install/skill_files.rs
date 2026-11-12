//! The files that Horizon bundles with its agent integrations.
//!
//! One `EmbeddedFile` is one file of a skill or of the plugin, with the content
//! that the build embedded. The lists below are the files of one skill, and the
//! installer writes them under the paths that they name.

pub(super) struct EmbeddedFile {
    pub(super) relative_path: &'static str,
    pub(super) content: &'static str,
}

pub(super) const CLAUDE_PLUGIN_FILES: &[EmbeddedFile] = &[
    EmbeddedFile {
        relative_path: ".claude-plugin/plugin.json",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/.claude-plugin/plugin.json"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-browser/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-browser/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-device/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-device/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-block/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-block/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-machines/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-machines/references/aws.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/aws.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-machines/references/browser-machine.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/browser-machine.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-machines/references/browser-machine/Dockerfile",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/browser-machine/Dockerfile"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-machines/references/browser-machine/entrypoint.sh",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/browser-machine/entrypoint.sh"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-machines/references/browser-machine/horizon-browser-wrapper.sh",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/browser-machine/horizon-browser-wrapper.sh"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-machines/references/browser-machine/setup.sh",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/browser-machine/setup.sh"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-machines/references/azure.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/azure.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-machines/references/gcp.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/gcp.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-machines/references/hetzner.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/hetzner.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-machines/references/runpod.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/runpod.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-mobile/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-mobile/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-mobile/references/android.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-mobile/references/android.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-mobile/references/ios.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-mobile/references/ios.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-mobile/references/provider.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-mobile/references/provider.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-mobile/scripts/mobile_session.py",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-mobile/scripts/mobile_session.py"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-speech/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-speech/SKILL.md"
        )),
    },
];

/// The skill that owns a tool of its own: the body, the three references and the
/// script that drives one phone.
pub(super) const MOBILE_SKILL_FILES: &[EmbeddedFile] = &[
    EmbeddedFile {
        relative_path: "SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-mobile/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/android.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-mobile/references/android.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/ios.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-mobile/references/ios.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/provider.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-mobile/references/provider.md"
        )),
    },
    EmbeddedFile {
        relative_path: "scripts/mobile_session.py",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-mobile/scripts/mobile_session.py"
        )),
    },
];

pub(super) const SPEECH_SKILL_FILES: &[EmbeddedFile] = &[EmbeddedFile {
    relative_path: "SKILL.md",
    content: include_str!(concat!(
        env!("OUT_DIR"),
        "/assets/plugins/codex/skills/horizon-speech/SKILL.md"
    )),
}];

pub(super) const BROWSER_SKILL_FILES: &[EmbeddedFile] = &[EmbeddedFile {
    relative_path: "SKILL.md",
    content: include_str!(concat!(
        env!("OUT_DIR"),
        "/assets/plugins/codex/skills/horizon-browser/SKILL.md"
    )),
}];

pub(super) const DEVICE_SKILL_FILES: &[EmbeddedFile] = &[EmbeddedFile {
    relative_path: "SKILL.md",
    content: include_str!(concat!(
        env!("OUT_DIR"),
        "/assets/plugins/codex/skills/horizon-device/SKILL.md"
    )),
}];

pub(super) const BLOCK_SKILL_FILES: &[EmbeddedFile] = &[EmbeddedFile {
    relative_path: "SKILL.md",
    content: include_str!(concat!(
        env!("OUT_DIR"),
        "/assets/plugins/codex/skills/horizon-block/SKILL.md"
    )),
}];

/// The first skill with reference files: the body of the skill and the five
/// provider references that it links, under their own `references/` directory.
pub(super) const MACHINES_SKILL_FILES: &[EmbeddedFile] = &[
    EmbeddedFile {
        relative_path: "SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/aws.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/aws.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/browser-machine.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/browser-machine.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/browser-machine/Dockerfile",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/browser-machine/Dockerfile"
        )),
    },
    EmbeddedFile {
        relative_path: "references/browser-machine/entrypoint.sh",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/browser-machine/entrypoint.sh"
        )),
    },
    EmbeddedFile {
        relative_path: "references/browser-machine/horizon-browser-wrapper.sh",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/browser-machine/horizon-browser-wrapper.sh"
        )),
    },
    EmbeddedFile {
        relative_path: "references/browser-machine/setup.sh",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/browser-machine/setup.sh"
        )),
    },
    EmbeddedFile {
        relative_path: "references/azure.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/azure.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/gcp.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/gcp.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/hetzner.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/hetzner.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/runpod.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/runpod.md"
        )),
    },
];
