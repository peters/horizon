//! Which machine serves the browser panels of a new panel, and where a saved
//! page comes back.
//!
//! The rule `crate::browser::new_browser_panel` decides which machine serves a
//! new browser panel, because it reads the settings entry of this app; this
//! module turns that answer into the mode of the spawn, and a panel of a saved
//! run comes back on the host and profile that served it. Both modes are decided
//! here, so the panel spawn of `spawn.rs` reads one answer, and a machine of the
//! machine list takes the login of that machine and never the SSH configuration
//! of the person.

use horizon_browser_control::ssh_forward::Ssh;

use crate::browser::BrowserPlace;
use crate::editor::PanelContent;
use crate::error::Result;
use crate::ssh::SshConnection;

use super::{Panel, PanelKind, StaticPanelSeed};

/// The mode of a browser panel that a previous run saved: the browser host and
/// profile that served it, when the saved panel carries them, and otherwise the
/// mode that its other saved fields give.
///
/// The entry of today's settings takes no part: a saved panel keeps the host
/// that served it, and a live page cannot move between machines.
///
/// # Errors
/// Returns the reason that `AttachedHost::new` gives for a saved target that
/// `ssh` would take as an option or split, so no such value reaches a forward.
pub(super) fn restored_browser_mode(
    mode: BrowserSpawnMode,
    browser_host: Option<crate::runtime_state::BrowserHostState>,
) -> Result<BrowserSpawnMode> {
    match (browser_host, mode) {
        (Some(host), BrowserSpawnMode::Local(None)) => {
            let mut attached = crate::browser::AttachedHost::with_engine(
                host.ssh_target,
                host.profile.as_deref(),
                host.engine.unwrap_or_default(),
                host.command.as_deref(),
            )?;
            // A saved panel of a machine of the list comes back through the machine
            // key of this computer, which is the only login that such a machine has.
            if let Some(login) = host.login {
                attached = attached.with_login(login);
            }
            Ok(BrowserSpawnMode::RestoredHost(attached))
        }
        (_, mode) => Ok(mode),
    }
}
/// The mode of the browser panel that `opts` asks for, as the spawn of a panel
/// reads it. A test names this answer instead of starting a browser.
pub(super) fn browser_spawn_for(opts: &mut crate::PanelOptions) -> Result<BrowserSpawnMode> {
    // A panel that follows the browser host of the settings takes the entry that
    // this app read for it, and no caller sets the entry of a panel of its own.
    let browser_host = opts.browser_host_entry.take().filter(|_| opts.follows_browser_host);
    let mode = match (opts.remote_session.take(), opts.remote_target.take()) {
        (Some(request), _) => BrowserSpawnMode::Remote(Box::new(request)),
        (None, Some(target)) => BrowserSpawnMode::RestoredRemote(target),
        (None, None) => BrowserSpawnMode::Local(opts.browser_session_id.take()),
    };
    // A saved panel comes back on the browser host and profile that served it.
    // The host of today's settings does not decide a saved panel, and a live page
    // cannot move between machines.
    let mode = restored_browser_mode(mode, opts.browser_host.take())?;
    browser_spawn_mode(
        mode,
        opts.browser_place.take(),
        browser_host.as_ref(),
        opts.ssh_connection.take(),
    )
}

/// The machine that serves the page of a browser panel, in the terms that a test
/// reads: this computer in one engine, or the browser host of the settings in the
/// profile of that host.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BrowserMachine {
    /// This computer serves the page in `backend`, in the shared profile
    /// `profile`, or in a private page when `private` is true.
    ThisComputer {
        backend: crate::browser::BackendKind,
        profile: Option<String>,
        private: bool,
    },
    /// The browser host `ssh_target` serves the page in `profile` of that machine.
    /// `private` says that the page keeps nothing of that profile.
    Host {
        ssh_target: String,
        profile: String,
        private: bool,
    },
}

