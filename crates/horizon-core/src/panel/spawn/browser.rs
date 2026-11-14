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

use crate::browser::BrowserPlace;
use crate::error::Result;
use crate::ssh::SshConnection;

use super::BrowserSpawnMode;

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
