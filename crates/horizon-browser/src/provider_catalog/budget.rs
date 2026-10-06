//! Admission for discovery jobs. Budgets outlive credential and profile
//! invalidation because a superseded job keeps running until it exits.
use super::{CatalogError, progress::Job};
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

/// Running jobs for one provider, including superseded ones: a stalled job
/// plus one replacement.
pub(super) const MAX_JOBS_PER_PROVIDER: usize = 2;
/// Starts for one provider across retries and binding changes.
pub(super) const MAX_STARTS_PER_WINDOW: usize = 4;
pub(super) const START_WINDOW: Duration = Duration::from_secs(60);
const MAX_ACTIVE_FETCHES: usize = 32;
static ACTIVE_FETCHES: AtomicUsize = AtomicUsize::new(0);

/// Process-wide cap held by a job until its thread exits.
pub(super) struct Permit;
impl Permit {
    pub(super) fn acquire() -> Option<Self> {
        ACTIVE_FETCHES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_ACTIVE_FETCHES).then_some(n + 1)
            })
            .ok()
            .map(|_| Self)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE_FETCHES.fetch_sub(1, Ordering::Release);
    }
}

#[derive(Default)]
pub(super) struct ProviderBudget {
    jobs: Vec<Job>,
    starts: VecDeque<Instant>,
}

impl ProviderBudget {
    pub(super) fn admits(&mut self, now: Instant) -> bool {
        self.jobs.retain(Job::running);
        while self
            .starts
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= START_WINDOW)
        {
            self.starts.pop_front();
        }
        self.jobs.len() < MAX_JOBS_PER_PROVIDER && self.starts.len() < MAX_STARTS_PER_WINDOW
    }

    pub(super) fn record(&mut self, job: Option<Job>, now: Instant) {
        self.jobs.extend(job);
        self.starts.push_back(now);
    }

    pub(super) fn idle(&mut self, now: Instant) -> bool {
        self.admits(now) && self.jobs.is_empty() && self.starts.is_empty()
    }

    /// Why a binding without its own attempt is still waiting for admission.
    /// None means a healthy job will release capacity shortly.
    pub(super) fn refusal(&self, now: Instant) -> Option<CatalogError> {
        if self.jobs.iter().any(|job| job.healthy(now)) {
            return None;
        }
        Some(
            self.jobs
                .iter()
                .find_map(|job| job.stall(now))
                .unwrap_or(CatalogError::Unavailable),
        )
    }
}
