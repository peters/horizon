use std::path::{Path, PathBuf};

use crate::{BackendKind, BrowserConfig, paths};

/// Longest name of a browser profile folder, as the settings entry of a browser
/// host and the remote tunnel allow it.
pub const MAX_PROFILE_NAME_CHARS: usize = 40;

/// The shared profile that a create opens in when it names none. Every panel of
/// it is one page of one browser, so a person signs in one time.
pub const DEFAULT_PROFILE: &str = "default";

/// The word that a create reserves for a panel that keeps nothing: its own
/// throwaway folder, gone when the panel closes.
pub const PRIVATE_PROFILE: &str = "private";

/// The folder of the throwaway profiles of private pages, under a profile root.
///
/// The folder of a shared profile is a direct child of a profile root, so no
/// sweep of the throwaway profiles can reach one: the two namespaces are apart
/// on the disk and not by the shape of a name.
pub const PRIVATE_PROFILES_FOLDER: &str = "private";

/// The profile that a create names: one shared profile by name, or a folder of
/// the panel alone.
///
/// A shared profile is one browser process and one set of cookies. A private
/// panel is a browser of its own that starts signed in nowhere and leaves no
/// folder. The two kinds never meet: a private panel never joins a shared
/// profile, and no name reaches a private folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileChoice {
    /// A profile that the panels of this Horizon share, by name.
    Shared(String),
    /// A folder of this panel alone, removed when it closes.
    Private,
}

impl ProfileChoice {
    /// Read the `profile` field of a create.
    ///
    /// # Errors
    /// Returns the words that a caller shows for a name that the rule refuses,
    /// or for a reserved word that this build does not know.
    pub fn parse(field: Option<&str>) -> Result<Self, String> {
        let Some(name) = field.map(str::trim).filter(|name| !name.is_empty()) else {
            return Ok(Self::Shared(DEFAULT_PROFILE.to_string()));
        };
        if name == PRIVATE_PROFILE {
            return Ok(Self::Private);
        }
        if !valid_profile_name(name) {
            return Err(format!(
                "the field profile refused {name:?}: use 1 to {MAX_PROFILE_NAME_CHARS} characters of a-z, 0-9 and -, \
                 with a letter or a digit at each end, or the reserved word {PRIVATE_PROFILE:?} for a panel that is not \
                 signed in"
            ));
        }
        Ok(Self::Shared(name.to_string()))
    }

    /// The shared profile that a create opens in, or `None` for a private panel.
    #[must_use]
    pub fn shared_name(&self) -> Option<&str> {
        match self {
            Self::Shared(name) => Some(name),
            Self::Private => None,
        }
    }
}

pub(crate) fn resolved_for_launch(config: &BrowserConfig) -> std::io::Result<BrowserConfig> {
    let mut resolved = config.clone();
    resolved.quality = resolved.quality.clamp(1, 100);
    // A profile folder of another computer is read as unset here as well, so a
    // launch that resolves its folders once does not keep the path of that home.
    let Some(root) = crate::here::folder("browser.profile_root", config.profile_root.as_deref()) else {
        resolved.profile_root = None;
        return Ok(resolved);
    };
    resolved.profile_root = Some(if root.is_absolute() {
        root
    } else {
        std::env::current_dir()?.join(root)
    });
    Ok(resolved)
}

#[must_use]
pub(crate) fn profile_dir(config: &BrowserConfig, panel_local_id: &str) -> PathBuf {
    profile_dir_with_default_root(config, panel_local_id, &paths::profile_root())
}

/// The folder one panel saves its page's downloads into.
///
/// The default is `Downloads/Horizon` in the home directory, where a person
/// opens a download afterwards. Each panel gets its own subfolder so that two
/// panels never replace each other's file of one name, and a panel that shares
/// a browser profile shares that profile's subfolder.
#[must_use]
pub(crate) fn downloads_dir(config: &BrowserConfig, panel_local_id: &str) -> PathBuf {
    downloads_dir_in(config, panel_local_id, &paths::home_dir())
}

