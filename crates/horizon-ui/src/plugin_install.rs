use std::io::Write;
use std::path::{Path, PathBuf};

use horizon_core::HorizonHome;

struct EmbeddedFile {
    relative_path: &'static str,
    content: &'static str,
}

const CLAUDE_PLUGIN_FILES: &[EmbeddedFile] = &[
    EmbeddedFile {
        relative_path: ".claude-plugin/plugin.json",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/.claude-plugin/plugin.json"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-notify/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-notify/SKILL.md"
        )),
    },
];

const TEXT_SKILL_FILES: &[EmbeddedFile] = &[EmbeddedFile {
    relative_path: "SKILL.md",
    content: include_str!(concat!(
        env!("OUT_DIR"),
        "/assets/plugins/codex/skills/horizon-notify/SKILL.md"
    )),
}];

pub(crate) fn install_agent_plugins(horizon_home: &HorizonHome) {
    let user_home = std::env::var_os("HOME").map(PathBuf::from);

    match install_agent_plugins_impl(horizon_home, user_home.as_deref()) {
        Ok(updated_files) if updated_files > 0 => {
            tracing::info!(updated_files, "synced embedded Horizon agent plugins");
        }
        Ok(_) => {}
        Err(error) => tracing::warn!("failed to sync embedded Horizon agent plugins: {error}"),
    }
}

fn install_agent_plugins_impl(horizon_home: &HorizonHome, user_home: Option<&Path>) -> std::io::Result<usize> {
    let mut updated_files = 0usize;

    updated_files += sync_plugin_files(&horizon_home.claude_plugin_dir(), CLAUDE_PLUGIN_FILES)?;
    updated_files += sync_plugin_files(&horizon_home.codex_skill_dir(), TEXT_SKILL_FILES)?;

    if let Some(home) = user_home {
        let codex_home_skill_dir = home.join(".codex").join("skills").join("horizon-notify");
        updated_files += sync_plugin_files(&codex_home_skill_dir, TEXT_SKILL_FILES)?;
        let codex_export_dir = home.join(".agents").join("skills").join("horizon-notify");
        updated_files += sync_plugin_files(&codex_export_dir, TEXT_SKILL_FILES)?;
        let kilo_export_dir = home.join(".kilocode").join("skills").join("horizon-notify");
        updated_files += sync_plugin_files(&kilo_export_dir, TEXT_SKILL_FILES)?;
    }

    Ok(updated_files)
}

fn sync_plugin_files(base: &Path, files: &[EmbeddedFile]) -> std::io::Result<usize> {
    let mut updated_files = 0usize;

    for embedded_file in files {
        let path = base.join(embedded_file.relative_path);
        if sync_file_if_changed(&path, embedded_file.content)? {
            updated_files += 1;
        }
    }

    Ok(updated_files)
}

fn sync_file_if_changed(path: &Path, content: &str) -> std::io::Result<bool> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(content) {
        return Ok(false);
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let mut temp_file = tempfile::NamedTempFile::new_in(parent)?;
    temp_file.write_all(content.as_bytes())?;
    temp_file.flush()?;
    temp_file.persist(path).map_err(|error| error.error)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use horizon_core::HorizonHome;

    use super::{EmbeddedFile, TEXT_SKILL_FILES, install_agent_plugins_impl, sync_file_if_changed, sync_plugin_files};

    #[test]
    fn sync_file_if_changed_writes_missing_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("skill").join("SKILL.md");

        let updated = sync_file_if_changed(&path, "version-1").expect("write file");

        assert!(updated);
        assert_eq!(std::fs::read_to_string(path).expect("read file"), "version-1");
    }

    #[test]
    fn sync_file_if_changed_skips_identical_content() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("SKILL.md");
        std::fs::write(&path, "same").expect("seed file");

        let updated = sync_file_if_changed(&path, "same").expect("sync file");

        assert!(!updated);
    }

    #[test]
    fn sync_plugin_files_reports_only_changed_files() {
        let temp = tempfile::tempdir().expect("temp dir");
        let files = [
            EmbeddedFile {
                relative_path: "a.txt",
                content: "alpha",
            },
            EmbeddedFile {
                relative_path: "nested/b.txt",
                content: "beta",
            },
        ];

        let first = sync_plugin_files(temp.path(), &files).expect("first sync");
        let second = sync_plugin_files(temp.path(), &files).expect("second sync");

        assert_eq!(first, 2);
        assert_eq!(second, 0);
    }

    #[test]
    fn install_agent_plugins_syncs_codex_skill_into_current_codex_home() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let user_home = temp.path().join("user-home");

        let updated = install_agent_plugins_impl(&horizon_home, Some(&user_home)).expect("install plugins");

        assert!(updated > 0);
        assert_eq!(
            std::fs::read_to_string(user_home.join(".codex/skills/horizon-notify/SKILL.md"))
                .expect("codex skill should be exported"),
            TEXT_SKILL_FILES[0].content,
        );
        assert_eq!(
            std::fs::read_to_string(user_home.join(".agents/skills/horizon-notify/SKILL.md"))
                .expect("legacy codex skill export should still exist"),
            TEXT_SKILL_FILES[0].content,
        );
        assert_eq!(
            std::fs::read_to_string(user_home.join(".kilocode/skills/horizon-notify/SKILL.md"))
                .expect("kilo skill should be exported"),
            TEXT_SKILL_FILES[0].content,
        );
        assert_eq!(
            std::fs::read_to_string(horizon_home.codex_skill_dir().join("SKILL.md"))
                .expect("horizon codex integration should be synced"),
            TEXT_SKILL_FILES[0].content,
        );
    }
}
