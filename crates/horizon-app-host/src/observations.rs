//! Bounded read-only evidence references never own resources or renew a native lease.
use crate::{Error, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;
const HISTORY_LIMIT: usize = 32;
// A validated run executes at most 1024 steps across at most 32 initial devices.
const RUN_LIMIT: usize = HISTORY_LIMIT + 1024 + 32;
#[derive(Default)]
pub(crate) struct Observations(Mutex<History>);
#[derive(Default)]
struct History {
    references: BTreeMap<Uuid, Reference>,
    protected: BTreeSet<Uuid>,
    running: bool,
}
struct Reference {
    value: String,
    expiry: Instant,
    pinned: bool,
}
impl History {
    fn prune(&mut self) {
        self.references
            .retain(|_, reference| reference.pinned || reference.expiry > Instant::now());
    }
    fn evict(&mut self) -> Result<()> {
        let oldest = self
            .references
            .iter()
            .filter(|(id, reference)| !reference.pinned && !self.protected.contains(id))
            .min_by_key(|(_, reference)| reference.expiry)
            .map(|(id, _)| *id)
            .ok_or(Error::Unavailable)?;
        self.references.remove(&oldest);
        Ok(())
    }
}
// The actor's exclusive run lock outlives this scope; interactive allocations cannot join it.
pub(crate) struct Run<'a>(&'a Observations);
impl Drop for Run<'_> {
    fn drop(&mut self) {
        let mut history = self.0.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        history.running = false;
        for reference in history.references.values_mut() {
            reference.pinned = false;
        }
        history.prune();
        while history.references.len() > HISTORY_LIMIT {
            if history.evict().is_err() {
                break;
            }
        }
    }
}
impl Observations {
    pub(crate) fn begin_run(&self) -> Result<Run<'_>> {
        let mut history = self.0.lock().map_err(|_| Error::Unavailable)?;
        if history.running {
            return Err(Error::Unavailable);
        }
        history.running = true;
        Ok(Run(self))
    }
    pub(crate) fn retain(
        &self,
        id: Uuid,
        reference: &str,
        deadline: Instant,
        protected: &BTreeSet<Uuid>,
    ) -> Result<()> {
        let mut history = self.0.lock().map_err(|_| Error::Unavailable)?;
        history.protected.clone_from(protected);
        history.prune();
        let limit = if history.running { RUN_LIMIT } else { HISTORY_LIMIT };
        if !history.references.contains_key(&id) && history.references.len() >= limit {
            history.evict()?;
        }
        let pinned = history.running;
        history.references.insert(
            id,
            Reference {
                value: reference.to_owned(),
                expiry: deadline + Duration::from_secs(300),
                pinned,
            },
        );
        Ok(())
    }
    pub(crate) fn reference(&self, id: Uuid) -> Result<String> {
        let mut history = self.0.lock().map_err(|_| Error::Unavailable)?;
        history.prune();
        history
            .references
            .get(&id)
            .map(|reference| reference.value.clone())
            .ok_or(Error::SessionUnknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_history_cannot_evict_a_shorter_lived_active_lane() {
        let history = Observations::default();
        let live = Uuid::new_v4();
        let protected = BTreeSet::from([live]);
        history
            .retain(
                live,
                "private-live-id",
                Instant::now() + Duration::from_secs(10),
                &protected,
            )
            .unwrap();
        for _ in 0..64 {
            history
                .retain(
                    Uuid::new_v4(),
                    "private-completed-id",
                    Instant::now() + Duration::from_secs(30),
                    &protected,
                )
                .unwrap();
        }
        assert_eq!(history.reference(live).unwrap(), "private-live-id");
        assert_eq!(history.0.lock().unwrap().references.len(), HISTORY_LIMIT);
    }
    #[test]
    fn run_history_is_bounded_and_unpins_on_panic_without_evicting_an_active_lane() {
        let history = Observations::default();
        let live = Uuid::new_v4();
        let protected = BTreeSet::from([live]);
        history
            .retain(live, "private-live-id", Instant::now(), &protected)
            .unwrap();
        let result = std::panic::catch_unwind(|| {
            let _run = history.begin_run().unwrap();
            for _ in 0..RUN_LIMIT - 1 {
                history
                    .retain(Uuid::new_v4(), "private-run-id", Instant::now(), &protected)
                    .unwrap();
            }
            assert_eq!(history.0.lock().unwrap().references.len(), RUN_LIMIT);
            // No unpinned completed history is available to make room.
            assert_eq!(
                history.retain(Uuid::new_v4(), "overflow", Instant::now(), &protected),
                Err(Error::Unavailable)
            );
            panic!("synthetic run failure");
        });
        assert!(result.is_err());
        assert_eq!(history.0.lock().unwrap().references.len(), HISTORY_LIMIT);
        assert_eq!(history.reference(live).unwrap(), "private-live-id");
        assert!(!history.0.lock().unwrap().running);
    }
    #[test]
    fn expired_pinned_evidence_survives_only_until_run_scope_ends() {
        let history = Observations::default();
        let id = Uuid::new_v4();
        let run = history.begin_run().unwrap();
        history
            .retain(
                id,
                "private-expired-id",
                Instant::now() - Duration::from_secs(301),
                &BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(history.reference(id).unwrap(), "private-expired-id");
        drop(run);
        assert_eq!(history.reference(id), Err(Error::SessionUnknown));
    }
}
