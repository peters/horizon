use std::path::{Path, PathBuf};

use horizon_browser_control::paths::RuntimePathError;
pub(crate) use horizon_browser_control::paths::safe_local_id;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HorizonHome {
    root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum HorizonHomeError {
    #[error(transparent)]
    Override(#[from] RuntimePathError),
    #[error("could not create HORIZON_HOME {}: {source}", path.display())]
    Create { path: PathBuf, source: std::io::Error },
    #[error("HORIZON_HOME {} is not a directory", .0.display())]
    NotDirectory(PathBuf),
    #[error("HORIZON_HOME {} is a symbolic link; name the folder it points to", .0.display())]
    Symlink(PathBuf),
    #[error("HORIZON_HOME {} belongs to another user", .0.display())]
    NotOwned(PathBuf),
    #[error("HORIZON_HOME {} is open to other users; make it private with chmod 700", .0.display())]
    NotPrivate(PathBuf),
    #[error("HORIZON_HOME {} must be readable, writable and searchable by its owner; use chmod 700", .0.display())]
    NotUsable(PathBuf),
}

impl HorizonHome {
    /// The home from `HORIZON_HOME`, else `.horizon` in the user's home directory.
    #[must_use]
    pub fn resolve() -> Self {
        Self::from_root(horizon_browser_control::paths::default_horizon_root())
    }

    /// Resolve the home of a process that keeps its state there. A `HORIZON_HOME` override
    /// must be an absolute path. Horizon creates it with owner-only permissions when it is
    /// missing; an existing one must be a directory, not a symbolic link, whose permission
    /// bits admit only its owner. ACLs are not examined: on macOS an ACL can still grant
    /// other users access.
    ///
    /// # Errors
    ///
    /// Returns why the override cannot hold private state. The default home is not checked.
    pub fn prepare() -> Result<Self, HorizonHomeError> {
        Self::prepare_with(Self::override_root()?)
    }

    fn override_root() -> Result<Option<PathBuf>, HorizonHomeError> {
        Ok(horizon_browser_control::paths::home_override()?)
    }

    fn prepare_with(override_root: Option<PathBuf>) -> Result<Self, HorizonHomeError> {
        let Some(root) = override_root else {
            return Ok(Self::resolve());
        };
        prepare_private_root(&root)?;
        Ok(Self::from_root(root))
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
    pub fn agent_work_plugin_dir_for_host(&self, host_instance: &str) -> PathBuf {
        self.agent_plugin_host_dir(host_instance).join("work-resume")
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

    /// The SQLite runtime index of a session, next to its `runtime.yaml`.
    #[must_use]
    pub fn session_runtime_index_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("runtime.sqlite")
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
    pub fn agent_plugin_hosts_dir(&self) -> PathBuf {
        self.root.join("runtime").join("agent-plugins")
    }

    #[must_use]
    pub fn agent_plugin_host_dir(&self, host_instance: &str) -> PathBuf {
        self.agent_plugin_hosts_dir().join(safe_local_id(host_instance))
    }

    #[must_use]
    pub fn claude_plugin_dir_for_host(&self, host_instance: &str) -> PathBuf {
        self.agent_plugin_host_dir(host_instance).join("claude-code")
    }

    #[must_use]
    pub fn codex_integrations_dir(&self) -> PathBuf {
        self.root.join("integrations").join("codex")
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

fn prepare_private_root(root: &Path) -> Result<(), HorizonHomeError> {
    // Without trailing separators, so that `lstat` examines a symbolic link and not its target.
    let root = root.components().collect::<PathBuf>();
    let root = root.as_path();
    if let Err(source) = create_private_dirs(root) {
        if std::fs::metadata(root).is_ok_and(|metadata| !metadata.is_dir()) {
            return Err(HorizonHomeError::NotDirectory(root.to_path_buf()));
        }
        return Err(HorizonHomeError::Create {
            path: root.to_path_buf(),
            source,
        });
    }
    // A symbolic link could lead the instance to another state folder, such as `~/.horizon`.
    let metadata = std::fs::symlink_metadata(root).map_err(|source| HorizonHomeError::Create {
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(HorizonHomeError::Symlink(root.to_path_buf()));
    }
    if !metadata.is_dir() {
        return Err(HorizonHomeError::NotDirectory(root.to_path_buf()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(HorizonHomeError::NotOwned(root.to_path_buf()));
        }
        if metadata.mode() & 0o077 != 0 {
            return Err(HorizonHomeError::NotPrivate(root.to_path_buf()));
        }
        if metadata.mode() & 0o700 != 0o700 {
            return Err(HorizonHomeError::NotUsable(root.to_path_buf()));
        }
    }
    Ok(())
}

/// Creates the missing directories of `root` with mode 700, whatever the umask, and leaves
/// existing ancestors as they are.
fn create_private_dirs(root: &Path) -> std::io::Result<()> {
    let mut missing = Vec::new();
    let mut next = Some(root);
    while let Some(path) = next {
        if std::fs::symlink_metadata(path).is_ok() {
            break;
        }
        missing.push(path);
        next = path.parent();
    }
    // Never more open than 700, also while a concurrent start inspects the new directory.
    #[cfg(unix)]
    let builder = {
        let mut builder = std::fs::DirBuilder::new();
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
    };
    for path in missing.into_iter().rev() {
        match builder.create(path) {
            Ok(()) => {
                // A umask such as 0700 removes owner bits that the mode asked for.
                #[cfg(unix)]
                std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{HorizonHome, HorizonHomeError, browser_mcp_executable_for_process};

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
            home.session_runtime_index_path("session-1"),
            PathBuf::from("/tmp/horizon-home/sessions/session-1/runtime.sqlite")
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
            home.agent_plugin_host_dir("host-a"),
            PathBuf::from("/tmp/horizon-home/runtime/agent-plugins/%686f73742d61")
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

    #[test]
    fn without_an_override_the_prepared_home_is_the_default_home() {
        assert_eq!(
            HorizonHome::prepare_with(None).expect("default"),
            HorizonHome::resolve()
        );
    }

    #[test]
    fn a_missing_override_is_created_privately() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path().join("parent").join("horizon-home");

        let home = HorizonHome::prepare_with(Some(root.clone())).expect("created");

        assert_eq!(home.root(), root);
        assert!(root.is_dir());
        assert_eq!(home.config_path(), root.join("config.yaml"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |path: &std::path::Path| std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode(&root), 0o700);
            assert_eq!(
                mode(&temp.path().join("parent")),
                0o700,
                "created parents are private too"
            );
        }
        HorizonHome::prepare_with(Some(root)).expect("an existing private home is reused");
    }

    #[test]
    fn an_override_that_is_a_file_is_rejected() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let file = temp.path().join("file");
        std::fs::write(&file, b"not a directory").expect("file");

        assert!(matches!(
            HorizonHome::prepare_with(Some(file.clone())),
            Err(HorizonHomeError::NotDirectory(path)) if path == file
        ));
        assert!(matches!(
            HorizonHome::prepare_with(Some(file.join("child"))),
            Err(HorizonHomeError::Create { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn an_override_that_is_a_symbolic_link_is_rejected() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        HorizonHome::prepare_with(Some(target.clone())).expect("private target");
        std::os::unix::fs::symlink(&target, &link).expect("symbolic link");

        for root in [link.clone(), link.join(""), link.join(".")] {
            assert!(
                matches!(
                    HorizonHome::prepare_with(Some(root.clone())),
                    Err(HorizonHomeError::Symlink(path)) if path == link
                ),
                "{} is accepted",
                root.display()
            );
        }
        // A link among the ancestors is the system's business, as `/tmp` is on macOS.
        HorizonHome::prepare_with(Some(link.join("child"))).expect("a folder below a linked ancestor");
        assert!(target.join("child").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_override_open_to_other_users_is_rejected() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path().join("shared");
        std::fs::create_dir(&root).expect("directory");
        for mode in [0o755, 0o750, 0o701, 0o770] {
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(mode)).expect("permissions");
            assert!(
                matches!(
                    HorizonHome::prepare_with(Some(root.clone())),
                    Err(HorizonHomeError::NotPrivate(path)) if path == root
                ),
                "mode {mode:o} must be rejected"
            );
        }
        for mode in [0o600, 0o500, 0o300, 0o000] {
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(mode)).expect("permissions");
            assert!(
                matches!(
                    HorizonHome::prepare_with(Some(root.clone())),
                    Err(HorizonHomeError::NotUsable(path)) if path == root
                ),
                "mode {mode:o} cannot hold state"
            );
        }
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).expect("permissions");
        HorizonHome::prepare_with(Some(root)).expect("an owner-only directory is accepted");
    }

    /// The umask is process-wide, so the restrictive one is set in a child process.
    #[cfg(unix)]
    #[test]
    fn a_restrictive_umask_still_creates_a_usable_override() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().expect("temporary directory");
        let existing = temp.path().join("existing");
        std::fs::create_dir(&existing).expect("directory");
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).expect("permissions");
        let root = existing.join("new").join("horizon-home");
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "horizon_home::tests::umask_child", "--nocapture"])
            .env("HORIZON_HOME_UMASK_CHILD", &root)
            .output()
            .expect("child test");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let mode = |path: &std::path::Path| std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&existing.join("new")), 0o700);
        assert_eq!(mode(&existing), 0o755, "an existing ancestor keeps its mode");
    }

    #[cfg(unix)]
    #[test]
    fn umask_child() {
        let Some(root) = std::env::var_os("HORIZON_HOME_UMASK_CHILD") else {
            return;
        };
        rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o777));
        HorizonHome::prepare_with(Some(PathBuf::from(root))).expect("created despite the umask");
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
