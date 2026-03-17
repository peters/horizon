use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HorizonHome {
    root: PathBuf,
}

impl HorizonHome {
    #[must_use]
    pub fn resolve() -> Self {
        let root = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map_or_else(|| PathBuf::from(".horizon"), |home| home.join(".horizon"));
        Self { root }
    }

    #[must_use]
    pub fn from_root(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.yaml")
    }

    #[must_use]
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    #[must_use]
    pub fn session_index_path(&self) -> PathBuf {
        self.sessions_dir().join("index.yaml")
    }

    #[must_use]
    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(session_id)
    }

    #[must_use]
    pub fn session_meta_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("meta.yaml")
    }

    #[must_use]
    pub fn session_runtime_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("runtime.yaml")
    }

    #[must_use]
    pub fn session_lease_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("lease.json")
    }

    #[must_use]
    pub fn session_transcripts_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("transcripts")
    }

    #[must_use]
    pub fn plugins_dir(&self) -> PathBuf {
        self.root.join("plugins")
    }

    #[must_use]
    pub fn claude_plugin_dir(&self) -> PathBuf {
        self.plugins_dir().join("claude-code")
    }

    #[must_use]
    pub fn codex_integrations_dir(&self) -> PathBuf {
        self.root.join("integrations").join("codex")
    }

    #[must_use]
    pub fn codex_skill_dir(&self) -> PathBuf {
        self.codex_integrations_dir().join("horizon-notify")
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::HorizonHome;

    #[test]
    fn session_paths_live_under_horizon_home() {
        let home = HorizonHome::from_root("/tmp/horizon-home".into());

        assert_eq!(home.config_path(), PathBuf::from("/tmp/horizon-home/config.yaml"));
        assert_eq!(
            home.session_index_path(),
            PathBuf::from("/tmp/horizon-home/sessions/index.yaml")
        );
        assert_eq!(
            home.session_runtime_path("session-1"),
            PathBuf::from("/tmp/horizon-home/sessions/session-1/runtime.yaml")
        );
        assert_eq!(
            home.session_transcripts_dir("session-1"),
            PathBuf::from("/tmp/horizon-home/sessions/session-1/transcripts")
        );
        assert_eq!(
            home.claude_plugin_dir(),
            PathBuf::from("/tmp/horizon-home/plugins/claude-code")
        );
    }
}
