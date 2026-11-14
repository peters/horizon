//! Persistent profile identity and host membership for shared browser panels.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use horizon_browser::session::{BrowserShutdownSignal, SharedBrowserSession, SharedSessionGroup};

use super::{BackendKind, BrowserConfig, BrowserPanelState, BrowserStatus};
use crate::panel::{PanelKind, PanelOptions};

pub(super) type ProfileSession = SharedSessionGroup;

struct RegisteredSession {
    session: Weak<ProfileSession>,
    ever_shared: bool,
    /// The launch settings of the browser that runs this profile. A second page
    /// joins that browser or is refused; it never starts a browser of its own
    /// settings on one profile folder.
    launch: ProfileLaunch,
}

type Registry = Mutex<HashMap<PathBuf, RegisteredSession>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}

/// The folder that names one profile, without the engine folder inside it.
///
/// One profile is one engine: the first panel of a profile fixes it, and the
/// next create of that name in another engine is refused. The registry therefore
/// names a profile by the folder above the engine folder.
fn profile_key(config: &BrowserConfig, profile_id: &str) -> PathBuf {
    let folder = config.profile_dir(profile_id);
    // `profile_dir` ends with the engine folder (`chromium`, `firefox`), so the
    // folder of the profile is the one above it.
    folder.parent().map_or(folder.clone(), Path::to_path_buf)
}

pub(super) fn has_shared_members(config: &BrowserConfig, group: &Arc<ProfileSession>) -> bool {
    registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&profile_key(config, group.profile_id()))
        .is_some_and(|entry| entry.ever_shared && Weak::ptr_eq(&entry.session, &Arc::downgrade(group)))
}

pub(super) fn acquire(config: &BrowserConfig, profile_id: &str) -> Arc<ProfileSession> {
    let key = profile_key(config, profile_id);
    let mut sessions = registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    sessions.retain(|_, entry| entry.session.strong_count() > 0);
    if let Some(entry) = sessions.get_mut(&key)
        && let Some(session) = entry.session.upgrade()
    {
        // Closed siblings may still be shutting down after their host handles
        // disappear. Keep a shared profile on one backend for this lifetime.
        entry.ever_shared = true;
        return session;
    }
    let session = Arc::new(if config.backend == BackendKind::FirefoxBidi {
        SharedSessionGroup::new_firefox(profile_id.to_string())
    } else {
        SharedSessionGroup::Chromium(SharedBrowserSession::new(profile_id.to_string()))
    });
    sessions.insert(
        key,
        RegisteredSession {
            session: Arc::downgrade(&session),
            ever_shared: false,
            launch: ProfileLaunch::of(config),
        },
    );
    session
}

/// The launch settings that decide the process of one profile.
///
/// Two pages of one profile are two pages of one browser, so they cannot
/// disagree about the engine, the executable, the extra arguments or the
/// window. The first panel of a profile pins these, and the next create that
/// asks for another set is refused before its browser starts.
#[derive(Clone, PartialEq, Eq)]
struct ProfileLaunch {
    backend: BackendKind,
    headless: bool,
    command: Option<String>,
    extra_args: Vec<String>,
    disclosure: horizon_browser::AutomationDisclosurePolicy,
    background_windows: bool,
}

impl ProfileLaunch {
    fn of(config: &BrowserConfig) -> Self {
        Self {
            backend: config.backend,
            headless: config.headless,
            command: config.command.clone(),
            extra_args: config.extra_args.clone(),
            disclosure: config.automation_disclosure,
            background_windows: config.background_windows,
        }
    }

    /// What this create asks for and the running browser of the profile cannot
    /// take, in the words that a panel shows.
    fn difference(&self, asked: &Self) -> Option<String> {
        if self.backend != asked.backend {
            return Some(format!(
                "its panels run {}, and this create asks for {}",
                backend_name(self.backend),
                backend_name(asked.backend)
            ));
        }
        if self.headless != asked.headless {
            return Some(format!(
                "its panels run {}, and this create asks for {}",
                window_word(self.headless),
                window_word(asked.headless)
            ));
        }
        if self.command != asked.command {
            return Some("its panels use another executable".to_string());
        }
        if self.extra_args != asked.extra_args {
            return Some("its panels pass other extra arguments".to_string());
        }
        if self.disclosure != asked.disclosure {
            return Some("its panels use another automation disclosure".to_string());
        }
        if self.background_windows != asked.background_windows {
            return Some("its panels use another window setting".to_string());
        }
        None
    }
}

