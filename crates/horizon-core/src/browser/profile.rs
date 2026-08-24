//! Browser-profile path resolution and launch-time persistence invariants.

#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use super::BrowserConfig;

impl BrowserConfig {
    pub(super) fn resolved_for_launch(&self) -> std::io::Result<Self> {
        let Some(configured_root) = &self.profile_root else {
            return Ok(self.clone());
        };
        let expanded_root = crate::config::Config::expand_tilde(&configured_root.to_string_lossy());
        let absolute_root = if expanded_root.is_absolute() {
            expanded_root
        } else {
            std::env::current_dir()?.join(expanded_root)
        };
        let mut resolved = self.clone();
        resolved.profile_root = Some(absolute_root);
        Ok(resolved)
    }

    #[cfg(test)]
    fn resolved_for_launch_at(&self, launch_dir: &Path) -> Self {
        let mut resolved = self.clone();
        resolved.profile_root = self.profile_root.as_ref().map(|configured_root| {
            let expanded_root = crate::config::Config::expand_tilde(&configured_root.to_string_lossy());
            if expanded_root.is_absolute() {
                expanded_root
            } else {
                launch_dir.join(expanded_root)
            }
        });
        resolved
    }
}

pub(crate) fn profile_dir_for_home(
    config: &BrowserConfig,
    home: &crate::horizon_home::HorizonHome,
    panel_local_id: &str,
) -> PathBuf {
    // The configured value is a root: every panel still gets its own safely
    // encoded directory, or concurrent panels would share Chrome's lock.
    match &config.profile_root {
        Some(root) => crate::config::Config::expand_tilde(&root.to_string_lossy())
            .join(crate::horizon_home::safe_local_id(panel_local_id)),
        None => home.browser_profile_dir(panel_local_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::BrowserPanelState;

    #[test]
    fn relative_profile_roots_are_anchored_once_to_the_launch_directory() {
        #[cfg(windows)]
        let launch_dir = Path::new(r"C:\horizon-launch");
        #[cfg(not(windows))]
        let launch_dir = Path::new("/horizon-launch");
        let config = BrowserConfig {
            profile_root: Some(PathBuf::from("profiles/browser")),
            ..BrowserConfig::default()
        };

        let resolved = config.resolved_for_launch_at(launch_dir);

        assert_eq!(resolved.profile_root, Some(launch_dir.join("profiles/browser")));
        assert!(resolved.profile_root.is_some_and(|root| root.is_absolute()));
    }

    #[test]
    fn panel_persists_the_absolute_root_resolved_before_driver_launch() {
        let relative_root = PathBuf::from("relative-browser-profiles");
        let expected_root = std::env::current_dir()
            .expect("current test directory")
            .join(&relative_root);
        let config = BrowserConfig {
            command: Some("/definitely/missing/horizon-test-chrome".to_string()),
            profile_root: Some(relative_root),
            ..BrowserConfig::default()
        };

        let mut panel = BrowserPanelState::start("relative-profile-panel", &config, None).expect("panel state");

        assert_eq!(panel.profile_root_for_persistence(), Some(expected_root.as_path()));
        panel.request_shutdown();
        if let Some(signal) = panel.take_shutdown_signal() {
            assert!(signal.wait(std::time::Duration::from_secs(2)));
        }
    }
}
