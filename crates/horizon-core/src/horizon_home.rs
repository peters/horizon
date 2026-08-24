use std::path::{Path, PathBuf};

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

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

    #[must_use]
    pub fn browsers_manifest_dir(&self) -> PathBuf {
        self.root.join("runtime").join("browsers")
    }

    #[must_use]
    pub fn browser_profile_dir(&self, local_id: &str) -> PathBuf {
        self.root.join("browser-profiles").join(safe_local_id(local_id))
    }
}

#[must_use]
pub(crate) fn safe_local_id(local_id: &str) -> String {
    // Always encode the exact UTF-8 bytes. Keeping an apparently safe ID
    // verbatim would make identifiers that differ only by case collide on
    // default macOS and Windows filesystems. The lowercase hexadecimal
    // alphabet itself has no case variants, and the '%' prefix keeps the
    // empty identifier distinct.
    let mut encoded = String::with_capacity(1 + local_id.len() * 2);
    encoded.push('%');
    for byte in local_id.bytes() {
        encoded.push(char::from(LOWER_HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(LOWER_HEX[usize::from(byte & 0x0f)]));
    }
    encoded
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
        assert_eq!(
            home.browsers_manifest_dir(),
            PathBuf::from("/tmp/horizon-home/runtime/browsers")
        );
        assert_eq!(
            home.browser_profile_dir("panel-1"),
            PathBuf::from("/tmp/horizon-home/browser-profiles/%70616e656c2d31")
        );
        assert_eq!(
            home.browser_profile_dir("../outside"),
            PathBuf::from("/tmp/horizon-home/browser-profiles/%2e2e2f6f757473696465")
        );
        assert_eq!(
            home.browser_profile_dir(""),
            PathBuf::from("/tmp/horizon-home/browser-profiles/%")
        );
    }

    #[test]
    fn unsafe_local_ids_have_distinct_paths() {
        let home = HorizonHome::from_root("/tmp/horizon-home".into());

        assert_ne!(home.browser_profile_dir("a/b"), home.browser_profile_dir("a_b"));
        assert_ne!(home.browser_profile_dir(""), home.browser_profile_dir("_"));
        assert_ne!(home.browser_profile_dir("%"), home.browser_profile_dir("%25"));
        assert_ne!(home.browser_profile_dir("Panel-A"), home.browser_profile_dir("panel-a"));
    }

    #[test]
    fn every_local_id_uses_the_canonical_encoding() {
        let home = HorizonHome::from_root("/tmp/horizon-home".into());

        for local_id in ["panel-1", "CON", "nul", "Aux", "PRN", "COM1", "com9", "LPT1", "lpt9"] {
            assert!(
                home.browser_profile_dir(local_id)
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('%')),
                "{local_id} must use the canonical path encoding"
            );
        }
    }
}
