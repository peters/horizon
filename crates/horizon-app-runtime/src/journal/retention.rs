//! Account-wide admission retention for confirmed, resource-free history only.
use super::{Ledger, MAX_OPERATIONS, Phase};

pub(super) fn make_room(
    store: &super::store::Store,
    ledger: &mut Ledger,
) -> Option<super::store::execution::RetentionLease> {
    if ledger.records.len() < MAX_OPERATIONS {
        return None;
    }
    let mut eligible = ledger
        .records
        .iter()
        .filter(|(_, record)| record.phase == Phase::Complete && record.slot.is_none() && record.resources.is_empty())
        .map(|(id, record)| (record.created, *id, record.owner))
        .collect::<Vec<_>>();
    eligible.sort_unstable();
    for (_, id, owner) in eligible {
        if let Ok(Some(lease)) = store.inactive_owner(owner) {
            ledger.records.remove(&id);
            return Some(lease);
        }
    }
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::Error;
    use crate::journal::{
        Kind, Record, Resource, Slot,
        tests::{canonical_temp, journal},
    };
    use std::time::Duration;
    use uuid::Uuid;

    fn history(root: &std::path::Path, phase: Phase) -> Ledger {
        let records = (0..MAX_OPERATIONS)
            .map(|index| {
                (
                    Uuid::new_v4(),
                    Record {
                        owner: Uuid::from_u128((index / 64 + 1) as u128),
                        realm: if index % 2 == 0 { "a" } else { "b" }.repeat(64),
                        root: root.to_owned(),
                        kind: Kind::Run,
                        phase,
                        created: index as u64,
                        deadline: index as u64 + 30,
                        slot: None,
                        resources: Vec::new(),
                    },
                )
            })
            .collect();
        Ledger { version: 1, records }
    }

    #[test]
    fn inactive_owners_and_old_realms_do_not_exhaust_shared_admission() {
        let root = canonical_temp();
        let state = journal(&root.path().join("state"), 'c');
        let mut ledger = history(root.path(), Phase::Complete);
        let oldest = *ledger.records.iter().min_by_key(|(_, r)| r.created).unwrap().0;
        let protected = Uuid::new_v4();
        let mut pending = ledger.records.remove(&oldest).unwrap();
        pending.phase = Phase::Uncertain;
        ledger.records.insert(protected, pending);
        let next_oldest = *ledger
            .records
            .iter()
            .filter(|(_, r)| r.phase == Phase::Complete)
            .min_by_key(|(_, r)| r.created)
            .unwrap()
            .0;
        state
            .edit(|current| {
                current.records = ledger.records;
                Ok(())
            })
            .unwrap();
        let fresh = state
            .start(Uuid::new_v4(), root.path(), Kind::Session, Duration::from_secs(30))
            .unwrap();
        state
            .store
            .access(false, |current| {
                assert_eq!(current.records.len(), MAX_OPERATIONS);
                assert!(current.records.contains_key(&protected));
                assert!(current.records.contains_key(&fresh.id));
                assert!(!current.records.contains_key(&next_oldest));
                assert!(current.records.values().any(|r| r.realm == "b".repeat(64)));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn full_unfinished_ledger_still_refuses_admission_without_retiring_any_record() {
        let root = canonical_temp();
        let state = journal(&root.path().join("state"), 'a');
        let ledger = history(root.path(), Phase::Uncertain);
        let ids = ledger.records.keys().copied().collect::<Vec<_>>();
        state
            .edit(|current| {
                current.records = ledger.records;
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            state.start(Uuid::new_v4(), root.path(), Kind::Run, Duration::from_secs(30)),
            Err(Error::JournalUnavailable)
        ));
        state
            .store
            .access(false, |current| {
                assert_eq!(current.records.keys().copied().collect::<Vec<_>>(), ids);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn retention_never_drops_active_resources_or_reserved_capacity() {
        let root = canonical_temp();
        let mut ledger = history(root.path(), Phase::Complete);
        for (index, record) in ledger.records.values_mut().take(3).enumerate() {
            record.created = 0;
            record.kind = Kind::Session;
            record.phase = Phase::Uncertain;
            record.slot = Some(if index == 0 { Slot::Pending } else { Slot::Allocated });
            record.resources = vec![if index == 0 {
                Resource::AllocationIntent {}
            } else {
                Resource::Session {
                    reference: format!("synthetic-session-{index:016}"),
                }
            }];
        }
        let protected = ledger
            .records
            .iter()
            .filter(|(_, r)| r.phase != Phase::Complete)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        let state = journal(&root.path().join("state"), 'a');
        let _lease = make_room(&state.store, &mut ledger);
        assert_eq!(ledger.records.len(), MAX_OPERATIONS - 1);
        assert!(protected.iter().all(|id| ledger.records.contains_key(id)));
    }

    #[test]
    fn active_owner_history_is_protected_until_its_execution_lease_is_released() {
        use crate::journal::execution::Workspace;
        use std::sync::Arc;
        let root = canonical_temp();
        let state = Arc::new(journal(&root.path().join("state"), 'a'));
        let mut ledger = history(root.path(), Phase::Complete);
        let oldest_owner = ledger.records.iter().min_by_key(|(_, r)| r.created).unwrap().1.owner;
        let active = Workspace::open(Arc::clone(&state), oldest_owner, root.path()).unwrap();
        let protected = ledger
            .records
            .iter()
            .filter(|(_, r)| r.owner == oldest_owner)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        let lease = make_room(&state.store, &mut ledger);
        assert!(lease.is_some());
        assert!(protected.iter().all(|id| ledger.records.contains_key(id)));
        drop(lease);
        drop(active);
        let lease = make_room(&state.store, &mut ledger);
        assert!(lease.is_none()); // Already below the cap: do not prune additional history.
        let extra = Uuid::new_v4();
        let record = Record {
            owner: Uuid::new_v4(),
            realm: "a".repeat(64),
            root: root.path().to_owned(),
            kind: Kind::Run,
            phase: Phase::Complete,
            created: 9999,
            deadline: 9999,
            slot: None,
            resources: Vec::new(),
        };
        ledger.records.insert(extra, record);
        let _lease = make_room(&state.store, &mut ledger);
        assert_eq!(
            protected.iter().filter(|id| ledger.records.contains_key(id)).count(),
            protected.len() - 1
        );
    }
}