/// The machine that serves the page of `opts`, as the spawn of a panel decides it.
///
/// A test names this answer instead of starting a browser, so the check that a
/// create reaches the host goes from the request to the machine of the panel and
/// runs no `ssh` and no engine.
#[doc(hidden)]
pub fn browser_machine(opts: &mut crate::PanelOptions) -> Result<BrowserMachine> {
    let backend = opts
        .browser_config
        .as_ref()
        .map_or_else(crate::browser::BackendKind::default, |config| config.backend);
    let private = opts.browser_private;
    let session = opts.browser_session_id.clone();
    Ok(match browser_spawn_for(opts)? {
        BrowserSpawnMode::AttachedHost(host) | BrowserSpawnMode::RestoredHost(host) => BrowserMachine::Host {
            ssh_target: host.ssh_target().to_string(),
            profile: host.profile().to_string(),
            private,
        },
        BrowserSpawnMode::Local(_) | BrowserSpawnMode::Remote(_) | BrowserSpawnMode::RestoredRemote(_) => {
            BrowserMachine::ThisComputer {
                backend,
                profile: if private { None } else { session },
                private,
            }
        }
    })
}

pub(super) fn browser_spawn_mode(
    mode: BrowserSpawnMode,
    place: Option<BrowserPlace>,
    browser_host: Option<&crate::BrowserHostConfig>,
    login: Option<SshConnection>,
) -> Result<BrowserSpawnMode> {
    let Some(browser_host) = browser_host.filter(|browser_host| browser_host.names_host()) else {
        return Ok(mode);
    };
    // The rule of `crate::browser::new_browser_panel` decided the machine of this
    // panel when the app read the entry of the settings: a page of the host there,
    // and a page of this computer otherwise. Only a plain page of this computer can
    // become a page of the host: a remote session keeps its target, and a saved
    // panel keeps the host and the profile that served it, which the caller decides
    // before this point.
    match (place, &mode) {
        (Some(BrowserPlace::Host { .. }), BrowserSpawnMode::Local(_)) => {}
        _ => return Ok(mode),
    }
    let named = browser_host
        .ssh_target()
        .or_else(|| browser_host.machine())
        .unwrap_or_default();
    let host = crate::browser::AttachedHost::with_engine(
        named,
        browser_host.profile(),
        browser_host.engine(),
        browser_host.driver_command(),
    )?;
    // A machine of the list answers to the machine key of Horizon alone, so the
    // panel takes the login that the machine record and that key make, and never
    // the SSH configuration of the person.
    let host = match browser_host.machine() {
        Some(machine) => match login {
            Some(login) => host.with_login(login),
            None => {
                return Err(crate::error::Error::Config(format!(
                    "The browser host of this computer names the machine {machine}, and this run holds no login of it. Start Horizon again, or name the SSH target of that host."
                )));
            }
        },
        None => host,
    };
    Ok(BrowserSpawnMode::AttachedHost(host))
}

/// Where a browser panel's session runs.
pub(super) enum BrowserSpawnMode {
    Local(Option<String>),
    /// A prepared remote request: allocate at the grid instead of launching.
    Remote(Box<horizon_browser::RemoteSessionRequest>),
    /// A remote panel from a previous run; it comes back stopped.
    RestoredRemote(String),
    /// The browser host of another machine that the settings name.
    AttachedHost(crate::browser::AttachedHost),
    /// The browser host and profile that served a panel in a previous run. It
    /// comes back on that host, as a page cannot move between machines.
    RestoredHost(crate::browser::AttachedHost),
}

/// Spawn a browser panel. The generic `command` field carries the optional
/// initial URL (same convention as the editor's file path); `browser_config`
/// is the active `browser` config section (honors `--config`).
pub(super) fn spawn_browser(
    mut seed: StaticPanelSeed,
    initial_url: Option<String>,
    browser_config: Option<crate::browser::BrowserConfig>,
    mode: BrowserSpawnMode,
) -> Result<Panel> {
    let initial_url = initial_url.filter(|url| !url.trim().is_empty());
    let (title, has_custom_name) = seed.take_title(|| {
        initial_url
            .as_deref()
            .map_or_else(|| "Browser".to_string(), crate::browser::panel_title_for_url)
    });
    let browser = browser_state(
        &seed.local_id,
        &browser_config.unwrap_or_default(),
        initial_url.clone(),
        seed.browser_private,
        mode,
        Ssh::default(),
    )?;
    tracing::info!("created browser panel '{}' (id={})", title, seed.id.0);

    Ok(seed.into_panel(
        title,
        PanelKind::Browser,
        PanelContent::Browser(Box::new(browser)),
        initial_url,
        None,
        has_custom_name,
    ))
}

