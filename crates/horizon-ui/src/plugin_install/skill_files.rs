//! The files that Horizon bundles with its agent integrations.
//!
//! One `EmbeddedFile` is one file of a skill or of the plugin, with the content
//! that the build embedded. The lists below are the files of one skill, and the
//! installer writes them under the paths that they name.

pub(super) struct EmbeddedFile {
    pub(super) relative_path: &'static str,
    pub(super) content: &'static str,
}

pub(super) const FRONT_DESK_SKILL_FILES: &[EmbeddedFile] = &[EmbeddedFile {
    relative_path: "SKILL.md",
    content: include_str!(concat!(
        env!("OUT_DIR"),
        "/assets/plugins/codex/skills/horizon-front-desk/SKILL.md"
    )),
}];

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
        relative_path: "skills/horizon-front-desk/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-front-desk/SKILL.md"
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
        relative_path: "skills/horizon-worktree/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-worktree/SKILL.md"
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
        relative_path: "skills/horizon-machines/references/handoff-machine.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/handoff-machine.md"
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
        relative_path: "skills/horizon-machines/references/repository-machine.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/repository-machine.md"
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
        relative_path: "skills/horizon-machines/references/tailscale.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/tailscale.md"
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
    EmbeddedFile {
        relative_path: "skills/horizon-server/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-server/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-setup/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-setup/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-setup/references/workshop.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-setup/references/workshop.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-plan/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-plan/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-plan/references/from-an-issue.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-plan/references/from-an-issue.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-server-move/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-server-move/SKILL.md"
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

pub(super) const SERVER_SKILL_FILES: &[EmbeddedFile] = &[EmbeddedFile {
    relative_path: "SKILL.md",
    content: include_str!(concat!(
        env!("OUT_DIR"),
        "/assets/plugins/codex/skills/horizon-server/SKILL.md"
    )),
}];

pub(super) const SETUP_SKILL_FILES: &[EmbeddedFile] = &[
    EmbeddedFile {
        relative_path: "SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-setup/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/workshop.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-setup/references/workshop.md"
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

/// The skill that starts each task in its own git worktree and shows the
/// worktrees of one repository on a card.
pub(super) const WORKTREE_SKILL_FILES: &[EmbeddedFile] = &[EmbeddedFile {
    relative_path: "SKILL.md",
    content: include_str!(concat!(
        env!("OUT_DIR"),
        "/assets/plugins/codex/skills/horizon-worktree/SKILL.md"
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
        relative_path: "references/handoff-machine.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/handoff-machine.md"
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
        relative_path: "references/repository-machine.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/repository-machine.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/runpod.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/runpod.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/tailscale.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-machines/references/tailscale.md"
        )),
    },
];

/// The skill that plans one task as three to twelve todos and keeps one live
/// card of them: the state, the owner, the place and the time of each todo.
pub(super) const PLAN_SKILL_FILES: &[EmbeddedFile] = &[
    EmbeddedFile {
        relative_path: "SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-plan/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "references/from-an-issue.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/codex/skills/horizon-plan/references/from-an-issue.md"
        )),
    },
];

/// The skill that moves the Horizon server to another computer with its store,
/// its machines, its keys and its values.
pub(super) const SERVER_MOVE_SKILL_FILES: &[EmbeddedFile] = &[EmbeddedFile {
    relative_path: "SKILL.md",
    content: include_str!(concat!(
        env!("OUT_DIR"),
        "/assets/plugins/codex/skills/horizon-server-move/SKILL.md"
    )),
}];

/// The state report that Horizon gives a pi panel at its launch: one JavaScript
/// module that pi loads with `-e` beside the extensions of the person.
pub(super) const PI_REPORT_EXTENSION: &str =
    include_str!(concat!(env!("OUT_DIR"), "/assets/plugins/pi/horizon-report.mjs"));
