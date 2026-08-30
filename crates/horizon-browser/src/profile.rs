use std::path::{Path, PathBuf};

use crate::{BackendKind, BrowserConfig, paths};

pub(crate) fn resolved_for_launch(config: &BrowserConfig) -> std::io::Result<BrowserConfig> {
    let mut resolved = config.clone();
    resolved.quality = resolved.quality.clamp(1, 100);
    let Some(configured_root) = &config.profile_root else {
        return Ok(resolved);
    };
    let expanded_root = paths::expand_tilde(&configured_root.to_string_lossy());
    resolved.profile_root = Some(if expanded_root.is_absolute() {
        expanded_root
    } else {
        std::env::current_dir()?.join(expanded_root)
    });
    Ok(resolved)
}

#[must_use]
pub(crate) fn profile_dir(config: &BrowserConfig, panel_local_id: &str) -> PathBuf {
    let default_root = paths::default_root().join("profiles");
    profile_dir_with_default_root(config, panel_local_id, &default_root)
}

#[must_use]
pub(crate) fn profile_dir_with_default_root(
    config: &BrowserConfig,
    panel_local_id: &str,
    default_root: &Path,
) -> PathBuf {
    let panel_root = panel_profile_dir_with_default_root(config, panel_local_id, default_root);
    panel_root.join(match config.backend {
        BackendKind::ChromiumCdp => "chromium",
        BackendKind::FirefoxBidi => "firefox",
        BackendKind::SafariWebDriver => "safari",
    })
}

#[must_use]
pub(crate) fn panel_profile_dir_with_default_root(
    config: &BrowserConfig,
    panel_local_id: &str,
    default_root: &Path,
) -> PathBuf {
    effective_profile_root(config, default_root).join(paths::safe_local_id(panel_local_id))
}

#[must_use]
pub(crate) fn effective_profile_root(config: &BrowserConfig, default_root: &Path) -> PathBuf {
    if let Some(root) = &config.profile_root {
        return paths::expand_tilde(&root.to_string_lossy());
    }
    snap_browser_profile_root(config, default_root).unwrap_or_else(|| default_root.to_path_buf())
}

#[cfg(target_os = "linux")]
fn snap_browser_profile_root(config: &BrowserConfig, default_root: &Path) -> Option<PathBuf> {
    let home = paths::home_dir();
    if !default_root_is_hidden_beneath_home(default_root, &home) {
        return None;
    }
    if let Ok(command) = crate::process::resolve_binary_or_default(&config.command)
        && let Some(root) = snap_browser_profile_root_for_command(&command, "chromium", &home)
    {
        return Some(root);
    }
    snap_firefox_profile_root_with_resolver(config.firefox_command.as_deref(), &home, |command| {
        crate::process::resolve_binary(command).ok()
    })
}

#[cfg(not(target_os = "linux"))]
const fn snap_browser_profile_root(_config: &BrowserConfig, _default_root: &Path) -> Option<PathBuf> {
    None
}

#[cfg(any(target_os = "linux", test))]
fn default_root_is_hidden_beneath_home(default_root: &Path, home: &Path) -> bool {
    default_root
        .strip_prefix(home)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| component.as_os_str().to_string_lossy().starts_with('.'))
}

#[cfg(any(target_os = "linux", test))]
fn snap_browser_profile_root_for_command(command: &Path, browser_name: &str, home: &Path) -> Option<PathBuf> {
    let parent = command.parent()?;
    let is_snap_launcher = parent == Path::new("/snap/bin") || parent.ends_with(Path::new("snap/bin"));
    if !is_snap_launcher || command.file_name()? != browser_name {
        return None;
    }
    // Keep one panel root across backend switches. A package-owned Snap
    // directory would work for that package but can be inaccessible to the
    // other backend; this non-hidden directory is writable through Snap's
    // `home` interface and by regular browser packages.
    Some(home.join("Horizon").join("browser-profiles"))
}

