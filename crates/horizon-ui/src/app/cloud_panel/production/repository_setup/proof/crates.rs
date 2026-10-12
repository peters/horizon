//! Rust git dependencies from private repositories, such as a crate the repository pins
//! by `rev` from another private GitHub repository. The proof container holds no
//! credential, so this computer clones them with its own Git login, and the container
//! reads each clone read-only in place of the GitHub address.
//!
//! The setup agent can write the repository, so nothing here runs Cargo, rustup or the
//! repository's configuration on this computer: the lock files are read from the
//! committed tree, only `github.com` addresses are cloned, with plain `git clone
//! --mirror` into a new private folder, and only the repositories the person approved
//! by name when they checked the box.
use std::process::Command;
use std::time::Duration;

mod locks;
mod mirrors;
#[cfg(test)]
mod tests;
mod watch;

pub(in crate::app::cloud_panel::production::repository_setup) use locks::locked_repositories;
pub(super) use mirrors::Mirrors;

/// What a proof may clone, and whether its volume must go first: a repository cloned
/// for an earlier proof of the run that is no longer approved would otherwise stay in
/// the Cargo cache on the shared volume.
#[derive(Clone, Debug)]
pub(in crate::app::cloud_panel::production::repository_setup) struct Access {
    pub(in crate::app::cloud_panel::production::repository_setup) repositories: Vec<String>,
    pub(in crate::app::cloud_panel::production::repository_setup) forget_volume: bool,
    /// The box as it is now: clearing it while the proof builds stops the clones that
    /// have not started.
    pub(in crate::app::cloud_panel::production::repository_setup) allowed:
        std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Access {
    /// What the volume may hold after the proof that had this access: `in_volume`
    /// before it, updated by how far its CI phase got. Only a CI phase that passed
    /// proves the volume was removed and filled again; until then every repository a
    /// started CI phase could have copied stays counted, so the removal is asked again.
    pub(in crate::app::cloud_panel::production::repository_setup) fn settle(
        self,
        in_volume: &mut Vec<String>,
        (started, passed): (bool, bool),
    ) {
        if self.forget_volume && passed {
            *in_volume = self.repositories;
        } else if started {
            for repository in self.repositories {
                if !in_volume.contains(&repository) {
                    in_volume.push(repository);
                }
            }
        }
    }

    /// Removes the proof volume through `stream` when the approval shrank, so caches,
    /// the private clones' Cargo copies among them, start empty.
    pub(super) fn forget(
        &self,
        (volume, mut docker): (&str, Command),
        stream: impl FnOnce(Command, Duration) -> Result<(), String>,
    ) -> Result<(), String> {
        if !self.forget_volume {
            return Ok(());
        }
        docker.args(["volume", "rm", "--force", volume]);
        stream(docker, Duration::from_mins(2))
    }
}
