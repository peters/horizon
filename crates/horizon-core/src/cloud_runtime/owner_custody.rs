//! Live original-cloud custody. A typed alias is never native approval or restored authority.
use super::{
    local_network::endpoint::Recipient,
    settings::Settings,
    state::{
        Deployment, Store,
        service_admission::{OwnerScope, Record},
    },
};
use crate::cloud_panel::CloudLaunch;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The original live cloud owner scope changed")]
    Scope,
    #[error("The original live cloud record is missing")]
    Missing,
    #[error("The original live cloud worker binding changed")]
    Binding,
    #[error("The original live cloud field changed before its save")]
    Conflict,
    #[error("The original live cloud tailnet selection is unavailable")]
    Selection,
    #[error(transparent)]
    State(#[from] super::Error),
}
impl From<Error> for super::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::Scope => Self::Invalid("The original live cloud owner scope changed"),
            Error::Missing => Self::Invalid("The original live cloud record is missing"),
            Error::Binding => Self::Invalid("The original live cloud worker binding changed"),
            Error::Conflict => Self::Invalid("The original live cloud field changed before its save"),
            Error::Selection => Self::Invalid("The original live cloud tailnet selection is unavailable"),
            Error::State(error) => error,
        }
    }
}

struct Inner {
    store: Mutex<Store>,
    root: PathBuf,
    owner: OwnerScope,
    recipient: Recipient,
}

/// One actual OS Store lock shared only by current original native controllers.
/// Capturing or cloning this value does not approve a new sharing/exposure start.
/// Every new start must recollect actual native context and explicit owner approval.
/// Other lifecycle operations remain Busy until all original aliases retire.
#[derive(Clone)]
pub struct OriginalCloud(Arc<Inner>);

/// Same-transaction comparison facts only, never worker identity observation or approval.
pub struct CheckedSnapshot {
    pub state: Deployment,
    pub recipient: Recipient,
    pub selection: super::tailnet::Selection,
}

impl OriginalCloud {
    /// Capture original comparison facts while consuming the already-held Store.
    /// Run on the background owner job; this reads settings and the original record.
    /// # Errors
    /// Refuses malformed scope, absent records and an unavailable original ready worker.
    pub fn capture(store: Store, settings: &Settings, owner: OwnerScope, launch: &CloudLaunch) -> Result<Self, Error> {
        if !OwnerScope::new(
            &owner.host_instance,
            &owner.session_id.to_string(),
            &owner.workspace_id,
            &owner.cloud_id,
        )
        .is_ok_and(|checked| checked == owner)
        {
            return Err(Error::Scope);
        }
        let state = store.load()?.ok_or(Error::Missing)?;
        if state.cloud_id != owner.cloud_id || launch.id != owner.cloud_id {
            return Err(Error::Binding);
        }
        let recipient = Recipient::from_snapshot(&state, settings, store.root(), launch).map_err(|_| Error::Binding)?;
        Ok(Self(Arc::new(Inner {
            root: store.root().to_path_buf(),
            store: Mutex::new(store),
            owner,
            recipient,
        })))
    }
    #[must_use]
    pub fn owner(&self) -> &OwnerScope {
        &self.0.owner
    }
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.0.root
    }
    fn lock(&self, owner: &OwnerScope) -> Result<MutexGuard<'_, Store>, Error> {
        if owner != &self.0.owner {
            return Err(Error::Scope);
        }
        Ok(self.0.store.lock().unwrap_or_else(PoisonError::into_inner))
    }
    fn current(&self, store: &Store) -> Result<Deployment, Error> {
        let state = store.load()?.ok_or(Error::Missing)?;
        if !self.0.recipient.matches(&state) {
            return Err(Error::Binding);
        }
        Ok(state)
    }
    /// Fresh comparison facts, never a snapshot that callers can save wholesale.
    /// # Errors
    /// Refuses another scope, missing/unreadable records and original worker drift.
    pub fn snapshot(&self, owner: &OwnerScope) -> Result<Deployment, Error> {
        let store = self.lock(owner)?;
        self.current(&store)
    }
    /// Recheck current settings/pins without lending the raw Store to another caller.
    /// # Errors
    /// Refuses a changed original scope, launch, worker or pinned connection.
    pub fn recipient(&self, owner: &OwnerScope, settings: &Settings, launch: &CloudLaunch) -> Result<Recipient, Error> {
        self.checked_snapshot(owner, settings, launch)
            .map(|checked| checked.recipient)
    }
    /// One guarded record and its checked full launch/settings/pins comparison.
    /// Enrollment and selected-tailnet facts in the snapshot belong to that same read.
    /// An actual current worker identity observer remains a separate prerequisite.
    /// # Errors
    /// Refuses another scope, missing/unreadable records or any original binding drift.
    pub fn checked_snapshot(
        &self,
        owner: &OwnerScope,
        settings: &Settings,
        launch: &CloudLaunch,
    ) -> Result<CheckedSnapshot, Error> {
        let store = self.lock(owner)?;
        let state = self.current(&store)?;
        let recipient = Recipient::from_snapshot(&state, settings, store.root(), launch).map_err(|_| Error::Binding)?;
        if recipient != self.0.recipient {
            return Err(Error::Binding);
        }
        let selection = super::tailnet::Selection::load(store.root()).map_err(|_| Error::Selection)?;
        Ok(CheckedSnapshot {
            state,
            recipient,
            selection,
        })
    }
    pub(in crate::cloud_runtime) fn replace_admission(
        &self,
        owner: &OwnerScope,
        expected: Option<&Record>,
        next: Record,
    ) -> Result<(), Error> {
        let store = self.lock(owner)?;
        let mut state = self.current(&store)?;
        if next.owner() != owner || state.worker.as_ref().is_none_or(|worker| next.worker_id() != worker.id) {
            return Err(Error::Binding);
        }
        if state.service_admission.as_ref() != expected {
            return Err(Error::Conflict);
        }
        state.service_admission = Some(next);
        store.save(&state)?;
        Ok(())
    }
    /// Save only an exact original exposure field, preserving all fresh sibling metadata.
    /// This records facts only; it cannot start a keeper or approve an exposure.
    /// # Errors
    /// Distinguishes foreign scope, missing record, original binding drift and field conflict.
    #[cfg(feature = "cloud-exposure")]
    pub fn replace_exposure(
        &self,
        owner: &OwnerScope,
        expected: Option<&super::state::exposure::Record>,
        next: super::state::exposure::Record,
    ) -> Result<(), Error> {
        let store = self.lock(owner)?;
        let mut state = self.current(&store)?;
        let binding = next.binding();
        if binding.owner != *owner
            || binding.launch != *self.0.recipient.launch()
            || state.spec.as_ref() != Some(&binding.spec)
            || state
                .worker
                .as_ref()
                .is_none_or(|worker| worker.id != binding.worker_id)
            || binding.pinned_ssh_args.as_slice() != self.0.recipient.pinned_args()
        {
            return Err(Error::Binding);
        }
        if state.exposure.as_ref() != expected {
            return Err(Error::Conflict);
        }
        state.exposure = Some(next);
        store.save(&state)?;
        Ok(())
    }
}

#[cfg(all(test, unix, feature = "cloud-exposure"))]
mod tests;
