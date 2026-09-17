//! Persistent profile identity and host membership for shared browser panels.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use horizon_browser::session::{SharedBrowserSession, SharedSessionGroup};

use super::{BackendKind, BrowserConfig, BrowserPanelState, BrowserStatus};
use crate::panel::{PanelKind, PanelOptions};

pub(super) type ProfileSession = SharedSessionGroup;

type Registry = Mutex<HashMap<PathBuf, Weak<ProfileSession>>>;

pub(super) fn acquire(config: &BrowserConfig, profile_id: &str) -> Arc<ProfileSession> {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    let key = config.profile_dir(profile_id);
    let mut sessions = REGISTRY
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    sessions.retain(|_, session| session.strong_count() > 0);
    if let Some(session) = sessions.get(&key).and_then(Weak::upgrade) {
        return session;
    }
    let session = Arc::new(if config.backend == BackendKind::FirefoxBidi {
        SharedSessionGroup::new_firefox(profile_id.to_string())
    } else {
        SharedSessionGroup::Chromium(SharedBrowserSession::new(profile_id.to_string()))
    });
    sessions.insert(key, Arc::downgrade(&session));
    session
}

impl BrowserPanelState {
    pub(super) fn capture_directory(&self, home: &crate::horizon_home::HorizonHome) -> PathBuf {
        let profile_id = self.shared_profile_id().unwrap_or(&self.panel_local_id);
        let mut directory = super::profile_dir_for_home(&self.config, home, profile_id);
        if profile_id != self.panel_local_id {
            directory = directory
                .join("panels")
                .join(horizon_browser_control::paths::safe_local_id(&self.panel_local_id));
        }
        directory.join("captures")
    }

    #[must_use]
    pub fn shared_profile_id(&self) -> Option<&str> {
        self.shared_session.as_ref().map(|session| session.profile_id())
    }

    #[must_use]
    pub fn can_duplicate(&self) -> bool {
        !self.is_remote()
            && matches!(self.backend(), BackendKind::ChromiumCdp | BackendKind::FirefoxBidi)
            && matches!(self.status, BrowserStatus::Ready)
            && self.shared_session.is_some()
    }

    /// Construct a new page's launch options without copying profile files.
    ///
    /// # Errors
    /// The source must be a ready local Chromium or Firefox page.
    pub fn duplicate_options(&self) -> crate::error::Result<PanelOptions> {
        if !self.can_duplicate() {
            return Err(crate::error::Error::State(
                "duplicate panel requires a ready local Chromium or Firefox panel".into(),
            ));
        }
        Ok(PanelOptions {
            kind: PanelKind::Browser,
            command: self.url.clone().or_else(|| self.requested_url.clone()),
            browser_config: Some(self.config.clone()),
            browser_session_id: self.shared_profile_id().map(str::to_string),
            ..PanelOptions::default()
        })
    }
}

