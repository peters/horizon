//! The proof steps that use this computer for the setup: the pin check, and the Rust
//! git dependencies cloned with this computer's Git login and the volume they leave.
use super::{Job, crates};
use std::process::Command;

impl Job {
    /// The proof's one snapshot of the setup. Horizon's pin verifier is written first,
    /// so the snapshot holds it as Use commits it.
    pub(super) fn read_setup(&self) -> Result<super::super::snapshot::Setup, String> {
        let pins = super::super::pins::write_verifier(&self.repository, self.identity);
        super::super::snapshot::Setup::read(&self.repository, self.identity).and_then(|setup| pins.map(|()| setup))
    }

    /// The CI phase before the CI runs: a volume no clone outlives, the checkout, the plan
    /// check, this computer's packages and the allowed Rust git dependencies.
    pub(super) fn prepare_ci(&self, log: &mut Option<std::fs::File>) -> Result<(), String> {
        self.forget_volume(log)?;
        self.prepare(log)?;
        self.check_plan()?;
        self.warm_packages(log)?;
        self.clone_crates(log);
        Ok(())
    }

    /// The CI container reads the clones read-only in place of their GitHub addresses.
    pub(super) fn mount_crates(&self, command: &mut Command) {
        if let Some(mirrors) = self.mirrors.borrow().as_ref() {
            command.args(mirrors.docker_args());
        }
    }

    /// Checks the snapshot's toolchain pins first: a toolchain the repository moved fails
    /// in seconds. A note when this computer cannot run the check.
    pub(super) fn check_pins(
        &self,
        setup: &super::super::snapshot::Setup,
        log: &mut Option<std::fs::File>,
    ) -> Result<(), String> {
        let mut notes = Vec::new();
        let checked = super::super::pins::run(
            (&self.repository, setup),
            |check, limit| self.stream_within(check, log, Some(limit)),
            |note| notes.push(note),
        );
        for note in notes {
            self.record(log, note);
        }
        checked
    }

    /// Removes the proof volume first when an earlier proof cloned a repository the box
    /// no longer allows.
    pub(super) fn forget_volume(&self, log: &mut Option<std::fs::File>) -> Result<(), String> {
        // The proof's own pinned Docker daemon, never another.
        self.host_crates.forget((&self.volume, self.docker()), |remove, limit| {
            self.stream_within(remove, log, Some(limit))
        })
    }

    /// Clones the allowed Rust git dependencies for both CI runs, with a note for each
    /// repository left out.
    pub(super) fn clone_crates(&self, log: &mut Option<std::fs::File>) {
        let mut notes = Vec::new();
        *self.mirrors.borrow_mut() = crates::Mirrors::for_proof(
            (&self.repository, self.head(), &self.host_crates, &self.cancel),
            // Captured, never streamed: with this computer's sign-in settings Git's output
            // can carry a credential, so only a fixed reason reaches the log.
            |clone, limit| {
                self.capture(clone, limit).map(|_| ()).map_err(|error| {
                    if error == "stopped" || error.starts_with("still running") {
                        error
                    } else {
                        "git could not clone it with this computer's Git login".to_owned()
                    }
                })
            },
            |note| notes.push(note),
        );
        for note in notes {
            self.record(log, note);
        }
    }
}
