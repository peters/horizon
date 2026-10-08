use std::{io, path::Path};

use horizon_core::HorizonHome;

use super::{AgentPluginHostLease, EmbeddedFile, provider_home, skill_dir_is_leased, sync_plugin_files};

pub(super) struct McpSkill {
    pub name: &'static str,
    pub files: &'static [EmbeddedFile],
}

pub(super) const MCP_SKILLS: &[McpSkill] = &[
    McpSkill {
        name: "horizon-cast",
        files: &[
            EmbeddedFile {
                relative_path: "SKILL.md",
                content: include_str!(concat!(
                    env!("OUT_DIR"),
                    "/assets/plugins/codex/skills/horizon-cast/SKILL.md"
                )),
            },
            EmbeddedFile {
                relative_path: "references/casting.md",
                content: include_str!(concat!(
                    env!("OUT_DIR"),
                    "/assets/plugins/codex/skills/horizon-cast/references/casting.md"
                )),
            },
        ],
    },
    McpSkill {
        name: "horizon-cloud",
        files: &[
            EmbeddedFile {
                relative_path: "SKILL.md",
                content: include_str!(concat!(
                    env!("OUT_DIR"),
                    "/assets/plugins/codex/skills/horizon-cloud/SKILL.md"
                )),
            },
            EmbeddedFile {
                relative_path: "references/cloud.md",
                content: include_str!(concat!(
                    env!("OUT_DIR"),
                    "/assets/plugins/codex/skills/horizon-cloud/references/cloud.md"
                )),
            },
        ],
    },
    McpSkill {
        name: "horizon-app-testing",
        files: &[
            EmbeddedFile {
                relative_path: "SKILL.md",
                content: include_str!(concat!(
                    env!("OUT_DIR"),
                    "/assets/plugins/codex/skills/horizon-app-testing/SKILL.md"
                )),
            },
            EmbeddedFile {
                relative_path: "references/native-apps.md",
                content: include_str!(concat!(
                    env!("OUT_DIR"),
                    "/assets/plugins/codex/skills/horizon-app-testing/references/native-apps.md"
                )),
            },
        ],
    },
];

pub(super) fn is_mcp_skill(dir: &Path) -> bool {
    MCP_SKILLS
        .iter()
        .any(|skill| dir.file_name().is_some_and(|name| name == skill.name))
}

pub(super) fn validate_mcp_skill(dir: &Path) -> io::Result<()> {
    let skill = MCP_SKILLS
        .iter()
        .find(|skill| dir.file_name().is_some_and(|name| name == skill.name));
    let Some(skill) = skill else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Unknown Horizon MCP skill"));
    };
    super::owned_skills::validate(dir, skill.files)
}

pub(super) fn validate_skill_files(dir: &Path, files: &[EmbeddedFile]) -> io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(unowned()),
    }
    let mut found = 0;
    validate_entries(dir, dir, files, &mut found)?;
    if found == files.len() { Ok(()) } else { Err(unowned()) }
}

fn validate_entries(root: &Path, dir: &Path, files: &[EmbeddedFile], found: &mut usize) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(io::Error::other)?;
        let kind = entry.file_type()?;
        if kind.is_dir()
            && files
                .iter()
                .any(|file| Path::new(file.relative_path).starts_with(relative))
        {
            validate_entries(root, &path, files, found)?;
        } else if kind.is_file() {
            let Some(file) = files.iter().find(|file| relative == Path::new(file.relative_path)) else {
                return Err(unowned());
            };
            if std::fs::read_to_string(&path)? != file.content {
                return Err(unowned());
            }
            *found += 1;
        } else {
            return Err(unowned());
        }
    }
    Ok(())
}

fn unowned() -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Horizon skill differs from bundled files; preserving existing content",
    )
}

pub(super) fn sync_mcp_skills(
    horizon_home: &HorizonHome,
    claude_plugin_dir: &Path,
    user_home: Option<&Path>,
    grok_home: Option<&Path>,
    codex_home: Option<&Path>,
    lease: Option<&AgentPluginHostLease>,
) -> io::Result<usize> {
    let mut updated = 0;
    let grok_root = provider_home(grok_home, user_home, ".grok").filter(|root| super::grok_mcp::register(root).is_ok());
    let codex_root = provider_home(codex_home, user_home, ".codex");
    for skill in MCP_SKILLS {
        updated += sync_plugin_files(&claude_plugin_dir.join("skills").join(skill.name), skill.files)?;
        updated += sync_plugin_files(&horizon_home.codex_integrations_dir().join(skill.name), skill.files)?;
        for root in [codex_root.as_ref(), grok_root.as_ref()].into_iter().flatten() {
            let dir = root.join("skills").join(skill.name);
            if !skill_dir_is_leased(lease, &dir) {
                continue;
            }
            if let Err(error) = super::owned_skills::validate(&dir, skill.files) {
                tracing::warn!(path = %dir.display(), %error, "MCP skill unavailable; preserving existing content");
                continue;
            }
            updated += super::owned_skills::sync(&dir, skill.files)?;
        }
    }
    Ok(updated)
}

#[cfg(test)]
mod tests;
