//! The references of these settings that the keyring of an older Horizon can hold.
//!
//! An older Horizon kept a provider value in the keyring under the account
//! `<origin>|<slot>`, where the origin is the origin of the endpoint of its provider
//! and the slot is the slot of the binding of its reference (`KeyringCredentialStore`
//! of that version). This version keeps the same settings, so it can name the same
//! account, and only a reference that these settings name is imported: a row of the
//! secrets file is `credential/<reference>`, and a value under any other account
//! would be a row that nothing reads.

use horizon_browser::remote::{CredentialStoreKind, RemoteBrowserConfig};

use super::super::CredentialLocator;

/// One reference of these settings, with the account that an older Horizon gave its
/// keyring item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedReference {
    /// The account of the older version's item: `<origin>|<slot>`.
    pub user: String,
    /// Where this version keeps the value of the reference.
    pub locator: CredentialLocator,
}

/// The references of one set of settings that the keyring of an older Horizon can
/// answer, in the order of the settings.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyringImportPlan {
    references: Vec<PlannedReference>,
}

impl KeyringImportPlan {
    /// The references of `remote` whose value lives in the secrets file and whose
    /// binding names a slot, which is what the keyring of an older Horizon needed.
    ///
    /// A reference appears once, because the secrets file holds one row for it
    /// whatever provider names it.
    #[must_use]
    pub fn of(remote: &RemoteBrowserConfig) -> Self {
        let mut references: Vec<PlannedReference> = Vec::new();
        for profile in remote.providers.values() {
            let origin = profile.endpoint.origin();
            if origin.is_empty() {
                continue;
            }
            for reference in profile.authentication.references() {
                let Some(binding) = profile.credential_bindings.get(reference) else {
                    continue;
                };
                if binding.store != CredentialStoreKind::File {
                    continue;
                }
                let Some(slot) = binding.slot.as_deref() else {
                    continue;
                };
                if references.iter().any(|planned| planned.locator.reference == *reference) {
                    continue;
                }
                references.push(PlannedReference {
                    user: format!("{origin}|{slot}"),
                    locator: CredentialLocator::new(&profile.endpoint, reference, binding),
                });
            }
        }
        Self { references }
    }

    /// Whether these settings name no reference that a keyring can hold.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.references.is_empty()
    }

    /// The place of every reference, in the order of the settings.
    pub fn locators(&self) -> impl Iterator<Item = &CredentialLocator> {
        self.references.iter().map(|planned| &planned.locator)
    }

    /// The distinct accounts that these settings name, in the order of the settings.
    #[must_use]
    pub fn users(&self) -> Vec<&str> {
        let mut users: Vec<&str> = Vec::new();
        for planned in &self.references {
            if !users.contains(&planned.user.as_str()) {
                users.push(&planned.user);
            }
        }
        users
    }

    /// The references whose value the item of `user` holds, which is more than one
    /// when two references share a slot.
    #[must_use]
    pub fn references_of(&self, user: &str) -> Vec<&PlannedReference> {
        self.references.iter().filter(|planned| planned.user == user).collect()
    }
}
