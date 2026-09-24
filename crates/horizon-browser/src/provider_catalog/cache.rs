//! Bounded asynchronous discovery; creating a session never performs a catalog HTTP request.
//!
//! One provider binding (name, profile and credential generation) has at most
//! one healthy job in flight, and consumers share it instead of starting
//! duplicates. A job whose current stage exceeds its budget is reported as
//! stalled but kept, so a late result for the same binding is still accepted.
//! A replacement starts only after `RETRY` and within the provider's budget.
//! Changing the binding drops the old receivers at once, so stale rows are
//! never served.
use super::{
    CatalogDevice, CatalogError, CatalogPage, CatalogQuery,
    budget::{Permit, ProviderBudget},
    progress::{CatalogProgress, CatalogStage, Job},
    target_provider,
};
use crate::remote::RemoteProviderProfile;
use std::{
    collections::BTreeMap,
    sync::mpsc::{Receiver, TryRecvError, channel},
    time::{Duration, Instant},
};

const FRESH: Duration = Duration::from_secs(300);
const RETRY: Duration = Duration::from_secs(15);
const MAX_ENTRIES: usize = 32;

type Outcome = Result<Vec<CatalogDevice>, CatalogError>;

struct Attempt {
    job: Job,
    receiver: Receiver<Outcome>,
    started: Instant,
}

struct Entry {
    profile: RemoteProviderProfile,
    attempts: Vec<Attempt>,
    /// The latest completed outcome and the start of the attempt that produced it.
    result: Option<(Outcome, Instant)>,
    last_start: Option<Instant>,
}

impl Entry {
    fn new(profile: &RemoteProviderProfile) -> Self {
        Self {
            profile: profile.clone(),
            attempts: Vec::new(),
            result: None,
            last_start: None,
        }
    }

    fn fresh_rows(&self, now: Instant) -> Option<&[CatalogDevice]> {
        match &self.result {
            Some((Ok(rows), started)) if now.saturating_duration_since(*started) < FRESH => Some(rows),
            _ => None,
        }
    }

    /// A job within its stage budget, or one that finished and awaits `poll`.
    fn in_flight(&self, now: Instant) -> bool {
        self.attempts.iter().any(|attempt| attempt.job.stall(now).is_none())
    }

    fn wants_start(&self, now: Instant) -> bool {
        self.fresh_rows(now).is_none()
            && !self.in_flight(now)
            && self
                .last_start
                .is_none_or(|at| now.saturating_duration_since(at) >= RETRY)
    }

    fn poll(&mut self, now: Instant) {
        let mut completed = Vec::new();
        self.attempts.retain(|attempt| match attempt.receiver.try_recv() {
            Err(TryRecvError::Empty) => true,
            Ok(outcome) => {
                completed.push((outcome, attempt.started));
                false
            }
            Err(TryRecvError::Disconnected) => {
                completed.push((Err(CatalogError::Unavailable), attempt.started));
                false
            }
        });
        for (outcome, started) in completed {
            // Rows from any attempt of this binding beat a failure; between two
            // answers of the same kind the newer attempt wins. A failure never
            // replaces rows that are still fresh.
            let replace = match (&outcome, &self.result) {
                (_, None) | (Ok(_), Some((Err(_), _))) => true,
                (Ok(_), Some((Ok(_), at))) => started >= *at,
                (Err(_), Some((_, at))) => started >= *at && self.fresh_rows(now).is_none(),
            };
            if replace {
                self.result = Some((outcome, started));
            }
        }
    }

    /// The newest known failure: a completed error, expired rows or a stalled stage.
    fn failure(&self, now: Instant) -> Option<CatalogError> {
        let completed = match &self.result {
            Some((Err(error), at)) => Some((*at, *error)),
            Some((Ok(_), at)) => Some((*at, CatalogError::RefreshRequired)),
            None => None,
        };
        let stalled = self
            .attempts
            .iter()
            .rev()
            .find_map(|attempt| attempt.job.stall(now).map(|error| (attempt.started, error)));
        stalled
            .into_iter()
            .chain(completed)
            .max_by_key(|(at, _)| *at)
            .map(|(_, error)| error)
    }
}

#[derive(Default)]
pub struct CatalogCache {
    credential_generation: u64,
    entries: BTreeMap<String, Entry>,
    budgets: BTreeMap<String, ProviderBudget>,
    skew: Duration,
}

impl CatalogCache {
    /// Discard completed and pending rows after an in-process credential change.
    pub fn invalidate_credentials(&mut self, generation: u64) {
        if self.credential_generation != generation {
            self.clear();
            self.credential_generation = generation;
        }
    }

    /// Drop every binding and late result. Jobs that are still running keep
    /// counting against their provider's budget until they exit.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Returns true when `start` would launch a job now. A changed binding
    /// replaces the old one at once; a healthy job for the same binding is
    /// never duplicated.
    pub fn needs_refresh(&mut self, name: &str, profile: &RemoteProviderProfile) -> bool {
        self.poll();
        let now = self.now();
        self.admits(name, profile, now)
    }