#[must_use]
pub(crate) fn downloads_dir_in(config: &BrowserConfig, panel_local_id: &str, home: &Path) -> PathBuf {
    // A download folder of another computer is no folder of this one, and the
    // default folder is where a person of this computer looks for a download.
    let root = crate::here::folder("browser.downloads_root", config.downloads_root.as_deref())
        .unwrap_or_else(|| home.join("Downloads").join("Horizon"));
    root.join(paths::readable_id(panel_local_id))
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
    let root = effective_profile_root(config, default_root);
    // A page that keeps nothing lives under the folder of the private pages, so
    // its folder never shares a name with the folder of a shared profile, whose
    // folder is a direct child of the root.
    if config.profile.as_deref() == Some(PRIVATE_PROFILE) {
        return root
            .join(PRIVATE_PROFILES_FOLDER)
            .join(paths::safe_local_id(panel_local_id));
    }
    root.join(paths::safe_local_id(panel_local_id))
}

#[must_use]
pub(crate) fn effective_profile_root(config: &BrowserConfig, default_root: &Path) -> PathBuf {
    if let Some(root) = crate::here::folder("browser.profile_root", config.profile_root.as_deref()) {
        return root;
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

/// Whether `name` is one profile name that a host file of `horizon-browser
/// host` accepts: 1 to 40 of `a-z`, `0-9` and `-`, with a letter or a digit at
/// each end. The name is part of each page ID and of the profile folder, so it
/// is never a path or a shell word of its own.
///
/// The reserved word [`PRIVATE_PROFILE`] names no profile, so this rule refuses
/// it here for every caller: the settings entry of a browser host, the driver of
/// that host, and the create that reads the word as the private panel.
#[must_use]
pub fn valid_profile_name(name: &str) -> bool {
    if name == PRIVATE_PROFILE {
        return false;
    }
    let edge = |byte: Option<&u8>| byte.is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    (1..=MAX_PROFILE_NAME_CHARS).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && edge(name.as_bytes().first())
        && edge(name.as_bytes().last())
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

    /// The field of a create reads as the shared profile that it names, and the
    /// reserved word reads as a private panel of its own.
    #[test]
    fn a_create_names_a_shared_profile_or_the_private_panel() {
        assert_eq!(
            ProfileChoice::parse(None),
            Ok(ProfileChoice::Shared(DEFAULT_PROFILE.to_string())),
            "a create that names no profile opens in the shared default"
        );
        assert_eq!(
            ProfileChoice::parse(Some("  ")),
            Ok(ProfileChoice::Shared(DEFAULT_PROFILE.to_string())),
            "a blank field is no name"
        );
        assert_eq!(
            ProfileChoice::parse(Some("work")),
            Ok(ProfileChoice::Shared("work".to_string()))
        );
        assert_eq!(ProfileChoice::parse(Some(PRIVATE_PROFILE)), Ok(ProfileChoice::Private));
        assert_eq!(
            ProfileChoice::parse(Some("private")).map(|choice| choice.shared_name().map(str::to_string)),
            Ok(None),
            "the private panel joins no shared profile"
        );
        let refusal = ProfileChoice::parse(Some("../private")).expect_err("a path is no name");
        assert!(
            refusal.contains("the field profile") && refusal.contains("private") && refusal.contains("a-z, 0-9"),
            "the refusal names the field, the rule and the reserved word: {refusal}"
        );
        assert!(
            ProfileChoice::parse(Some("Private")).is_err(),
            "the reserved word is lower-case only"
        );
    }

    #[test]
    fn a_profile_name_is_one_folder_under_a_letter_or_a_digit() {
        for accepted in ["mail", "a", "work-1", "1", &"a".repeat(MAX_PROFILE_NAME_CHARS)] {
            assert!(valid_profile_name(accepted), "{accepted}");
        }
        for refused in [
            "",
            "private",
            "Mail",
            "mail_box",
            "-mail",
            "mail-",
            "mail box",
            "../mail",
            "mail\n",
            &"a".repeat(MAX_PROFILE_NAME_CHARS + 1),
        ] {
            assert!(!valid_profile_name(refused), "{refused:?}");
        }
    }

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
    fn a_panel_saves_downloads_in_its_own_folder_in_the_home_downloads_folder() {
        let home = PathBuf::from("/home/alice");
        assert_eq!(
            downloads_dir_in(&BrowserConfig::default(), "panel-1", &home),
            PathBuf::from("/home/alice/Downloads/Horizon/panel-1")
        );
    }

    #[test]
    fn an_explicit_downloads_root_holds_one_folder_for_each_panel() {
        let data = tempfile::tempdir().expect("a folder of this computer");
        let config = BrowserConfig {
            downloads_root: Some(data.path().join("downloads")),
            ..BrowserConfig::default()
        };
        // The root of the section exists on this computer, so each panel saves into
        // its own folder under it.
        std::fs::create_dir_all(config.downloads_root.as_ref().expect("the root")).expect("the folder");
        assert_eq!(
            downloads_dir_in(&config, "panel-1", Path::new("/home/alice")),
            config.downloads_root.as_ref().expect("the root").join("panel-1")
        );
    }

    /// A shared section can name the download folder of another computer. A folder
    /// that this computer does not have is read as unset, so the person of this
    /// computer finds the download where a download of this computer goes.
    #[test]
    fn a_downloads_root_of_another_computer_saves_where_a_download_of_this_one_goes() {
        let config = BrowserConfig {
            downloads_root: Some(PathBuf::from(
                "/nonexistent/a-state-folder-of-another-computer/downloads",
            )),
            ..BrowserConfig::default()
        };
        assert_eq!(
            downloads_dir_in(&config, "panel-1", Path::new("/home/alice")),
            PathBuf::from("/home/alice/Downloads/Horizon/panel-1")
        );
    }

    #[test]
    fn a_shared_profile_keeps_one_download_folder_of_its_own() {
        let config = BrowserConfig::default();
        assert_eq!(
            downloads_dir_in(&config, "shared-profile", Path::new("/home/alice")),
            downloads_dir_in(&config, "shared-profile", Path::new("/home/alice"))
        );
        assert_ne!(
            downloads_dir_in(&config, "shared-profile", Path::new("/home/alice")),
            downloads_dir_in(&config, "other-panel", Path::new("/home/alice"))
        );
    }

    #[test]
    fn explicit_profile_root_wins_over_the_platform_default() {
        let profiles = tempfile::tempdir().expect("a folder of this computer");
        let config = BrowserConfig {
            profile_root: Some(profiles.path().join("custom")),
            ..BrowserConfig::default()
        };
        std::fs::create_dir_all(config.profile_root.as_ref().expect("the root")).expect("the folder");

        assert_eq!(
            effective_profile_root(&config, Path::new("/profiles/default")),
            config.profile_root.as_ref().expect("the root").clone()
        );
    }

    /// A shared section can name the profile folder of another computer, and this
    /// computer starts its panels in its own folder then.
    #[test]
    fn a_profile_root_of_another_computer_uses_the_folder_of_this_one() {
        let config = BrowserConfig {
            profile_root: Some(PathBuf::from(
                "/nonexistent/a-state-folder-of-another-computer/profiles",
            )),
            ..BrowserConfig::default()
        };
        assert_eq!(
            effective_profile_root(&config, Path::new("/profiles/default")),
            PathBuf::from("/profiles/default")
        );
        let resolved = resolved_for_launch(&config).expect("the folders of this launch");
        assert_eq!(
            resolved.profile_root, None,
            "a profile folder of another computer is no folder of this launch"
        );
    }
}