/// Refuse a create that cannot join the browser of its profile, before that
/// browser starts.
///
/// The words name the profile and say what the caller can do, because a create
/// that reached the driver would show a protocol error of the engine instead.
///
/// # Errors
/// The backend keeps no shared profile, the profile runs with another launch
/// setting, or another browser holds its folder.
pub(super) fn refuse_unjoinable(config: &BrowserConfig, profile_id: &str) -> crate::error::Result<()> {
    if !matches!(config.backend, BackendKind::ChromiumCdp | BackendKind::FirefoxBidi) {
        return Err(crate::error::Error::State(format!(
            "the shared profile {profile_id} needs a local Chromium or Firefox; this panel asks for {}",
            backend_name(config.backend)
        )));
    }
    let folder = config.profile_dir(profile_id);
    let difference = {
        let sessions = registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = sessions
            .get(&profile_key(config, profile_id))
            .filter(|entry| entry.session.strong_count() > 0);
        entry.map(|entry| entry.launch.difference(&ProfileLaunch::of(config)))
    };
    if let Some(difference) = difference {
        return match difference {
            Some(difference) => Err(crate::error::Error::State(format!(
                "the shared profile {profile_id} cannot take this page: {difference}. Close the panels of that \
                 profile, or create this panel with the setting of that browser"
            ))),
            None => Ok(()),
        };
    }
    // No panel of this Horizon runs the profile, so a lock in its folder belongs
    // to another browser.
    match held_by_another_browser(&folder) {
        Some(held) => Err(crate::error::Error::State(format!(
            "the folder of the shared profile {profile_id} is held by {held}: close that browser, or open a private panel"
        ))),
        None => Ok(()),
    }
}

fn window_word(headless: bool) -> &'static str {
    if headless { "headless" } else { "with a window" }
}

fn backend_name(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::ChromiumCdp => "Chromium",
        BackendKind::FirefoxBidi => "Firefox",
        BackendKind::SafariWebDriver => "Safari",
    }
}

