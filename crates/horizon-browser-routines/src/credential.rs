use std::collections::BTreeMap;
use std::fmt;

use uuid::Uuid;

use crate::RoutineError;
use crate::definition::RoutineDefinition;
use crate::origin::Origin;
use crate::value::CredentialFieldKind;

/// Engine-owned destination for one fill. Implementations must not retain bytes
/// after [`FillSink::fill`] returns.
pub trait FillSink {
    /// # Errors
    /// Returns a typed failure when the engine cannot accept the field.
    fn fill(&mut self, field: CredentialFieldKind, bytes: &[u8]) -> Result<(), RoutineError>;
}

/// Persistence for opted-in routine credentials. No method returns secret bytes.
pub trait CredentialStore {
    /// # Errors
    /// Locked store, I/O, or policy-rejected identity.
    fn put(
        &mut self,
        routine_id: Uuid,
        slot: Uuid,
        field: CredentialFieldKind,
        secret: &[u8],
    ) -> Result<(), RoutineError>;

    /// # Errors
    /// Locked store or I/O. Missing items succeed as already-absent.
    fn delete(&mut self, routine_id: Uuid, slot: Uuid, field: CredentialFieldKind) -> Result<(), RoutineError>;

    /// # Errors
    /// Locked store or I/O.
    fn contains(&self, routine_id: Uuid, slot: Uuid, field: CredentialFieldKind) -> Result<bool, RoutineError>;

    #[must_use]
    fn is_locked(&self) -> bool;

    /// Copy stored bytes into `sink` only. Never returns the secret.
    ///
    /// # Errors
    /// Locked store, missing item, or sink failure.
    fn fill_into(
        &self,
        routine_id: Uuid,
        slot: Uuid,
        field: CredentialFieldKind,
        sink: &mut dyn FillSink,
    ) -> Result<(), RoutineError>;
}

/// Presence markers for export. No secret values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldPresence {
    pub field: CredentialFieldKind,
    pub present: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialExport {
    pub slot: Option<Uuid>,
    pub fields: Vec<FieldPresence>,
}

/// Lease-scoped origin checks in front of a [`CredentialStore`].
pub struct CredentialBroker<S> {
    store: S,
}

impl<S: CredentialStore> CredentialBroker<S> {
    #[must_use]
    pub const fn new(store: S) -> Self {
        Self { store }
    }

    /// Field names and present/missing markers only.
    ///
    /// # Errors
    /// Locked store or I/O while probing presence.
    pub fn export(&self, routine: &RoutineDefinition) -> Result<CredentialExport, RoutineError> {
        let slot = routine.credential_policy.slot;
        let mut fields = Vec::new();
        if let Some(slot) = slot {
            for field in [CredentialFieldKind::Username, CredentialFieldKind::Password] {
                fields.push(FieldPresence {
                    field,
                    present: self.store.contains(routine.routine_id, slot, field)?,
                });
            }
        }
        Ok(CredentialExport { slot, fields })
    }

    /// # Errors
    /// Locked store, policy/origin mismatch, or sink failure.
    pub fn fill(
        &self,
        routine: &RoutineDefinition,
        origin: &Origin,
        field: CredentialFieldKind,
        sink: &mut dyn FillSink,
    ) -> Result<(), RoutineError> {
        if self.store.is_locked() {
            return Err(RoutineError::LockedStore);
        }
        let slot = routine.credential_policy.slot.ok_or(RoutineError::SlotMismatch)?;
        routine.credential_policy.permits(slot, field)?;
        if !routine
            .credential_policy
            .allowed_origins
            .iter()
            .any(|allowed| allowed == origin)
        {
            return Err(RoutineError::OriginNotAllowed);
        }
        self.store.fill_into(routine.routine_id, slot, field, sink)
    }
}

/// In-memory store for tests. Debug formatting never includes secret bytes.
#[derive(Default)]
pub struct FakeCredentialStore {
    locked: bool,
    items: BTreeMap<(Uuid, Uuid, CredentialFieldKind), Vec<u8>>,
}

impl FakeCredentialStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lock(&mut self) {
        self.locked = true;
    }
}

impl fmt::Debug for FakeCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FakeCredentialStore")
            .field("locked", &self.locked)
            .field("entries", &self.items.len())
            .finish()
    }
}

impl CredentialStore for FakeCredentialStore {
    fn put(
        &mut self,
        routine_id: Uuid,
        slot: Uuid,
        field: CredentialFieldKind,
        secret: &[u8],
    ) -> Result<(), RoutineError> {
        if self.locked {
            return Err(RoutineError::LockedStore);
        }
        if secret.is_empty() {
            return Err(RoutineError::InvalidRecording);
        }
        self.items.insert((routine_id, slot, field), secret.to_vec());
        Ok(())
    }

    fn delete(&mut self, routine_id: Uuid, slot: Uuid, field: CredentialFieldKind) -> Result<(), RoutineError> {
        if self.locked {
            return Err(RoutineError::LockedStore);
        }
        self.items.remove(&(routine_id, slot, field));
        Ok(())
    }

