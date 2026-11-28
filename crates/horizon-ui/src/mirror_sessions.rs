//! The mode `horizon --mirror-sessions`: the publisher of the session mirror.
//!
//! This mode is the computer that has the sessions. It reads the session stores of
//! the agent programs of this computer and sends what it reads to the server of its
//! own home, so the other computers of that server can show the sessions in the
//! conversation dock. It opens no window, it reads no setting of Horizon and it
//! installs no agent integration: it comes before the Horizon home of the person, as
//! the three modes of the server do.
//!
//! Two starts, and no other:
//!
//! - `horizon --mirror-sessions --join <link>` joins the server with the code of one
//!   invitation, keeps the token and the address in its own home, prints one line and
//!   ends.
//! - `horizon --mirror-sessions` publishes the sessions of this computer until the
//!   person ends the process with SIGINT or SIGTERM.
//!
//! The home is `HORIZON_MIRROR_HOME` or `~/.horizon-mirror`, at mode 0700 with files
//! at 0600. This mode writes nothing outside that home: it writes no file of an agent
//! program, no `~/.horizon` and no log.
//!
//! `docs/architecture/session-mirror.md` gives the design, and
//! `docs/review/trust-boundaries/session-mirror.md` states what it guarantees.

mod home;
mod publisher;
#[cfg(test)]
mod tests;

use home::MirrorHome;

pub use publisher::{address_of, device_name, host_name};

/// One start of the mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorMode {
    /// The link of an invitation, when this start joins the server.
    pub join: Option<String>,
}

impl MirrorMode {
    /// The mode of one `--join` value, or of no join.
    #[must_use]
    pub fn with_join(join: Option<String>) -> Self {
        Self { join }
    }
}

/// Runs the mode and answers the exit code of this start: zero for a stop or for a
/// join, and two for a home or a server that this start cannot use.
pub fn run(mode: &MirrorMode) -> i32 {
    let home = match MirrorHome::resolve() {
        Ok(home) => home,
        Err(problem) => return fail(&problem),
    };
    if let Err(problem) = home.prepare() {
        return fail(&problem);
    }
    run_in(&home, mode)
}

/// Runs the mode with the home that it reads and writes, which a test names.
pub(crate) fn run_in(home: &MirrorHome, mode: &MirrorMode) -> i32 {
    match mode.join.as_deref() {
        Some(link) => join(home, link),
        None => publisher::run(home),
    }
}

/// Joins the server of `link` and keeps the token and the address in `home`.
fn join(home: &MirrorHome, link: &str) -> i32 {
    let Some(address) = address_of(link) else {
        return fail("that text is not the link of a join: copy the link that a computer of the server showed.");
    };
    let device = device_name(&host_name());
    let secrets = match home.secrets() {
        Ok(secrets) => secrets,
        Err(problem) => return fail(&problem),
    };
    match horizon_sync::join(link, &device, &secrets) {
        Ok(joined) => {
            if let Err(problem) = home
                .write_device(&joined.device)
                .and_then(|()| home.write_address(&address))
            {
                return fail(&problem);
            }
            println!(
                "horizon mirror: this computer joined the server at {address} as {}.",
                joined.device
            );
            0
        }
        Err(error) => fail(&error.sentence()),
    }
}

/// One line on the standard error, and the code of a start that cannot continue.
fn fail(problem: &str) -> i32 {
    eprintln!("error: {problem}");
    2
}
