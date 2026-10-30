//! The values a caller already read from the OS store, offered back as that
//! store. A request built from them takes the same path, and produces the same
//! refusals, as one built from the store directly, without the caller taking
//! the store mutex or asking the keychain on its own thread.

use std::collections::BTreeMap;

use horizon_browser::remote::{CredentialReference, CredentialStoreKind};
use zeroize::Zeroizing;

use super::{CredentialLocator, RemoteCredentialError, RemoteCredentialStore, Sealed, SecretSink};

/// Values read from the OS store on the workbench thread, addressed by the
/// reference they answer. A reference that was not read is missing here, and a
/// value that cannot become part of a header fails at the sink, exactly as the
/// value the OS store holds would. Every buffer is wiped when the store drops.
#[derive(Default)]
pub struct ReadCredentialStore {
    values: BTreeMap<CredentialReference, Zeroizing<Vec<u8>>>,
}

impl ReadCredentialStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep one read value, with the reference it answers.
    pub fn insert(&mut self, reference: CredentialReference, value: Zeroizing<Vec<u8>>) {
        self.values.insert(reference, value);
    }

    /// Whether any value was read, so a caller that read none resolves the
    /// request as a computer without an OS store does.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl Sealed for ReadCredentialStore {}

impl RemoteCredentialStore for ReadCredentialStore {
    fn kind(&self) -> CredentialStoreKind {
        CredentialStoreKind::OsKeychain
    }

    fn put(&mut self, _: &CredentialLocator, _: &[u8]) -> Result<(), RemoteCredentialError> {
        Err(RemoteCredentialError::StoreUnavailable)
    }

    fn delete(&mut self, _: &CredentialLocator) -> Result<(), RemoteCredentialError> {
        Err(RemoteCredentialError::StoreUnavailable)
    }

    fn contains(&self, locator: &CredentialLocator) -> Result<bool, RemoteCredentialError> {
        Ok(self.values.contains_key(&locator.reference))
    }

    fn with_secret(&self, locator: &CredentialLocator, sink: &mut dyn SecretSink) -> Result<(), RemoteCredentialError> {
        match self.values.get(&locator.reference) {
            Some(value) => sink.accept(value),
            None => Err(RemoteCredentialError::Missing),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_browser::remote::{ControlEndpoint, CredentialBinding};
    use horizon_browser_control::remote_authorization::header_safe_value;

    fn locator() -> CredentialLocator {
        let endpoint = ControlEndpoint::parse("https://grid.example.net/wd/hub").expect("endpoint");
        let binding = CredentialBinding {
            store: CredentialStoreKind::OsKeychain,
            slot: Some("remote-browser/grid/key".into()),
        };
        CredentialLocator::new(&endpoint, &CredentialReference::from("key"), &binding)
    }

    struct Value(Option<Zeroizing<String>>);

    impl Sealed for Value {}

    impl SecretSink for Value {
        fn accept(&mut self, bytes: &[u8]) -> Result<(), RemoteCredentialError> {
            self.0 = Some(header_safe_value(bytes)?);
            Ok(())
        }
    }

    #[test]
    fn a_read_value_is_offered_back_with_the_same_checks_as_the_store() {
        let mut store = ReadCredentialStore::new();
        assert!(store.is_empty());
        let mut sink = Value(None);
        assert_eq!(
            store.with_secret(&locator(), &mut sink).expect_err("nothing was read"),
            RemoteCredentialError::Missing
        );
        store.insert(CredentialReference::from("key"), Zeroizing::new(b"tok-en".to_vec()));
        assert!(!store.is_empty());
        store.with_secret(&locator(), &mut sink).expect("the read value");
        assert_eq!(sink.0.as_deref().map(String::as_str), Some("tok-en"));

        let mut unusable = ReadCredentialStore::new();
        unusable.insert(CredentialReference::from("key"), Zeroizing::new(b"line\n".to_vec()));
        let mut sink = Value(None);
        assert_eq!(
            unusable
                .with_secret(&locator(), &mut sink)
                .expect_err("a value that cannot form a header"),
            RemoteCredentialError::NotHeaderSafe
        );
    }

    #[test]
    fn a_read_store_answers_no_write_and_counts_only_its_own_references() {
        let mut store = ReadCredentialStore::new();
        store.insert(CredentialReference::from("key"), Zeroizing::new(b"tok-en".to_vec()));
        assert!(store.contains(&locator()).expect("presence"));
        assert_eq!(
            store
                .put(&locator(), b"other")
                .expect_err("a read store is not a destination"),
            RemoteCredentialError::StoreUnavailable
        );
        assert_eq!(
            store.delete(&locator()).expect_err("a read store is not a destination"),
            RemoteCredentialError::StoreUnavailable
        );
    }
}
