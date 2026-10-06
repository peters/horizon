//! Bounded read-only evidence references never own resources or renew a native lease.
use crate::{Error, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;
#[derive(Default)]
pub(crate) struct Observations(Mutex<BTreeMap<Uuid, (String, Instant)>>);
impl Observations {
    pub(crate) fn retain(
        &self,
        id: Uuid,
        reference: &str,
        deadline: Instant,
        protected: &BTreeSet<Uuid>,
    ) -> Result<()> {
        let mut references = self.0.lock().map_err(|_| Error::Unavailable)?;
        references.retain(|_, (_, expiry)| *expiry > Instant::now());
        if references.len() >= 32 {
            let oldest = references
                .iter()
                .filter(|(id, _)| !protected.contains(id))
                .min_by_key(|(_, (_, expiry))| *expiry)
                .map(|(id, _)| *id)
                .ok_or(Error::Unavailable)?;
            references.remove(&oldest);
        }
        references.insert(id, (reference.to_owned(), deadline + Duration::from_secs(300)));
        Ok(())
    }
    pub(crate) fn reference(&self, id: Uuid) -> Result<String> {
        let mut references = self.0.lock().map_err(|_| Error::Unavailable)?;
        references.retain(|_, (_, expiry)| *expiry > Instant::now());
        references
            .get(&id)
            .map(|(reference, _)| reference.clone())
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
        assert_eq!(history.0.lock().unwrap().len(), 32);
    }
}
