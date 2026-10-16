//! Proofs built for tests, without a worker: one whose build already failed, and one
//! that waits for a build slot.
use super::{Cancellation, Phase, PhaseState, Pinned, Proof};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

impl Proof {
    /// A proof whose build failed on an account problem, for tests of the fix.
    pub(in crate::app::cloud_panel::production::repository_setup) fn failed_on(fix: super::super::access::Fix) -> Self {
        let (_, receiver) = mpsc::channel();
        let now = Instant::now();
        let failed = PhaseState {
            started: Some(now),
            ended: Some(now),
            failure: Some("exit 1 after 1 s".to_owned()),
        };
        Self {
            receiver,
            ran: "CI jobs",
            cancel: Cancellation::default(),
            log: VecDeque::new(),
            phases: [
                (Phase::Build, failed),
                (Phase::Ci, PhaseState::default()),
                (Phase::Fast, PhaseState::default()),
            ],
            log_path: PathBuf::new(),
            image: String::new(),
            volume: String::new(),
            unreported: false,
            container: String::new(),
            inputs: None,
            host_restore: false,
            host_crates: Vec::new(),
            gpu: false,
            worker: None,
            pinned: Pinned::default(),
            fix: Some(fix),
            waiting: Vec::new(),
        }
    }

    /// A proof that waits for a build slot that the setups of `holders` hold.
    pub(in crate::app::cloud_panel::production::repository_setup) fn waiting_on(holders: &[&str]) -> Self {
        let mut proof = Self::failed_on(super::super::access::Fix::GrantOrganization {
            organization: String::new(),
        });
        proof.phases[0].1 = PhaseState::default();
        proof.fix = None;
        proof.waiting = holders.iter().map(|holder| (*holder).to_owned()).collect();
        proof
    }
}