impl crate::board::Board {
    /// Open another page in the source panel's session and workspace.
    ///
    /// # Errors
    /// The source is absent, unsupported, or not ready, or creation fails.
    pub fn duplicate_browser_panel(
        &mut self,
        source: crate::panel::PanelId,
    ) -> crate::error::Result<crate::panel::PanelId> {
        let panel = self
            .panel(source)
            .ok_or_else(|| crate::error::Error::State("browser panel no longer exists".into()))?;
        let workspace = panel.workspace_id;
        let mut options = panel
            .browser()
            .ok_or_else(|| crate::error::Error::State("source is not a browser panel".into()))?
            .duplicate_options()?;
        options.size = Some(panel.layout.size);
        self.create_panel(options, workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn panel(root: &std::path::Path, id: &str, profile: &str) -> BrowserPanelState {
        let mut state = BrowserPanelState::inert();
        state.config.profile_root = Some(root.to_path_buf());
        state.panel_local_id = id.to_string();
        state.shared_session = Some(acquire(&state.config, profile));
        state.status = BrowserStatus::Ready;
        state.url = Some("https://example.test/current".into());
        state
    }

    #[test]
    fn closing_original_preserves_shared_profile_until_last_panel_closes() {
        let root = tempfile::tempdir().expect("profiles");
        let mut source = panel(root.path(), "source", "source");
        let mut duplicate = panel(root.path(), "duplicate", "source");
        let profile = source.config.profile_dir("source");
        std::fs::create_dir_all(&profile).expect("profile");
        std::fs::write(profile.join("fixture"), "session").expect("fixture");
        assert!(source.close_permanently().wait(Duration::from_secs(2)));
        assert!(profile.join("fixture").exists());
        assert_eq!(duplicate.shared_profile_id(), Some("source"));
        assert!(duplicate.close_permanently().wait(Duration::from_secs(2)));
        assert!(!profile.exists());
    }

    #[test]
    fn pending_cleanup_keeps_the_registered_group_alive() {
        let root = tempfile::tempdir().expect("profiles");
        let config = BrowserConfig {
            profile_root: Some(root.path().to_path_buf()),
            ..BrowserConfig::default()
        };
        let group = acquire(&config, "source");
        let weak = Arc::downgrade(&group);
        let signal = horizon_browser::session::BrowserShutdownSignal::completed().with_shared_profile_cleanup(group);
        let reacquired = acquire(&config, "source");
        assert!(Arc::ptr_eq(
            &weak.upgrade().expect("cleanup retains group"),
            &reacquired
        ));
        drop(reacquired);
        drop(signal);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn closing_duplicate_first_preserves_original_profile() {
        let root = tempfile::tempdir().expect("profiles");
        let mut source = panel(root.path(), "source", "source");
        let mut duplicate = panel(root.path(), "duplicate", "source");
        let profile = source.config.profile_dir("source");
        std::fs::create_dir_all(&profile).expect("profile");
        assert!(duplicate.close_permanently().wait(Duration::from_secs(2)));
        assert!(profile.exists());
        assert!(source.close_permanently().wait(Duration::from_secs(2)));
        assert!(!profile.exists());
    }

    #[test]
    fn duplicate_options_use_current_url_and_group_but_a_new_panel_identity() {
        let root = tempfile::tempdir().expect("profiles");
        let source = panel(root.path(), "duplicate", "original");
        let options = source.duplicate_options().expect("options");
        assert_eq!(options.command.as_deref(), Some("https://example.test/current"));
        assert_eq!(options.browser_session_id.as_deref(), Some("original"));
        assert!(options.local_id.is_none());
    }

    #[test]
    fn independent_panels_and_profile_roots_remain_isolated() {
        let root = tempfile::tempdir().expect("profiles");
        let left = panel(root.path(), "left", "left");
        let right = panel(root.path(), "right", "right");
        assert!(!Arc::ptr_eq(
            left.shared_session.as_ref().expect("left"),
            right.shared_session.as_ref().expect("right")
        ));
        let other = panel(&root.path().join("other"), "left", "left");
        assert!(!Arc::ptr_eq(
            left.shared_session.as_ref().expect("left"),
            other.shared_session.as_ref().expect("other")
        ));
    }

    #[test]
    fn unsupported_or_stopped_panels_cannot_duplicate() {
        let root = tempfile::tempdir().expect("profiles");
        let mut source = panel(root.path(), "source", "source");
        source.status = BrowserStatus::Stopped { code: None };
        assert!(source.duplicate_options().is_err());
        source.status = BrowserStatus::Ready;
        source.config.backend = BackendKind::SafariWebDriver;
        assert!(source.duplicate_options().is_err());
    }

    #[test]
    fn capture_exports_keep_panel_scoping_and_legacy_paths_with_shared_cleanup() {
        let root = tempfile::tempdir().expect("profiles");
        let home = crate::horizon_home::HorizonHome::from_root(root.path().join("home"));
        let mut original = panel(root.path(), "original", "original");
        let duplicate = panel(root.path(), "duplicate", "original");
        let profile = super::super::profile_dir_for_home(&original.config, &home, "original");
        let old_path = profile.join("captures");
        assert_eq!(original.capture_directory(&home), old_path);
        let shared_path = duplicate.capture_directory(&home);
        assert!(shared_path.starts_with(&profile));
        assert_ne!(shared_path, old_path);
        assert_eq!(shared_path.file_name().expect("capture name"), "captures");
        assert_eq!(
            shared_path.parent().expect("panel").file_name().expect("id"),
            horizon_browser_control::paths::safe_local_id("duplicate").as_str()
        );
        original.shared_session = None;
        original.config.backend = BackendKind::FirefoxBidi;
        assert_eq!(original.capture_directory(&home), old_path);
    }
}
