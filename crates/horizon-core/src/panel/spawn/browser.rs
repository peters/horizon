//! Which machine serves the browser panels of a new panel, and where a saved
//! page comes back.
//!
//! A panel that follows the entry `browser_host` is attached to that host, and a
//! panel of a saved run comes back on the host and profile that served it. Both
//! modes are decided here, so the panel spawn of `spawn.rs` reads one answer, and
//! a machine of the machine list takes the login of that machine and never the
//! SSH configuration of the person.

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
pub(super) fn browser_spawn_mode(
    mode: BrowserSpawnMode,
    browser_host: Option<&crate::BrowserHostConfig>,
    follows_browser_host: bool,
    login: Option<SshConnection>,
) -> Result<BrowserSpawnMode> {
    if !follows_browser_host {
        return Ok(mode);
    }
    let Some(browser_host) = browser_host.filter(|browser_host| browser_host.names_host()) else {
        return Ok(mode);
    };
    if !matches!(mode, BrowserSpawnMode::Local(None)) {
        return Ok(mode);
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