/// The lock of a browser process on the folder of one profile.
///
/// Chromium and Firefox keep a lock in the profile folder that names the
/// process that holds it, so a lock that a crash left behind does not stop a
/// create. On another system than Linux the engine reports the lock itself.
#[cfg(target_os = "linux")]
fn held_by_another_browser(folder: &Path) -> Option<&'static str> {
    for marker in ["SingletonLock", "lock"] {
        let Some(target) = std::fs::read_link(folder.join(marker)).ok() else {
            continue;
        };
        let pid = target
            .to_string_lossy()
            .rsplit(['-', '+'])
            .next()
            .and_then(|pid| pid.parse::<u32>().ok());
        if pid.is_some_and(|pid| Path::new("/proc").join(pid.to_string()).exists()) {
            return Some("another browser process");
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
const fn held_by_another_browser(_folder: &Path) -> Option<&'static str> {
    None
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

    /// Whether this panel is a private one: a browser of its own that is signed
    /// in nowhere and whose folder goes away at its close.
    #[must_use]
    pub fn is_private_profile(&self) -> bool {
        self.shared_session.is_some() && self.shared_profile_id().is_none()
    }

    /// The name of the shared profile of this panel.
    ///
    /// `None` is a private panel: its folder belongs to this panel alone, keeps
    /// nothing after the close, and is the folder of its own id.
    #[must_use]
    pub fn shared_profile_id(&self) -> Option<&str> {
        let profile_id = self.shared_session.as_ref()?.profile_id();
        (profile_id != self.panel_local_id).then_some(profile_id)
    }

    /// Permanently close this panel. Only a private panel removes its profile
    /// folder, and only after the driver has released the profile lock.
    ///
    /// A shared profile holds the sign-ins of every panel of it, so no close of
    /// a panel removes its folder.
    ///
    /// The returned signal is what actually starts a profile deletion: it must
    /// be polled or waited on (see [`BrowserShutdownSignal`]). Dropping it
    /// leaves the folder on disk.
    #[must_use = "profile cleanup only starts when the returned signal is polled or waited on"]
    pub(crate) fn close_permanently(&mut self) -> BrowserShutdownSignal {
        // A page of a browser host ends with its panel: no link of that panel
        // comes back, so the page must not wait on the machine of the host for
        // ever.
        self.close_host_driver_page();
        self.request_shutdown();
        if self.is_attached() {
            // The Chromium of another machine keeps its profile there, and this
            // computer must not delete a local folder that it never made.
            return self
                .take_shutdown_signal()
                .unwrap_or_else(BrowserShutdownSignal::completed);
        }
        let private_folder = self.shared_profile_id().is_none().then(|| {
            super::profile_dir_for_home(
                &self.config,
                &crate::horizon_home::HorizonHome::resolve(),
                &self.panel_local_id,
            )
        });
        let group = self.shared_session.take();
        let last_panel = group.as_ref().is_none_or(|group| Arc::strong_count(group) == 1);
        let signal = self
            .take_shutdown_signal()
            .unwrap_or_else(BrowserShutdownSignal::completed);
        if !last_panel {
            return signal;
        }
        let signal = match private_folder {
            Some(folder) => signal.with_profile_cleanup(folder),
            None => signal,
        };
        if let Some(group) = group {
            signal.with_shared_profile_cleanup(group)
        } else {
            signal
        }
    }

    #[must_use]
    pub fn can_duplicate(&self) -> bool {
        !self.is_remote()
            && matches!(self.backend(), BackendKind::ChromiumCdp | BackendKind::FirefoxBidi)
            && matches!(self.status, BrowserStatus::Ready)
            && self.shared_session.is_some()
    }

    /// Whether this panel keeps nothing of a browser profile.
    ///
    /// A panel of a browser of this computer keeps nothing when it has no shared
    /// profile: its own folder, gone at its close. A panel of a browser host
    /// keeps nothing when it asked for a private page: a page of an isolated
    /// context of the browser of that machine, which takes no cookie of the
    /// profile and leaves none in it.
    #[must_use]
    pub fn is_private_page(&self) -> bool {
        if self.is_attached() || self.is_host_driver() {
            return self.config.profile.as_deref() == Some(horizon_browser::PRIVATE_PROFILE);
        }
        self.is_private_profile()
    }

    /// The name of the shared profile that this panel shows, when it has one:
    /// the profile of this computer, or the profile of the browser of its host.
    ///
    /// `None` is a private panel, whose profile name is the reserved word alone.
    #[must_use]
    pub fn visible_profile_name(&self) -> Option<&str> {
        if let Some(name) = self.shared_profile_id() {
            return Some(name);
        }
        if self.is_private_page() {
            return None;
        }
        self.config
            .profile
            .as_deref()
            .filter(|name| *name != horizon_browser::PRIVATE_PROFILE)
    }

    /// Whether this panel can open a private panel beside it.
    ///
    /// Every browser panel can: a panel of this computer opens a throwaway folder
    /// of its own, and a panel of a browser host opens a page of an isolated
    /// context of that browser, which keeps none of the sign-ins of the profile.
    #[must_use]
    pub fn can_open_private(&self) -> bool {
        !self.is_remote()
    }

    /// The browser host of this panel, when it runs on one.
    #[must_use]
    pub fn browser_host_of_panel(&self) -> Option<crate::runtime_state::BrowserHostState> {
        self.attached_host_state()
    }

    /// Construct the options of a private panel with the page of this one.
    ///
    /// The panel gets a profile folder of its own, or a page of an isolated
    /// context of the browser of its host, so it is signed in nowhere and keeps
    /// nothing after its close.
    #[must_use]
    pub fn private_panel_options(&self) -> Option<PanelOptions> {
        self.can_open_private().then(|| {
            let mut config = self.config.clone();
            config.profile = Some(horizon_browser::PRIVATE_PROFILE.to_string());
            PanelOptions {
                kind: PanelKind::Browser,
                command: self.url.clone().or_else(|| self.requested_url.clone()),
                browser_config: Some(config),
                browser_session_id: None,
                browser_private: true,
                // A private panel of a browser host is a page of that host, so
                // the person's private panel follows the entry of the settings as
                // every other browser panel of that person does.
                follows_browser_host: true,
                ..PanelOptions::default()
            }
        })
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
            browser_private: self.is_private_page(),
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
    use std::io::{Read as _, Write as _};
    use std::time::{Duration, Instant};

    fn panel(root: &std::path::Path, id: &str, profile: &str) -> BrowserPanelState {
        let mut state = BrowserPanelState::inert();
        state.config.profile_root = Some(root.to_path_buf());
        state.panel_local_id = id.to_string();
        state.shared_session = Some(acquire(&state.config, profile));
        state.status = BrowserStatus::Ready;
        state.url = Some("https://example.test/current".into());
        state
    }

    /// The folder of a shared profile holds the sign-ins of every panel of it, so
    /// the close of one panel leaves it, even when that panel is the last one.
    #[test]
    fn a_shared_profile_folder_survives_the_close_of_its_last_panel() {
        let root = tempfile::tempdir().expect("profiles");
        let mut first = panel(root.path(), "page-1", "work");
        let mut second = panel(root.path(), "page-2", "work");
        let profile = first.config.profile_dir("work");
        std::fs::create_dir_all(&profile).expect("profile");
        std::fs::write(profile.join("Cookies"), "a sign-in").expect("fixture");
        assert_eq!(second.shared_profile_id(), Some("work"), "a page of a named profile");
        assert!(first.close_permanently().wait(Duration::from_secs(2)));
        assert!(profile.join("Cookies").exists(), "a sibling still uses the profile");
        assert!(second.close_permanently().wait(Duration::from_secs(2)));
        assert!(
            profile.join("Cookies").exists(),
            "the last panel of a shared profile keeps the sign-ins of the person"
        );
    }

    /// A saved panel names the profile that its page joined, so a restart opens
    /// the same page of the same browser, and a private panel comes back private.
    #[test]
    fn a_saved_panel_keeps_its_profile_kind() {
        let root = tempfile::tempdir().expect("profiles");
        let shared = panel(root.path(), "page-1", "work");
        assert_eq!(
            crate::runtime_state::browser_profile_state(&shared, false)
                .session_id
                .as_deref(),
            Some("work")
        );
        let private = panel(root.path(), "page-2", "page-2");
        assert_eq!(
            crate::runtime_state::browser_profile_state(&private, false).session_id,
            None,
            "a private panel saves no shared profile"
        );
    }

    /// A private panel keeps its folder while it lives and none after it.
    #[test]
    fn a_private_panel_folder_goes_away_at_close() {
        let root = tempfile::tempdir().expect("profiles");
        let mut private = panel(root.path(), "page-1", "page-1");
        let folder = private.config.profile_dir("page-1");
        std::fs::create_dir_all(&folder).expect("profile");
        std::fs::write(folder.join("Cookies"), "nothing to keep").expect("fixture");
        assert_eq!(private.shared_profile_id(), None, "a private panel joins no profile");
        assert!(private.close_permanently().wait(Duration::from_secs(2)));
        assert!(!folder.exists(), "the throwaway folder of a private panel goes away");
    }

    /// Two pages of one profile are two pages of one browser: the second create
    /// cannot ask for another engine, and the words name the profile.
    #[test]
    fn a_create_in_another_engine_of_a_running_profile_is_refused() {
        let root = tempfile::tempdir().expect("profiles");
        let config = BrowserConfig {
            profile_root: Some(root.path().to_path_buf()),
            ..BrowserConfig::default()
        };
        let group = acquire(&config, "work");
        assert!(refuse_unjoinable(&config, "work").is_ok(), "the same engine joins");
        let firefox = BrowserConfig {
            backend: BackendKind::FirefoxBidi,
            ..config.clone()
        };
        let refusal = refuse_unjoinable(&firefox, "work")
            .expect_err("another engine cannot join")
            .to_string();
        assert!(refusal.contains("work"), "the refusal names the profile: {refusal}");
        assert!(refusal.contains("Chromium"), "the refusal names the engine: {refusal}");
        drop(group);
    }

    /// The first panel of a profile pins how its browser starts. A second create
    /// with another setting is refused before its browser starts.
    #[test]
    fn a_create_with_another_launch_setting_of_a_running_profile_is_refused() {
        for (asked, setting) in [
            (
                BrowserConfig {
                    headless: false,
                    ..BrowserConfig::default()
                },
                "headless",
            ),
            (
                BrowserConfig {
                    command: Some("/usr/bin/other-browser".into()),
                    ..BrowserConfig::default()
                },
                "executable",
            ),
            (
                BrowserConfig {
                    extra_args: vec!["--lang=en".into()],
                    ..BrowserConfig::default()
                },
                "extra arguments",
            ),
        ] {
            let root = tempfile::tempdir().expect("profiles");
            let config = BrowserConfig {
                profile_root: Some(root.path().to_path_buf()),
                ..BrowserConfig::default()
            };
            let group = acquire(&config, "work");
            let asked = BrowserConfig {
                profile_root: Some(root.path().to_path_buf()),
                ..asked
            };
            let refusal = refuse_unjoinable(&asked, "work")
                .expect_err("another launch setting")
                .to_string();
            assert!(refusal.contains("work"), "{refusal}");
            assert!(
                refusal.to_lowercase().contains(setting),
                "the refusal names {setting}: {refusal}"
            );
            drop(group);
        }
    }

    /// A shared profile needs a local Chromium or Firefox: a Safari panel has no
    /// process to share, and a folder that another browser holds is refused.
    #[test]
    fn a_profile_of_a_backend_without_a_shared_process_is_refused() {
        let root = tempfile::tempdir().expect("profiles");
        let safari = BrowserConfig {
            backend: BackendKind::SafariWebDriver,
            profile_root: Some(root.path().to_path_buf()),
            ..BrowserConfig::default()
        };
        let refusal = refuse_unjoinable(&safari, "work")
            .expect_err("Safari keeps no shared profile")
            .to_string();
        assert!(refusal.contains("work") && refusal.contains("Safari"), "{refusal}");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_profile_folder_that_another_browser_holds_is_refused() {
        let root = tempfile::tempdir().expect("profiles");
        let config = BrowserConfig {
            profile_root: Some(root.path().to_path_buf()),
            ..BrowserConfig::default()
        };
        let folder = config.profile_dir("work");
        std::fs::create_dir_all(&folder).expect("profile");
        let lock = folder.join("SingletonLock");
        // A lock names the process that holds the profile, and a live process of
        // this computer holds it now.
        std::os::unix::fs::symlink(format!("this-computer-{}", std::process::id()), &lock).expect("lock");
        let refusal = refuse_unjoinable(&config, "work")
            .expect_err("a held folder cannot be joined")
            .to_string();
        assert!(refusal.contains("work"), "{refusal}");
        // A lock that a crash left behind names a process that is gone, and the
        // browser of the profile starts as usual.
        std::fs::remove_file(&lock).expect("remove lock");
        std::os::unix::fs::symlink("this-computer-4294967290", &lock).expect("stale lock");
        assert!(refuse_unjoinable(&config, "work").is_ok());
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
    fn standalone_profile_can_still_switch_backend() {
        let root = tempfile::tempdir().expect("profiles");
        let mut source = panel(root.path(), "source", "source");
        let (completion, receiver) = std::sync::mpsc::channel();
        source.teardown_signal = Some(Box::new(horizon_browser::session::BrowserShutdownSignal::for_test(
            receiver,
        )));
        source.switch_backend(BackendKind::FirefoxBidi);
        assert_eq!(source.backend(), BackendKind::FirefoxBidi);
        assert!(source.navigation_error.is_none());
        assert!(source.pending_relaunch.is_some());
        completion.send(()).expect("finish old driver");
    }

    #[test]
    fn closing_sibling_keeps_backend_fixed_through_pending_shutdown() {
        for backend in [BackendKind::ChromiumCdp, BackendKind::FirefoxBidi] {
            let root = tempfile::tempdir().expect("profiles");
            let mut source = BrowserPanelState::inert();
            source.config.profile_root = Some(root.path().to_path_buf());
            source.config.backend = backend;
            source.panel_local_id = "source".into();
            source.status = BrowserStatus::Ready;
            source.shared_session = Some(acquire(&source.config, "source"));
            assert!(!has_shared_members(
                &source.config,
                source.shared_session.as_ref().expect("standalone")
            ));
            let mut duplicate = BrowserPanelState::inert();
            duplicate.config = source.config.clone();
            duplicate.panel_local_id = "duplicate".into();
            duplicate.shared_session = Some(acquire(&duplicate.config, "source"));
            let (completion, receiver) = std::sync::mpsc::channel();
            duplicate.teardown_signal = Some(Box::new(horizon_browser::session::BrowserShutdownSignal::for_test(
                receiver,
            )));
            let shutdown = duplicate.close_permanently();
            drop(duplicate);
            assert!(!shutdown.is_complete());
            let group = Arc::downgrade(source.shared_session.as_ref().expect("source group"));
            assert_eq!(
                Arc::strong_count(source.shared_session.as_ref().expect("sole panel")),
                1
            );
            let other = if backend == BackendKind::ChromiumCdp {
                BackendKind::FirefoxBidi
            } else {
                BackendKind::ChromiumCdp
            };
            source.switch_backend(other);
            assert_eq!(source.backend(), backend);
            assert!(source.navigation_error.is_some());
            assert!(Weak::ptr_eq(
                &Arc::downgrade(source.shared_session.as_ref().expect("unchanged group")),
                &group
            ));
            completion.send(()).expect("finish sibling");
            assert!(shutdown.wait(Duration::from_secs(2)));
            drop(shutdown);
            // Membership is sticky because an absent host handle alone does
            // not establish that every sibling has released the profile.
            source.switch_backend(other);
            assert_eq!(source.backend(), backend);
            drop(group);
            let profile = source.config.profile_dir("source");
            std::fs::create_dir_all(&profile).expect("profile");
            assert!(source.close_permanently().wait(Duration::from_secs(2)));
            assert!(!profile.exists());
        }
    }

    /// A page of a shared profile keeps the profile when a sibling closes, and a
    /// duplicate of a private panel is another private panel.
    #[test]
    fn closing_duplicate_first_preserves_the_shared_profile() {
        let root = tempfile::tempdir().expect("profiles");
        let mut source = panel(root.path(), "page-1", "work");
        let mut duplicate = panel(root.path(), "page-2", "work");
        let profile = source.config.profile_dir("work");
        std::fs::create_dir_all(&profile).expect("profile");
        assert!(duplicate.close_permanently().wait(Duration::from_secs(2)));
        assert!(profile.exists());
        assert!(source.close_permanently().wait(Duration::from_secs(2)));
        assert!(profile.exists(), "a shared profile outlives its panels");

        let private = panel(root.path(), "page-3", "page-3");
        let options = private.duplicate_options().expect("options");
        assert!(
            options.browser_session_id.is_none(),
            "a duplicate of a private panel keeps nothing either"
        );
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

    /// The private panel of the menu takes the page of its source and no profile.
    #[test]
    fn a_private_panel_takes_the_page_of_its_source_and_no_profile() {
        let root = tempfile::tempdir().expect("profiles");
        let source = panel(root.path(), "page-1", "work");
        let options = source
            .private_panel_options()
            .expect("a local panel opens a private one");
        assert!(
            options.browser_session_id.is_none(),
            "a private panel joins no shared profile"
        );
        assert_eq!(options.command.as_deref(), Some("https://example.test/current"));
        assert!(
            options.browser_config.is_some(),
            "the private panel keeps the engine of its source"
        );

        let remote = BrowserPanelState::restored_remote("page-2", &BrowserConfig::default(), "phone".into(), None);
        assert!(
            !remote.can_open_private(),
            "a remote device keeps its own browser, so it opens no private panel of this computer"
        );
    }

    /// A page of this procedure sets one cookie and shows the cookie that it read
    /// before it set its own, so a page that joins a signed-in profile reads
    /// `signin=yes` and a private page reads none.
    const COOKIE_PAGE: &str = r#"<!doctype html><title>pending</title>
<script>
const seen = document.cookie;
document.cookie = "signin=yes; path=/";
document.title = "seen: " + (seen || "none");
</script>"#;

    fn serve_cookie_page() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the test page");
        let address = listener.local_addr().expect("address of the test page");
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request);
                let body = COOKIE_PAGE.as_bytes();
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
            }
        });
        format!("http://{address}/")
    }

    /// Wait for the title that the page of `state` reports.
    fn page_title(state: &mut BrowserPanelState, wanted: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut title = String::new();
        while Instant::now() < deadline {
            let _ = state.drain_events();
            title = state.title.clone();
            if title.contains(wanted) {
                return title;
            }
            if let BrowserStatus::Stopped { code } = state.status {
                panic!("the browser stopped with {code:?} before its page read {wanted:?}: {title}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the page reported {title:?} and not {wanted:?} within the deadline");
    }

    /// The judge of the shared profile and the private panel, with a real browser
    /// and a page of this computer.
    ///
    /// Run it with:
    ///
    /// ```text
    /// cargo test -p horizon-core --lib a_shared_profile_keeps_a_sign_in -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "starts the Firefox and geckodriver of this computer"]
    fn a_shared_profile_keeps_a_sign_in_and_a_private_panel_reads_none() {
        let url = serve_cookie_page();
        let profiles = tempfile::tempdir().expect("profile root");
        let config = BrowserConfig {
            backend: BackendKind::FirefoxBidi,
            profile_root: Some(profiles.path().to_path_buf()),
            ..BrowserConfig::default()
        };
        let mut first = BrowserPanelState::start_with_session(
            "judge-page-1".to_string(),
            &config,
            Some(url.clone()),
            Some(horizon_browser::DEFAULT_PROFILE),
        )
        .expect("the first page of the shared profile");
        let title = page_title(&mut first, "seen:");
        assert_eq!(title, "seen: none", "the first page signs in");

        let mut second = BrowserPanelState::start_with_session(
            "judge-page-2".to_string(),
            &config,
            Some(url.clone()),
            Some(horizon_browser::DEFAULT_PROFILE),
        )
        .expect("the second page of the shared profile");
        let title = page_title(&mut second, "seen:");
        assert_eq!(title, "seen: signin=yes", "a second create reads the sign-in");

        let mut private = BrowserPanelState::start_with_session("judge-private".to_string(), &config, Some(url), None)
            .expect("a private panel");
        let title = page_title(&mut private, "seen:");
        assert_eq!(title, "seen: none", "a private panel reads no sign-in");

        let shared_folder = config.profile_dir(horizon_browser::DEFAULT_PROFILE);
        let private_folder = config.profile_dir("judge-private");
        assert!(shared_folder.exists(), "the shared profile is on disk");
        assert!(
            private_folder.exists(),
            "the private profile lives while its panel lives"
        );

        for panel in [&mut first, &mut second, &mut private] {
            let signal = panel.close_permanently();
            assert!(
                signal.wait(Duration::from_secs(30)) || signal.force_cleanup(Duration::from_secs(10)),
                "the browser and its profile must stop within the deadline"
            );
        }
        assert!(
            shared_folder.exists(),
            "the shared profile keeps the sign-ins after the last close"
        );
        assert!(!private_folder.exists(), "the private profile is gone after its close");
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
