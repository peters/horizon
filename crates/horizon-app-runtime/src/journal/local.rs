//! Durable binding of private foreground process guardians to owned native operations.
use super::{Journal, Kind, Phase, Resource};
use crate::{Error, Result};
use uuid::Uuid;

impl Journal {
    /// # Errors
    /// Publish uncertainty before spawning a private guardian. The host chooses its state directory
    /// from this operation ID; project configuration and tools never choose process identities.
    pub fn local_intent(&self, owner: Uuid, id: Uuid) -> Result<()> {
        self.edit(|ledger| {
            let record = self.record(ledger, owner, id)?;
            if !matches!(record.kind, Kind::Run | Kind::Tunnel)
                || record.phase != Phase::Preparing
                || !record.resources.is_empty()
                || record.deadline <= super::now()?
            {
                return Err(Error::OperationInvalid);
            }
            record.resources.push(Resource::LocalIntent {});
            record.phase = Phase::Allocating;
            Ok(())
        })
    }

    /// # Errors
    /// Called only by the armed guardian callback, before authorizing any child command.
    pub fn local_started(&self, owner: Uuid, id: Uuid, operation: Uuid, guardian_pid: u32) -> Result<()> {
        if operation.is_nil() || guardian_pid == 0 {
            return Err(Error::OperationInvalid);
        }
        self.edit(|ledger| {
            if ledger.records.values().any(|record| matches!(record.resources.as_slice(), [Resource::LocalProcess { operation: existing, .. }] if *existing == operation)) {
                return Err(Error::OperationInvalid);
            }
            let record = self.record(ledger, owner, id)?;
            if !matches!(record.kind, Kind::Run | Kind::Tunnel)
                || record.phase != Phase::Allocating
                || !matches!(record.resources.as_slice(), [Resource::LocalIntent {}])
                || record.deadline <= super::now()?
            {
                return Err(Error::OperationInvalid);
            }
            record.resources = vec![Resource::LocalProcess {
                operation,
                guardian_pid,
            }];
            record.phase = Phase::Active;
            Ok(())
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::journal::recovery::{Recovery, Resolution};
    use crate::journal::tests::{canonical_temp, journal};
    use std::time::Duration;

    #[test]
    fn local_process_binding_survives_restart_and_requires_positive_exact_cleanup() {
        let root = canonical_temp();
        let state = root.path().join("state");
        let owner = Uuid::new_v4();
        let first = journal(&state, 'a');
        let op = first
            .start(owner, root.path(), Kind::Run, Duration::from_secs(30))
            .unwrap();
        first.local_intent(owner, op.id).unwrap();
        let guard = Uuid::new_v4();
        first.local_started(owner, op.id, guard, 123).unwrap();
        drop(first);
        let reopened = journal(&state, 'a');
        assert_eq!(reopened.recover_owned(owner, op.id, |input| {
            assert!(matches!(input, Recovery::LocalProcess { operation, guardian_pid } if operation == guard && guardian_pid == 123));
            Err(Error::ReconciliationRequired)
        }), Err(Error::ReconciliationRequired));
        assert_eq!(reopened.status(owner, op.id).unwrap().phase, Phase::Uncertain);
        reopened.recover_owned(owner, op.id, |input| {
            assert!(matches!(input, Recovery::LocalProcess { operation, guardian_pid } if operation == guard && guardian_pid == 123));
            Ok(Resolution::ConfirmedClosed)
        }).unwrap();
        assert_eq!(reopened.status(owner, op.id).unwrap().phase, Phase::Complete);
    }

    #[test]
    fn failed_arming_retains_local_intent_and_foreign_owner_cannot_bind_a_guardian() {
        let root = canonical_temp();
        let state = journal(&root.path().join("state"), 'a');
        let owner = Uuid::new_v4();
        let op = state
            .start(owner, root.path(), Kind::Tunnel, Duration::from_secs(30))
            .unwrap();
        state.local_intent(owner, op.id).unwrap();
        assert_eq!(
            state.local_started(Uuid::new_v4(), op.id, Uuid::new_v4(), 123),
            Err(Error::OwnershipRefused)
        );
        assert_eq!(
            state.local_started(owner, op.id, Uuid::nil(), 123),
            Err(Error::OperationInvalid)
        );
        state
            .recover_owned(owner, op.id, |input| {
                assert!(matches!(input, Recovery::LocalIntent));
                Err(Error::ReconciliationRequired)
            })
            .unwrap_err();
        assert_eq!(state.status(owner, op.id).unwrap().phase, Phase::Uncertain);
    }
    #[test]
    fn duplicate_guardian_binding_is_rejected_by_api_and_persisted_validation() {
        let root = canonical_temp();
        let path = root.path().join("state");
        let state = journal(&path, 'a');
        let owner = Uuid::new_v4();
        let first = state
            .start(owner, root.path(), Kind::Run, Duration::from_secs(30))
            .unwrap();
        let second = state
            .start(owner, root.path(), Kind::Tunnel, Duration::from_secs(30))
            .unwrap();
        let guard = Uuid::new_v4();
        state.local_intent(owner, first.id).unwrap();
        state.local_intent(owner, second.id).unwrap();
        state.local_started(owner, first.id, guard, 123).unwrap();
        assert_eq!(
            state.local_started(owner, second.id, guard, 123),
            Err(Error::OperationInvalid)
        );
        assert_eq!(state.status(owner, second.id).unwrap().phase, Phase::Allocating);
        state.local_started(owner, second.id, Uuid::new_v4(), 124).unwrap();
        let file = path.join("journal.json");
        let mut persisted: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        persisted["records"][second.id.to_string()]["resources"][0]["operation"] = guard.to_string().into();
        std::fs::write(file, serde_json::to_vec(&persisted).unwrap()).unwrap();
        assert!(matches!(state.pending(owner), Err(Error::JournalInvalid)));
    }
}
