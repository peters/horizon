//! The run's side of the private Rust git dependencies: what the box allows, and what
//! an earlier proof of the run may have left in the proof volume.
use super::Run;
use crate::app::cloud_panel::production::repository_setup::proof::{Phase, Proof, crates::Access};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// The run's private Rust git dependencies.
#[derive(Default)]
pub(in crate::app::cloud_panel::production::repository_setup) struct Crates {
    /// The person allowed cloning them on this computer.
    pub(in crate::app::cloud_panel::production::repository_setup) allowed: bool,
    /// The GitHub repositories the committed Cargo lock files pin, read when the run
    /// started: the box names them, and only they are cloned.
    pub(in crate::app::cloud_panel::production::repository_setup) locked: Vec<String>,
    /// The repositories the proof volume may hold copies of, and the access of the last
    /// proof, which the next one settles once it knows how far that proof got.
    in_volume: Vec<String>,
    last_access: Option<Access>,
    /// The box as the running proof sees it, so clearing it stops clones not yet begun.
    pub(in crate::app::cloud_panel::production::repository_setup) live: Arc<AtomicBool>,
}

impl Run {
    /// The proof had the access the boxes give now: the same packages and repositories
    /// of this computer.
    pub(super) fn same_access(&self, proof: &Proof) -> bool {
        proof.host_restore == self.host_restore && proof.host_crates == self.crates_now()
    }

    /// The Rust git repositories the box allows to clone now.
    pub(super) fn crates_now(&self) -> Vec<String> {
        if self.crates.allowed {
            self.crates.locked.clone()
        } else {
            Vec::new()
        }
    }

    /// The Rust git repositories the person allows this computer to clone for the next
    /// proof, and whether its volume must go first because an earlier proof of the run
    /// cloned one that is no longer allowed. Call it before the proof it is for replaces
    /// the last one, which it settles.
    pub(in crate::app::cloud_panel::production::repository_setup) fn crate_access(&mut self) -> Access {
        if let Some(last) = self.crates.last_access.take() {
            let ci = self.proof.as_ref().map(|proof| proof.phase(Phase::Ci));
            // Its phases show only the events the dialog drained, so the CI phase may have
            // begun copying unseen: always count it as started.
            let started = true;
            let passed = ci.is_some_and(|ci| ci.ended.is_some() && ci.failure.is_none());
            last.settle(&mut self.crates.in_volume, (started, passed));
        }
        let repositories = self.crates_now();
        let forget_volume = self.crates.in_volume.iter().any(|held| !repositories.contains(held));
        self.crates
            .live
            .store(self.crates.allowed, std::sync::atomic::Ordering::Relaxed);
        let access = Access {
            repositories,
            forget_volume,
            allowed: Arc::clone(&self.crates.live),
        };
        self.crates.last_access = Some(access.clone());
        access
    }
}