#[cfg(any(target_os = "linux", test))]
fn snap_firefox_profile_root_with_resolver(
    configured_command: Option<&str>,
    home: &Path,
    mut resolve: impl FnMut(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(command) = configured_command.filter(|command| !command.trim().is_empty()) {
        return resolve(command)
            .as_deref()
            .and_then(|resolved| snap_browser_profile_root_for_command(resolved, "firefox", home));
    }

    // Ubuntu's default `firefox` on PATH can be `/usr/bin/firefox`, a
    // wrapper for the real Snap launcher. Check that launcher directly for
    // automatic discovery, but never override an explicitly configured
    // native Firefox command.
    let snap_launcher = resolve("/snap/bin/firefox");
    if let Some(root) = snap_launcher
        .as_deref()
        .and_then(|resolved| snap_browser_profile_root_for_command(resolved, "firefox", home))
    {
        return Some(root);
    }

    resolve("firefox")
        .as_deref()
        .and_then(|resolved| snap_browser_profile_root_for_command(resolved, "firefox", home))
}

pub(crate) fn schedule_removal(profile_dir: PathBuf) -> std::sync::mpsc::Receiver<std::io::Result<()>> {
    let (completion_tx, completion_rx) = std::sync::mpsc::channel();
    let worker_tx = completion_tx.clone();
    let spawn = std::thread::Builder::new()
        .name("browser-profile-cleanup".into())
        .spawn(move || {
            let result = match std::fs::remove_dir_all(&profile_dir) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            };
            let _ = worker_tx.send(result);
        });
    if let Err(error) = spawn {
        tracing::warn!("failed to start browser profile cleanup: {error}");
        let _ = completion_tx.send(Err(error));
    }
    completion_rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_browser_profiles_live_in_a_cross_backend_writable_directory() {
        let home = Path::new("/home/alice");

        assert_eq!(
            snap_browser_profile_root_for_command(Path::new("/snap/bin/chromium"), "chromium", home),
            Some(PathBuf::from("/home/alice/Horizon/browser-profiles"))
        );
        assert_eq!(
            snap_browser_profile_root_for_command(Path::new("/var/lib/snapd/snap/bin/chromium"), "chromium", home),
            Some(PathBuf::from("/home/alice/Horizon/browser-profiles"))
        );
        assert_eq!(
            snap_browser_profile_root_for_command(Path::new("/snap/bin/firefox"), "firefox", home),
            Some(PathBuf::from("/home/alice/Horizon/browser-profiles"))
        );
    }

    #[test]
    fn non_snap_browsers_do_not_change_the_default_profile_root() {
        assert_eq!(
            snap_browser_profile_root_for_command(Path::new("/usr/bin/chromium"), "chromium", Path::new("/home/alice")),
            None
        );
        assert_eq!(
            snap_browser_profile_root_for_command(Path::new("/usr/bin/firefox"), "firefox", Path::new("/home/alice")),
            None
        );
    }

    #[test]
    fn default_firefox_discovery_detects_the_snap_launcher_behind_a_path_wrapper() {
        let mut requests = Vec::new();
        let root = snap_firefox_profile_root_with_resolver(None, Path::new("/home/alice"), |command| {
            requests.push(command.to_string());
            match command {
                "/snap/bin/firefox" => Some(PathBuf::from("/snap/bin/firefox")),
                "firefox" => Some(PathBuf::from("/usr/bin/firefox")),
                _ => None,
            }
        });

        assert_eq!(root, Some(PathBuf::from("/home/alice/Horizon/browser-profiles")));
        assert_eq!(requests, ["/snap/bin/firefox"]);
    }

    #[test]
    fn explicit_native_firefox_does_not_use_the_default_snap_launcher() {
        let mut requests = Vec::new();
        let root =
            snap_firefox_profile_root_with_resolver(Some("/usr/bin/firefox"), Path::new("/home/alice"), |command| {
                requests.push(command.to_string());
                match command {
                    "/usr/bin/firefox" => Some(PathBuf::from("/usr/bin/firefox")),
                    "/snap/bin/firefox" => Some(PathBuf::from("/snap/bin/firefox")),
                    _ => None,
                }
            });

        assert_eq!(root, None);
        assert_eq!(requests, ["/usr/bin/firefox"]);
    }

    #[test]
    fn default_native_firefox_keeps_the_hidden_root_when_no_snap_exists() {
        let root = snap_firefox_profile_root_with_resolver(None, Path::new("/home/alice"), |command| match command {
            "firefox" => Some(PathBuf::from("/usr/bin/firefox")),
            _ => None,
        });

        assert_eq!(root, None);
    }

    #[test]
    fn only_hidden_defaults_under_home_need_snap_redirection() {
        let home = Path::new("/home/alice");

        assert!(default_root_is_hidden_beneath_home(
            Path::new("/home/alice/.horizon/browser-profiles"),
            home
        ));
        assert!(!default_root_is_hidden_beneath_home(
            Path::new("/home/alice/browser-profiles"),
            home
        ));
        assert!(!default_root_is_hidden_beneath_home(
            Path::new("/tmp/horizon/browser-profiles"),
            home
        ));
    }

    #[test]
    fn explicit_profile_root_wins_over_the_platform_default() {
        let config = BrowserConfig {
            profile_root: Some(PathBuf::from("/profiles/custom")),
            ..BrowserConfig::default()
        };

        assert_eq!(
            effective_profile_root(&config, Path::new("/profiles/default")),
            PathBuf::from("/profiles/custom")
        );
    }
}
