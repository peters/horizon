//! Exact-owned recovery for trusted host callbacks; no provider identifiers are agent values.
use super::{Journal, Kind, Phase, Resource, Slot};
use crate::{Error, Result};
use uuid::Uuid;

/// Private host observation input. Deliberately neither Debug nor serializable.
pub enum Recovery<'a> {
    Empty,
    UploadIntent { operation: Uuid, digest: &'a str },
    Uploaded { reference: &'a str },
    AllocationIntent { operation: Uuid },
    Allocated { reference: &'a str },
}

/// Positive host evidence only. Missing provider results are unresolved, never `ConfirmedClosed`.
/// Deliberately neither Debug nor serializable because adopted references remain private.
pub enum Resolution {
    ConfirmedClosed,
    Uploaded(String),
    Allocated(String),
}

impl Journal {
    /// # Errors
    /// Recover an exact owned record after the host has acquired exclusive workspace execution ownership.
    /// Before discovery/cleanup, uncertainty is committed durably. The callback runs under the account lock.
    /// It must use bounded provider queries and must not recursively access this journal.
    /// Absence after an uncertain mutation must return an error, never authorize replay or release.
    pub fn recover_owned(
        &self,
        owner: Uuid,
        id: Uuid,
        observe: impl FnOnce(Recovery<'_>) -> Result<Resolution>,
    ) -> Result<()> {
        let complete = self.edit(|ledger| {
            let record = self.record(ledger, owner, id)?;
            if record.phase == Phase::Complete {
                return Ok(true);
            }
            record.phase = Phase::Uncertain;
            Ok(false)
        })?;
        if complete {
            return Ok(());
        }
        self.edit(|ledger| {
            let record = self.record(ledger, owner, id)?;
            let input = match record.resources.as_slice() {
                [] => Recovery::Empty,
                [Resource::UploadIntent { digest }] => Recovery::UploadIntent { operation: id, digest },
                [Resource::Upload { reference }] => Recovery::Uploaded { reference },
                [Resource::AllocationIntent {}] => Recovery::AllocationIntent { operation: id },
                [Resource::Session { reference }] => Recovery::Allocated { reference },
                _ => return Err(Error::JournalInvalid),
            };
            match observe(input)? {
                Resolution::ConfirmedClosed => {
                    record.resources.clear();
                    record.slot = None;
                    record.phase = Phase::Complete;
                }
                Resolution::Uploaded(reference) => {
                    if record.kind != Kind::Upload
                        || !super::valid_app(&reference)
                        || matches!(record.resources.as_slice(), [Resource::Upload { reference: known }] if known != &reference)
                        || !matches!(
                            record.resources.as_slice(),
                            [Resource::UploadIntent { .. } | Resource::Upload { .. }]
                        )
                    {
                        return Err(Error::OperationInvalid);
                    }
                    record.resources = vec![Resource::Upload { reference }];
                    record.phase = Phase::Active;
                }
                Resolution::Allocated(reference) => {
                    if record.kind != Kind::Session
                        || !super::valid_reference(&reference)
                        || matches!(record.resources.as_slice(), [Resource::Session { reference: known }] if known != &reference)
                        || !matches!(
                            record.resources.as_slice(),
                            [Resource::AllocationIntent {} | Resource::Session { .. }]
                        )
                    {
                        return Err(Error::OperationInvalid);
                    }
                    record.resources = vec![Resource::Session { reference }];
                    record.slot = Some(Slot::Allocated);
                    record.phase = Phase::Active;
                }
            }
            Ok(())
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::tests::{canonical_temp, journal, quota};
    use super::*;
    use std::time::Duration;

    #[test]
    fn lost_upload_reply_can_be_adopted_and_exactly_released_after_restart() {
        let folder = canonical_temp();
        let path = folder.path().join("state");
        let owner = Uuid::new_v4();
        let first = journal(&path, 'a');
        let operation = first
            .start(owner, folder.path(), Kind::Upload, Duration::from_secs(30))
            .unwrap();
        let digest = "a".repeat(64);
        first.upload_intent(owner, operation.id, &digest).unwrap();
        drop(first);
        let reopened = journal(&path, 'a');
        let token = "bs://0123456789abcdef";
        reopened.recover_owned(owner, operation.id, |input| {
            assert!(matches!(input, Recovery::UploadIntent { operation: id, digest: value } if id == operation.id && value == digest));
            Ok(Resolution::Uploaded(token.to_owned()))
        }).unwrap();
        assert_eq!(reopened.status(owner, operation.id).unwrap().phase, Phase::Active);
        reopened
            .release_upload(owner, operation.id, |reference| {
                assert_eq!(reference, token);
                Ok(())
            })
            .unwrap();
        assert_eq!(reopened.status(owner, operation.id).unwrap().phase, Phase::Complete);
    }

    #[test]
    fn missing_allocation_evidence_keeps_uncertain_slot_reserved() {
        let folder = canonical_temp();
        let journal = journal(&folder.path().join("state"), 'a');
        let owner = Uuid::new_v4();
        let uncertain = journal
            .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
            .unwrap();
        journal.reserve(owner, uncertain.id, quota).unwrap();
        assert_eq!(
            journal.recover_owned(owner, uncertain.id, |input| {
                assert!(matches!(input, Recovery::AllocationIntent { operation } if operation == uncertain.id));
                Err(Error::OperationInvalid)
            }),
            Err(Error::OperationInvalid)
        );
        assert_eq!(journal.status(owner, uncertain.id).unwrap().phase, Phase::Uncertain);
        let next = journal
            .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
            .unwrap();
        journal.reserve(owner, next.id, quota).unwrap();
        let blocked = journal
            .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
            .unwrap();
        assert_eq!(
            journal.reserve(owner, blocked.id, quota),
            Err(Error::CapacityUnavailable)
        );
    }

    #[test]
    fn foreign_owner_invalid_or_duplicate_recovery_cannot_change_owned_state() {
        let folder = canonical_temp();
        let journal = journal(&folder.path().join("state"), 'a');
        let owner = Uuid::new_v4();
        let a = journal
            .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
            .unwrap();
        journal.reserve(owner, a.id, quota).unwrap();
        journal.allocated(owner, a.id, "synthetic_session_0123456789").unwrap();
        let b = journal
            .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
            .unwrap();
        journal.reserve(owner, b.id, quota).unwrap();
        assert_eq!(
            journal.recover_owned(Uuid::new_v4(), b.id, |_| panic!("foreign callback executed")),
            Err(Error::OwnershipRefused)
        );
        assert_eq!(
            journal.recover_owned(owner, b.id, |_| Ok(Resolution::Allocated(
                "synthetic_session_0123456789".into()
            ))),
            Err(Error::JournalInvalid)
        );
        assert_eq!(journal.status(owner, b.id).unwrap().phase, Phase::Uncertain);
        assert_eq!(journal.status(owner, a.id).unwrap().phase, Phase::Active);
        assert_eq!(
            journal.recover_owned(owner, b.id, |_| Ok(Resolution::Uploaded(
                "bs://0123456789abcdef".into()
            ))),
            Err(Error::OperationInvalid)
        );
    }
    #[test]
    fn recorded_resource_identity_cannot_be_replaced_during_recovery() {
        let folder = canonical_temp();
        let journal = journal(&folder.path().join("state"), 'a');
        let owner = Uuid::new_v4();
        let operation = journal
            .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
            .unwrap();
        journal.reserve(owner, operation.id, quota).unwrap();
        let original = "synthetic_session_0123456789";
        journal.allocated(owner, operation.id, original).unwrap();
        assert_eq!(
            journal.recover_owned(owner, operation.id, |_| Ok(Resolution::Allocated(
                "different_session_0123456789".into()
            ))),
            Err(Error::OperationInvalid)
        );
        journal
            .release_session(owner, operation.id, |reference| {
                assert_eq!(reference, original);
                Ok(())
            })
            .unwrap();
        let operation = journal
            .start(owner, folder.path(), Kind::Upload, Duration::from_secs(30))
            .unwrap();
        journal.upload_intent(owner, operation.id, &"a".repeat(64)).unwrap();
        let original = "bs://0123456789abcdef";
        journal.uploaded(owner, operation.id, original).unwrap();
        assert_eq!(
            journal.recover_owned(owner, operation.id, |_| Ok(Resolution::Uploaded(
                "bs://1111111111111111".into()
            ))),
            Err(Error::OperationInvalid)
        );
        journal
            .release_upload(owner, operation.id, |reference| {
                assert_eq!(reference, original);
                Ok(())
            })
            .unwrap();
    }
}
