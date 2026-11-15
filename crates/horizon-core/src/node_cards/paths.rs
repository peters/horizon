//! The path of the card socket of one session, on the machine and here.
//!
//! Both paths end in `<machine>/<session>.sock` and both names pass the rules of
//! [`block_machines`](horizon_browser_control::manifest::block_machines), so no
//! path leaves its folder. The folder here is under the temporary folder of this
//! computer, inside the 104 bytes of a Unix socket path on macOS.

use std::path::{Path, PathBuf};

use super::{check_pair, check_socket_path};

/// The folder on a machine that holds one folder of sockets for each machine.
pub(crate) const REMOTE_DIR: &str = "/tmp/horizon-cards";

/// The prefix of the actor of one session of one machine. The machine states no
/// identity: this computer builds it from the two names of the session.
const ACTOR_PREFIX: &str = "horizon:machine/";

/// Longest socket path of this build: 104 bytes on macOS and 108 on Linux.
pub(crate) const MAX_SOCKET_PATH_BYTES: usize = 100;

/// The path on `machine` of the socket of `session`.
///
/// # Errors
/// Names the rule that a name broke.
pub fn remote_socket(machine: &str, session: &str) -> Result<String, super::MachineRefusal> {
    let (machine, session) = check_pair(machine, session)?;
    Ok(format!("{REMOTE_DIR}/{machine}/{session}.sock"))
}

/// The folder here of the sockets of this process, which carries its id so two
/// Horizon processes never share one path.
#[must_use]
pub fn local_dir(base: &Path) -> PathBuf {
    base.join(format!("horizon-cards-{}", std::process::id()))
}

/// The path here of the socket of `session` of `machine`.
///
/// # Errors
/// Names the rule that a name broke, or a path above the socket bound.
pub fn local_socket(dir: &Path, machine: &str, session: &str) -> Result<PathBuf, super::MachineRefusal> {
    let (machine, session) = check_pair(machine, session)?;
    let path = dir.join(machine).join(format!("{session}.sock"));
    let text = path.to_string_lossy();
    check_socket_path(text.as_ref())?;
    Ok(path)
}

/// The identity of one session of one machine, as the actor of its cards.
///
/// # Errors
/// Names the rule that a name broke.
pub fn actor(machine: &str, session: &str) -> Result<String, super::MachineRefusal> {
    let (machine, session) = check_pair(machine, session)?;
    Ok(format!("{ACTOR_PREFIX}{machine}/{session}"))
}

/// The machine and the session of an actor of a machine session, or `None` for
/// an actor of a local panel.
#[must_use]
pub fn actor_parts(actor: &str) -> Option<(&str, &str)> {
    let rest = actor.strip_prefix(ACTOR_PREFIX)?;
    let (machine, session) = rest.split_once('/')?;
    check_pair(machine, session).ok()
}
