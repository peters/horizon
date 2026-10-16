//! Trusted owner controller. Saved scope is metadata; native fulfillment must
//! verify original membership and explicit approval before calling start.
use super::{Gateway, Prepared, Receipt, attachment::Attached};
use crate::cloud_runtime::{
    Cancellation, Error, Result,
    owner_custody::OriginalCloud,
    settings::Settings,
    ssh::Connection,
    state::{
        Deployment, Store,
        service_admission::{OwnerScope, Phase, Record},
    },
};
use std::{
    io,
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

const OBSERVE_INTERVAL: Duration = Duration::from_millis(100);

/// Original live controller facts, never a deserialized restart grant.
#[derive(Clone, PartialEq, Eq)]
pub struct RecipientView {
    cloud_id: String,
    revision: String,
    profile: horizon_cloud::Profile,
    spec: horizon_cloud::WorkerSpec,
    worker_id: String,
    image: String,
    endpoint: horizon_cloud::SshEndpoint,
    docker_host: Option<Box<horizon_cloud::docker_host::Host>>,
}

/// Exact pinned connection facts; only checked Store/settings lookup creates this.
#[derive(Clone, PartialEq, Eq)]
pub struct Recipient {
    launch: crate::cloud_panel::CloudLaunch,
    view: RecipientView,
    ssh: Vec<String>,
}

impl Recipient {
    /// # Errors
    /// Requires the exact saved launch/current ready worker and original pinned SSH route.
    pub fn checked(store: &Store, settings: &Settings, launch: &crate::cloud_panel::CloudLaunch) -> Result<Self> {
        let state = store.load()?.ok_or(Error::Invalid("Missing one-service deployment"))?;
        Self::from_snapshot(&state, settings, store.root(), launch)
    }

    pub(in crate::cloud_runtime) fn from_snapshot(
        state: &Deployment,
        settings: &Settings,
        root: &std::path::Path,
        launch: &crate::cloud_panel::CloudLaunch,
    ) -> Result<Self> {
        if !state.worker_ready()
            || state.refuse_pending_replacement().is_err()
            || state.session_restart.is_some()
            || state.cloud_id != launch.id
            || state.revision != launch.revision
            || state.profile != launch.profile
        {
            return Err(Error::Invalid("The approved one-service launch changed"));
        }
        let worker = state
            .worker
            .clone()
            .ok_or(Error::Invalid("Missing one-service recipient"))?;
        let ssh = Connection::new(&worker, settings, root)?.pinned_args();
        let view = RecipientView::current(state)?;
        Ok(Self {
            launch: launch.clone(),
            view,
            ssh,
        })
    }

    #[must_use]
    pub fn view(&self) -> &RecipientView {
        &self.view
    }
    pub(in crate::cloud_runtime) fn matches(&self, state: &Deployment) -> bool {
        self.view.matches(state)
    }
    pub(in crate::cloud_runtime) fn launch(&self) -> &crate::cloud_panel::CloudLaunch {
        &self.launch
    }
    #[cfg(feature = "cloud-exposure")]
    pub(in crate::cloud_runtime) fn pinned_args(&self) -> &[String] {
        &self.ssh
    }
}

impl RecipientView {
    /// Pure native-controller facts only; never owner approval or dispatch authority.
    /// # Errors
    /// Refuses missing or unsettled ready worker records.
    pub fn current(state: &Deployment) -> Result<Self> {
        if !state.worker_ready() || state.refuse_pending_replacement().is_err() || state.session_restart.is_some() {
            return Err(Error::Invalid("The original one-service worker is unavailable"));
        }
        let worker = state
            .worker
            .as_ref()
            .ok_or(Error::Invalid("Missing one-service recipient"))?;
        Ok(Self {
            cloud_id: state.cloud_id.clone(),
            revision: state.revision.clone(),
            profile: state.profile.clone(),
            spec: state
                .spec
                .clone()
                .ok_or(Error::Invalid("Missing one-service worker specification"))?,
            worker_id: worker.id.clone(),
            image: worker.image_name.clone(),
            endpoint: worker
                .ssh_endpoint()
                .ok_or(Error::Invalid("Missing original SSH endpoint"))?,
            docker_host: worker.docker_host.clone(),
        })
    }

    fn matches(&self, state: &Deployment) -> bool {
        state.worker_ready()
            && state.refuse_pending_replacement().is_ok()
            && state.session_restart.is_none()
            && state.cloud_id == self.cloud_id
            && state.revision == self.revision
            && state.profile == self.profile
            && state.spec.as_ref() == Some(&self.spec)
            && state.worker.as_ref().is_some_and(|worker| {
                worker.id == self.worker_id
                    && worker.image_name == self.image
                    && worker.ssh_endpoint().as_ref() == Some(&self.endpoint)
                    && worker.docker_host == self.docker_host
            })
    }
}

/// Cached metadata plus actual helper state. Attached phase records local custody,
/// not remote Ready, endpoint reachability or TLS/agent authentication.
#[derive(Clone, Debug)]
pub struct StatusSnapshot {
    pub retained: Record,
    pub transport: super::attachment::StatusSnapshot,
    pub stopping: bool,
}

/// Holds the existing original lifecycle lock until local source and SSH retire.
/// There is intentionally no restoration factory from a saved receipt or port.
pub struct Admitted {
    attachment: Option<Arc<Attached>>,
    watcher: Option<JoinHandle<()>>,
    cancellation_watcher: Option<JoinHandle<()>>,
    stop: Cancellation,
    store: OriginalCloud,
    record: Record,
    receipt: Receipt,
    recipient: Recipient,
}

impl Admitted {
    /// Must run in a background owner job after native approval of `prepared.checked()`.
    /// # Errors
    /// Refuses changed original scope/worker/SSH, unresolved attempts and failed durable saves.
    pub fn start(
        store: Store,
        settings: &Settings,
        owner: OwnerScope,
        recipient: Recipient,
        prepared: Prepared,
    ) -> Result<Self> {
        Self::start_cancelled(store, settings, owner, recipient, prepared, Cancellation::default())
    }

    /// The native job and the physical watcher share cancellation. Cancelling a
    /// blocked saved-session observation still closes the exact admitted source.
    /// # Errors
    /// Refuses observed cancellation before dispatch and preserves any armed intent.
    pub fn start_cancelled(
        store: Store,
        settings: &Settings,
        owner: OwnerScope,
        recipient: Recipient,
        prepared: Prepared,
        cancellation: Cancellation,
    ) -> Result<Self> {
        if cancellation.is_cancelled() {
            return Err(Error::Invalid("The original one-service owner job was cancelled"));
        }
        let store = OriginalCloud::capture(store, settings, owner.clone(), recipient.launch())?;
        Self::start_shared_cancelled(store, settings, owner, recipient, prepared, cancellation)
    }

    /// Start from actual shared original custody, after fresh current native approval.
    /// A live alias is not approval; saved scope and restored metadata cannot create one.
    /// # Errors
    /// Refuses a foreign scope, changed recipient, unresolved attempt or failed durable save.
    pub fn start_shared_cancelled(
        store: OriginalCloud,
        settings: &Settings,
        owner: OwnerScope,
        recipient: Recipient,
        prepared: Prepared,
        cancellation: Cancellation,
    ) -> Result<Self> {
        Self::start_shared_cancelled_with(
            store,
            settings,
            owner,
            recipient,
            prepared,
            cancellation,
            |prepared, connection| {
                Gateway::start_private(prepared)
                    .and_then(|gateway| gateway.attach(connection))
                    .map_err(|_| Error::Invalid("The original one-service bridge could not start"))
            },
        )
    }

    #[cfg(all(test, unix))]
    fn start_with(
        store: Store,
        settings: &Settings,
        owner: OwnerScope,
        recipient: Recipient,
        prepared: Prepared,
        dispatch: impl FnOnce(Prepared, Connection) -> Result<Attached>,
    ) -> Result<Self> {
        let store = OriginalCloud::capture(store, settings, owner.clone(), recipient.launch())?;
        Self::start_shared_cancelled_with(
            store,
            settings,
            owner,
            recipient,
            prepared,
            Cancellation::default(),
            dispatch,
        )
    }

    #[cfg(all(test, unix))]
    fn start_cancelled_with(
        store: Store,
        settings: &Settings,
        owner: OwnerScope,
        recipient: Recipient,
        prepared: Prepared,
        cancellation: Cancellation,
        dispatch: impl FnOnce(Prepared, Connection) -> Result<Attached>,
    ) -> Result<Self> {
        if cancellation.is_cancelled() {
            return Err(Error::Invalid("The original one-service owner job was cancelled"));
        }
        let store = OriginalCloud::capture(store, settings, owner.clone(), recipient.launch())?;
        Self::start_shared_cancelled_with(store, settings, owner, recipient, prepared, cancellation, dispatch)
    }

    fn start_shared_cancelled_with(
        store: OriginalCloud,
        settings: &Settings,
        owner: OwnerScope,
        recipient: Recipient,
        prepared: Prepared,
        cancellation: Cancellation,
        dispatch: impl FnOnce(Prepared, Connection) -> Result<Attached>,
    ) -> Result<Self> {
        if cancellation.is_cancelled() {
            return Err(Error::Invalid("The original one-service owner job was cancelled"));
        }
        let checked = store.checked_snapshot(&owner, settings, recipient.launch())?;
        if checked.recipient != recipient {
            return Err(Error::Invalid("The approved one-service recipient changed"));
        }
        let state = checked.state;
        let worker = state
            .worker
            .as_ref()
            .ok_or(Error::Invalid("Missing one-service recipient"))?;
        let connection = Connection::new(worker, settings, store.root())?;
        if cancellation.is_cancelled() {
            return Err(Error::Invalid("The original one-service owner job was cancelled"));
        }
        if state.cloud_id != owner.cloud_id || !recipient.matches(&state) || connection.pinned_args() != recipient.ssh {
            return Err(Error::Invalid("The approved one-service recipient changed"));
        }
        if state
            .service_admission
            .as_ref()
            .is_some_and(|record| record.phase() != (Phase::Closed {}))
        {
            return Err(Error::Invalid("The original one-service attempt remains unresolved"));
        }
        let record = prepared.retained(owner, recipient.view.worker_id.clone())?;
        store.replace_admission(record.owner(), state.service_admission.as_ref(), record.clone())?;
        if cancellation.is_cancelled() {
            return Err(Error::Invalid("The original one-service owner job was cancelled"));
        }
        // No listener or SSH command precedes this durable intent. Any subsequent
        // startup/save failure keeps Armed evidence; it is never automatically reopened.
        let attachment = Arc::new(dispatch(prepared, connection)?);
        if cancellation.is_cancelled() {
            return Err(Error::Invalid("The original one-service owner job was cancelled"));
        }
        let receipt = attachment.status().gateway.receipt;
        let mut admitted = Self {
            attachment: Some(Arc::clone(&attachment)),
            watcher: None,
            cancellation_watcher: None,
            stop: cancellation,
            store,
            record,
            receipt: receipt.clone(),
            recipient,
        };
        admitted.cancellation_watcher = Some(watch_cancellation(attachment, receipt, admitted.stop.clone())?);
        let armed = admitted.record.clone();
        admitted.record.set_attached(
            admitted.receipt.operation_id,
            admitted.receipt.generation,
            admitted.receipt.proxy_port,
        )?;
        admitted
            .store
            .replace_admission(admitted.record.owner(), Some(&armed), admitted.record.clone())?;
        if admitted.stop.is_cancelled() {
            return Err(Error::Invalid("The original one-service owner job was cancelled"));
        }
        let original = admitted.store.clone();
        let scope = admitted.record.owner().clone();
        admitted.watcher = Some(watch_record(
            admitted.recipient.clone(),
            admitted.record.clone(),
            admitted.stop.clone(),
            move || original.snapshot(&scope).map(Some).map_err(Into::into),
        )?);
        Ok(admitted)
    }

    /// An alias of actual original custody, never permission for another start.
    /// A new controller must recollect current native context and owner approval.
    #[must_use]
    pub fn live_cloud(&self) -> OriginalCloud {
        self.store.clone()
    }

    #[must_use]
    pub fn status(&self) -> Option<StatusSnapshot> {
        self.attachment.as_ref().map(|attachment| StatusSnapshot {
            retained: self.record.clone(),
            transport: attachment.status(),
            stopping: self.stop.is_cancelled(),
        })
    }

    /// Trusted native context refresh; original session/workspace/host drift revokes.
    /// # Errors
    /// Any changed/unavailable membership or settings closes source without following successors.
    pub fn observe(
        &self,
        owner: &OwnerScope,
        settings: &Settings,
        launch: &crate::cloud_panel::CloudLaunch,
    ) -> Result<()> {
        if owner != self.record.owner()
            || self.recipient.launch != *launch
            || !self
                .store
                .recipient(owner, settings, launch)
                .is_ok_and(|current| current == self.recipient)
        {
            self.retire();
            return Err(Error::Invalid("The original one-service owner or recipient changed"));
        }
        Ok(())
    }

    /// # Errors
    /// Another owner's or generation's close never changes this original handle.
    pub fn close(&self, owner: &OwnerScope, receipt: &Receipt) -> Result<()> {
        if owner != self.record.owner() || receipt != &self.receipt {
            return Err(Error::Invalid("One-service close identity changed"));
        }
        self.retire();
        Ok(())
    }

    fn retire(&self) {
        self.stop.cancel();
        if let Some(attachment) = &self.attachment {
            let _ = attachment.close(&self.receipt);
        }
    }

    fn reap(&mut self) {
        self.retire();
        if let Some(watcher) = self.cancellation_watcher.take() {
            let _ = watcher.join();
        }
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
        drop(self.attachment.take());
    }

    /// Close physically first, then save only the exact original attempt's outcome.
    /// Invalid close identities retain this live handle. Dropping the handle is
    /// a separate owner action that retires its physical resources.
    /// # Errors
    /// Changed/lost records preserve uncertainty; nothing is recreated from metadata.
    pub fn finish(&mut self, owner: &OwnerScope, receipt: &Receipt) -> Result<()> {
        self.close(owner, receipt)?;
        self.reap();
        let mut closed = self.record.clone();
        closed.set_closed(closed.attempt())?;
        self.store
            .replace_admission(owner, Some(&self.record), closed.clone())?;
        self.record = closed;
        Ok(())
    }
}

fn watch_cancellation(attachment: Arc<Attached>, receipt: Receipt, stop: Cancellation) -> io::Result<JoinHandle<()>> {
    thread::Builder::new().name("one-service-cancel".into()).spawn(move || {
        while !stop.is_cancelled() {
            thread::sleep(OBSERVE_INTERVAL);
        }
        // No record, settings, status or SSH wait can delay physical revocation.
        let _ = attachment.close(&receipt);
    })
}

fn watch_record(
    recipient: Recipient,
    expected: Record,
    stop: Cancellation,
    mut read: impl FnMut() -> Result<Option<Deployment>> + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new().name("one-service-owner".into()).spawn(move || {
        while !stop.is_cancelled() {
            if !read()
                .ok()
                .flatten()
                .is_some_and(|state| recipient.matches(&state) && state.service_admission.as_ref() == Some(&expected))
            {
                stop.cancel();
                return;
            }
            thread::sleep(OBSERVE_INTERVAL);
        }
    })
}

impl Drop for Admitted {
    fn drop(&mut self) {
        self.reap();
    }
}

#[cfg(test)]
mod tests;
