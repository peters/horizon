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
    pub fn claude_plugin_dir_for_host(&self, host_instance: &str) -> PathBuf {
        self.root
            .join("runtime")
            .join("agent-plugins")
            .join(safe_local_id(host_instance))
            .join("claude-code")
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
    pub fn codex_browser_skill_dir(&self) -> PathBuf {
        self.codex_integrations_dir().join("horizon-browser")
    }

    #[must_use]
    pub fn browsers_manifest_dir(&self) -> PathBuf {
        self.root.join("runtime").join("browsers")
    }

    #[must_use]
    pub fn browser_results_dir(&self) -> PathBuf {
        self.root.join("runtime").join("browser-results")
    }

    #[must_use]
    pub fn browser_audit_dir(&self) -> PathBuf {
        self.root.join("audit").join("browsers")
    }

    /// Private agent-requested network exports removed with the panel profile.
    #[must_use]
    pub fn browser_capture_dir(&self, local_id: &str) -> PathBuf {
        self.browser_profile_dir(local_id).join("captures")
    }

    #[must_use]
    pub fn browser_profile_dir(&self, local_id: &str) -> PathBuf {
        self.root.join("browser-profiles").join(safe_local_id(local_id))
    }
}

#[must_use]
pub fn browser_mcp_executable() -> Option<PathBuf> {
    browser_mcp_executable_for_process(std::process::id(), std::env::current_exe().ok())
}

fn browser_mcp_executable_for_process(process_id: u32, current_executable: Option<PathBuf>) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let process_executable = PathBuf::from(format!("/proc/{process_id}/exe"));
        if process_executable.is_file() {
            return Some(process_executable);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = process_id;

    current_executable
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

    use super::{HorizonHome, browser_mcp_executable_for_process};

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
            home.claude_plugin_dir_for_host("host-a"),
            PathBuf::from("/tmp/horizon-home/runtime/agent-plugins/%686f73742d61/claude-code")
        );
        assert_eq!(
            home.codex_browser_skill_dir(),
            PathBuf::from("/tmp/horizon-home/integrations/codex/horizon-browser")
        );
        assert_eq!(
            home.browsers_manifest_dir(),
            PathBuf::from("/tmp/horizon-home/runtime/browsers")
        );
        assert_eq!(
            home.browser_results_dir(),
            PathBuf::from("/tmp/horizon-home/runtime/browser-results")
        );
        assert_eq!(
            home.browser_audit_dir(),
            PathBuf::from("/tmp/horizon-home/audit/browsers")
        );
        assert_eq!(
            home.browser_capture_dir("panel-1"),
            PathBuf::from("/tmp/horizon-home/browser-profiles/%70616e656c2d31/captures")
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

    #[cfg(target_os = "linux")]
    #[test]
    fn browser_mcp_executable_uses_the_live_process_after_the_original_path_is_deleted() {
        let deleted_path = PathBuf::from("/tmp/removed-worktree/target/release/horizon (deleted)");

        let executable = browser_mcp_executable_for_process(std::process::id(), Some(deleted_path));

        assert_eq!(
            executable,
            Some(PathBuf::from(format!("/proc/{}/exe", std::process::id())))
        );
    }

    #[test]
    fn browser_mcp_executable_falls_back_when_process_lookup_is_unavailable() {
        let current_executable = PathBuf::from("/opt/horizon");

        let executable = browser_mcp_executable_for_process(u32::MAX, Some(current_executable.clone()));

        assert_eq!(executable, Some(current_executable));
    }
}