    fn contains(&self, routine_id: Uuid, slot: Uuid, field: CredentialFieldKind) -> Result<bool, RoutineError> {
        if self.locked {
            return Err(RoutineError::LockedStore);
        }
        Ok(self.items.contains_key(&(routine_id, slot, field)))
    }

    fn is_locked(&self) -> bool {
        self.locked
    }

    fn fill_into(
        &self,
        routine_id: Uuid,
        slot: Uuid,
        field: CredentialFieldKind,
        sink: &mut dyn FillSink,
    ) -> Result<(), RoutineError> {
        if self.locked {
            return Err(RoutineError::LockedStore);
        }
        let secret = self
            .items
            .get(&(routine_id, slot, field))
            .ok_or(RoutineError::SlotMismatch)?;
        sink.fill(field, secret)
    }
}

impl<S: fmt::Debug> fmt::Debug for CredentialBroker<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialBroker")
            .field("store", &self.store)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{CredentialBroker, CredentialStore as _, FakeCredentialStore, FillSink};
    use crate::RoutineError;
    use crate::definition::RoutineDefinition;
    use crate::definition::tests::sample_definition;
    use crate::definition::{CredentialMode, CredentialPolicy};
    use crate::origin::Origin;
    use crate::value::CredentialFieldKind;
    use uuid::Uuid;

    struct RecordingSink {
        filled: bool,
    }

    impl FillSink for RecordingSink {
        fn fill(&mut self, _field: CredentialFieldKind, bytes: &[u8]) -> Result<(), RoutineError> {
            self.filled = !bytes.is_empty();
            Ok(())
        }
    }

    fn remember_username(routine: &mut RoutineDefinition) {
        routine.credential_policy = CredentialPolicy {
            mode: CredentialMode::UsernameOnly,
            slot: Some(Uuid::from_u128(1)),
            allowed_origins: vec![Origin::parse("https://reports.example").expect("origin")],
        };
    }

    #[test]
    fn fill_into_never_returns_bytes_and_export_has_no_values() {
        let mut store = FakeCredentialStore::new();
        let mut routine = sample_definition();
        remember_username(&mut routine);
        let slot = routine.credential_policy.slot.expect("slot");
        store
            .put(routine.routine_id, slot, CredentialFieldKind::Username, b"qkzm")
            .expect("put");
        let broker = CredentialBroker::new(store);
        let mut sink = RecordingSink { filled: false };
        broker
            .fill(
                &routine,
                &Origin::parse("https://reports.example").expect("origin"),
                CredentialFieldKind::Username,
                &mut sink,
            )
            .expect("fill");
        assert!(sink.filled);
        let exported = broker.export(&routine).expect("export");
        assert_eq!(exported.slot, Some(slot));
        assert!(
            exported
                .fields
                .iter()
                .any(|field| field.field == CredentialFieldKind::Username && field.present)
        );
        let encoded = format!("{exported:?}");
        assert!(!encoded.to_ascii_lowercase().contains("secret"));
        assert!(!encoded.contains("qkzm"));
    }

    #[test]
    fn another_routine_id_does_not_fill() {
        let mut store = FakeCredentialStore::new();
        let mut routine = sample_definition();
        remember_username(&mut routine);
        let slot = routine.credential_policy.slot.expect("slot");
        store
            .put(Uuid::from_u128(9), slot, CredentialFieldKind::Username, b"x")
            .expect("put");
        let broker = CredentialBroker::new(store);
        let mut sink = RecordingSink { filled: false };
        assert_eq!(
            broker.fill(
                &routine,
                &Origin::parse("https://reports.example").expect("origin"),
                CredentialFieldKind::Username,
                &mut sink,
            ),
            Err(RoutineError::SlotMismatch)
        );
        assert!(!sink.filled);
    }

    #[test]
    fn unapproved_origin_does_not_fill() {
        let mut store = FakeCredentialStore::new();
        let mut routine = sample_definition();
        remember_username(&mut routine);
        let slot = routine.credential_policy.slot.expect("slot");
        store
            .put(routine.routine_id, slot, CredentialFieldKind::Username, b"x")
            .expect("put");
        let broker = CredentialBroker::new(store);
        let mut sink = RecordingSink { filled: false };
        assert_eq!(
            broker.fill(
                &routine,
                &Origin::parse("https://idp.example").expect("origin"),
                CredentialFieldKind::Username,
                &mut sink,
            ),
            Err(RoutineError::OriginNotAllowed)
        );
        assert!(!sink.filled);
    }

    #[test]
    fn locked_store_degrades_without_plaintext() {
        let mut store = FakeCredentialStore::new();
        store.lock();
        let broker = CredentialBroker::new(store);
        let mut routine = sample_definition();
        remember_username(&mut routine);
        let mut sink = RecordingSink { filled: false };
        assert_eq!(
            broker.fill(
                &routine,
                &Origin::parse("https://reports.example").expect("origin"),
                CredentialFieldKind::Username,
                &mut sink,
            ),
            Err(RoutineError::LockedStore)
        );
        assert!(!sink.filled);
    }
}