    pub fn start(
        &mut self,
        name: &str,
        profile: &RemoteProviderProfile,
        fetch: impl FnOnce(&CatalogProgress) -> Outcome + Send + 'static,
    ) {
        self.poll();
        let now = self.now();
        if !self.admits(name, profile, now) {
            return;
        }
        let Some(permit) = Permit::acquire() else { return };
        let (Some(entry), Some(budget)) = (self.entries.get_mut(name), self.budgets.get_mut(name)) else {
            return;
        };
        let (job, progress) = Job::start(self.skew);
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("provider-catalog".into())
            .spawn(move || {
                let _permit = permit;
                let outcome = fetch(&progress);
                // Finish before answering, so a received answer never finds its job still running.
                drop(progress);
                let _ = tx.send(outcome);
            });
        entry.last_start = Some(now);
        if spawned.is_ok() {
            budget.record(Some(job.clone()), now);
            entry.attempts.push(Attempt {
                job,
                receiver: rx,
                started: now,
            });
        } else {
            budget.record(None, now);
            entry.result = Some((Err(CatalogError::Unavailable), now));
        }
    }

    /// Records the binding, then decides whether it may start a job now.
    fn admits(&mut self, name: &str, profile: &RemoteProviderProfile, now: Instant) -> bool {
        if self.entries.get(name).is_some_and(|entry| entry.profile != *profile) {
            self.entries.remove(name);
        }
        if !self.make_room(name, now) {
            return false;
        }
        let entry = self.entries.entry(name.into()).or_insert_with(|| Entry::new(profile));
        entry.wants_start(now) && self.budgets.entry(name.into()).or_default().admits(now)
    }

    /// Completed entries can be evicted; running jobs stay bounded by their budgets.
    fn make_room(&mut self, name: &str, now: Instant) -> bool {
        self.budgets
            .retain(|provider, budget| provider == name || !budget.idle(now));
        if self.entries.len() < MAX_ENTRIES || self.entries.contains_key(name) {
            return true;
        }
        let idle = self
            .entries
            .iter()
            .find(|(_, entry)| entry.attempts.is_empty())
            .map(|(name, _)| name.clone());
        idle.is_some_and(|name| self.entries.remove(&name).is_some())
    }

    pub fn poll(&mut self) {
        let now = self.now();
        for entry in self.entries.values_mut() {
            entry.poll(now);
        }
    }

    /// The stage of the newest running job for this provider, for diagnostics.
    #[must_use]
    pub fn stage(&self, name: &str) -> Option<CatalogStage> {
        self.entries
            .get(name)?
            .attempts
            .iter()
            .rev()
            .find(|attempt| attempt.job.running())
            .map(|attempt| attempt.job.stage())
    }

    /// # Errors
    /// A failed fetch or a changed/expired provider binding. None means discovery is still running.
    pub fn page(
        &self,
        profile: &RemoteProviderProfile,
        query: &CatalogQuery,
    ) -> Result<Option<CatalogPage>, CatalogError> {
        self.rows(profile, &query.provider)
            .map(|rows| rows.map(|rows| CatalogPage::select(rows, query)))
    }

    /// # Errors
    /// Undiscovered, expired or unavailable combinations must be discovered again, never guessed.
    pub fn target(&self, profile: &RemoteProviderProfile, target: &str) -> Result<&CatalogDevice, CatalogError> {
        let provider = target_provider(target).ok_or(CatalogError::RefreshRequired)?;
        self.rows(profile, provider)?
            .and_then(|rows| rows.iter().find(|row| row.target == target))
            .ok_or(CatalogError::RefreshRequired)
    }

    fn rows(&self, profile: &RemoteProviderProfile, provider: &str) -> Result<Option<&[CatalogDevice]>, CatalogError> {
        let now = self.now();
        let entry = self
            .entries
            .get(provider)
            .filter(|entry| entry.profile == *profile)
            .ok_or(CatalogError::RefreshRequired)?;
        if let Some(rows) = entry.fresh_rows(now) {
            return Ok(Some(rows));
        }
        if entry.in_flight(now) {
            return Ok(None);
        }
        if let Some(error) = entry.failure(now) {
            return Err(error);
        }
        // This binding is waiting for admission behind jobs from an earlier one.
        match self.budgets.get(provider) {
            Some(budget) => budget.refusal(now).map_or(Ok(None), Err),
            None => Err(CatalogError::Unavailable),
        }
    }

    fn now(&self) -> Instant {
        Instant::now() + self.skew
    }

    /// Moves this cache's clock forward so budgets can be tested without sleeping.
    #[doc(hidden)]
    pub fn advance_clock(&mut self, by: Duration) {
        self.skew += by;
    }
}

#[cfg(test)]
mod tests;