/// The state of the browser panel of one spawn: the machine, the engine and the
/// profile that the rule of this computer answered, and the `ssh` that carries a
/// panel of a browser host.
///
/// `ssh` is the `ssh` of this computer in the production path, and the stand-in
/// program of a test, so a test walks from the options of a create to the
/// commands that the browser of that host receives.
#[doc(hidden)]
pub fn browser_panel_of_options(opts: &mut crate::PanelOptions, ssh: Ssh) -> Result<crate::browser::BrowserPanelState> {
    let browser_config = opts.browser_config.clone().unwrap_or_default();
    let private = opts.browser_private;
    let local_id = opts
        .local_id
        .clone()
        .unwrap_or_else(|| horizon_browser::DEFAULT_PROFILE.to_string());
    let initial_url = opts.command.clone();
    let mode = browser_spawn_for(opts)?;
    browser_state(&local_id, &browser_config, initial_url, private, mode, ssh)
}

fn browser_state(
    local_id: &str,
    browser_config: &crate::browser::BrowserConfig,
    initial_url: Option<String>,
    private: bool,
    mode: BrowserSpawnMode,
    ssh: Ssh,
) -> Result<crate::browser::BrowserPanelState> {
    let mut browser_config = browser_config.clone();
    Ok(match mode {
        BrowserSpawnMode::Local(session_id) => crate::browser::BrowserPanelState::start_with_session(
            local_id.to_string(),
            &browser_config,
            initial_url,
            session_id.as_deref(),
        )?,
        BrowserSpawnMode::Remote(request) => crate::browser::BrowserPanelState::start_remote(
            local_id.to_string(),
            &browser_config,
            initial_url,
            *request,
        )?,
        BrowserSpawnMode::RestoredRemote(target) => crate::browser::BrowserPanelState::restored_remote(
            local_id.to_string(),
            &browser_config,
            target,
            initial_url,
        ),
        // A panel that follows the browser host of the settings takes the engine of
        // that host, because the host serves the browser of its profile: a person
        // who names a Firefox host gets Firefox pages, whatever the backend of the
        // browser preset said. A saved panel keeps the engine that it saved, and a
        // mismatch there is a page that this computer cannot open.
        BrowserSpawnMode::AttachedHost(host) => {
            browser_config.backend = host.engine().backend();
            browser_config.profile = Some(profile_of_host(&host, private).to_string());
            browser_of_host(local_id.to_string(), &browser_config, initial_url, &host, ssh)
        }
        BrowserSpawnMode::RestoredHost(host) => {
            if let Some(reason) = crate::browser::attached_host_backend_refusal(host.engine(), browser_config.backend) {
                return Err(crate::error::Error::Config(reason.to_string()));
            }
            browser_config.profile = Some(profile_of_host(&host, private).to_string());
            browser_of_host(local_id.to_string(), &browser_config, initial_url, &host, ssh)
        }
    })
}

/// The profile that a page of `host` keeps: the profile of that host, or the
/// reserved word of a private page, whose context keeps none of it.
fn profile_of_host(host: &crate::browser::AttachedHost, private: bool) -> &str {
    if private {
        horizon_browser::PRIVATE_PROFILE
    } else {
        host.profile()
    }
}

/// The browser of a panel whose pages the browser of `host` carries.
///
/// Both calls return at once: the link to that machine opens on a worker thread
/// of the panel, and a host that never answers is a state of the panel.
fn browser_of_host(
    local_id: String,
    browser_config: &crate::browser::BrowserConfig,
    initial_url: Option<String>,
    host: &crate::browser::AttachedHost,
    ssh: Ssh,
) -> crate::browser::BrowserPanelState {
    if host.engine() == crate::config::BrowserHostEngine::Firefox {
        // The Firefox of that machine runs behind the driver of Horizon that this
        // computer starts there, so its page is a page of a driver and not one of a
        // `DevTools` port.
        crate::browser::BrowserPanelState::start_host_driver_with(ssh, local_id, browser_config, initial_url, host)
    } else {
        crate::browser::BrowserPanelState::start_attached_host_with(ssh, local_id, browser_config, initial_url, host)
    }
}
