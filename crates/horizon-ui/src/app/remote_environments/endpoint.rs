//! Single-flight consent and result delivery; authenticated coordinate mutation stays in core.

mod paint;

use super::{Context, HorizonHome, InventoryAction, RemoteEnvironmentSummary, RemoteEnvironments, WakeOnDrop};
use horizon_core::{
    cloud_run::{CloudProvider, CloudWorkflowStore, WorkerLifetime},
    remote_provider_config::RemoteProviderConfig,
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_workspace::{
        RemoteRuntimePhase,
        start::{ConfiguredEndpointRefresh, ConfiguredEndpointRefreshError, refresh_configured_runpod_connection},
    },
};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Clone, Copy)]
pub(super) enum Action {
    Request,
    Confirm,
    Cancel,
}

#[derive(Clone)]
struct Scope {
    home: HorizonHome,
    config: RemoteProviderConfig,
    expected: RemoteEnvironmentSummary,
}

impl Scope {
    fn matches(
        &self,
        home: &HorizonHome,
        config: &RemoteProviderConfig,
        selected: Option<&RemoteEnvironmentSummary>,
    ) -> bool {
        self.home == *home && self.config == *config && selected == Some(&self.expected)
    }
}

struct Confirmation {
    scope: Scope,
    acknowledged: bool,
}
struct Pending {
    scope: Scope,
    rx: Receiver<Result<ConfiguredEndpointRefresh, Error>>,
    discard: bool,
}
struct Notice {
    scope: Scope,
    saved: Option<RemoteEnvironmentSummary>,
    message: &'static str,
}

#[derive(Default)]
pub(super) struct EndpointState {
    confirmation: Option<Confirmation>,
    pending: Option<Pending>,
    notice: Option<Notice>,
}

#[derive(Debug)]
enum Error {
    Storage,
    Worker,
    ResultMismatch,
    Core(ConfiguredEndpointRefreshError),
}

fn supported(summary: &RemoteEnvironmentSummary) -> bool {
    cfg!(target_os = "linux")
        && summary.provider == CloudProvider::RunPod
        && summary.lifetime == WorkerLifetime::Persistent
        && summary.worker_identity.as_ref().is_some_and(|worker| {
            worker.provider == summary.provider
                && Some(worker.workflow_id) == summary.workflow_id
                && Some(worker.job_id) == summary.job_id
                && !worker.resource_id.is_empty()
        })
        && matches!(
            summary.saved_phase,
            Some(
                RemoteRuntimePhase::Ready
                    | RemoteRuntimePhase::Reconciling
                    | RemoteRuntimePhase::Stopped { .. }
                    | RemoteRuntimePhase::Starting { .. }
            )
        )
}

impl Notice {
    fn finish(scope: Scope, result: Result<ConfiguredEndpointRefresh, Error>) -> Self {
        let result = result.and_then(|result| {
            let mut allowed = scope.expected.clone();
            allowed.revision = result.saved.revision;
            if !supported(&scope.expected)
                || allowed != result.saved
                || (result.saved.revision != scope.expected.revision
                    && scope.expected.revision.checked_add(1) != Some(result.saved.revision))
            {
                return Err(Error::ResultMismatch);
            }
            Ok(result.saved)
        });
        let (saved, message) = match result {
            Ok(saved) => {
                let message = if saved.revision == scope.expected.revision {
                    "Original SSH identity authenticated; saved coordinates already match. Nothing was started or reconnected."
                } else {
                    "Original SSH identity authenticated and connection coordinates saved. Nothing was started or reconnected."
                };
                (Some(saved), message)
            }
            Err(Error::Storage) => (
                None,
                "Cannot open the existing workflow store. No store was created or migrated; no remote request was made.",
            ),
            Err(Error::Core(ConfiguredEndpointRefreshError::CredentialUnavailable)) => (
                None,
                "Connection refresh requires RUNPOD_API_KEY after retained-identity admission. No credential is entered or displayed here.",
            ),
            Err(Error::Core(_)) => (
                None,
                "Connection refresh was not verified. Check the saved profile, HPS binding, original SSH identity and running worker; refresh saved inventory before trying again.",
            ),
            Err(Error::Worker | Error::ResultMismatch) => (
                None,
                "Connection refresh could not report a matching result. Saved coordinates may have changed; refresh saved inventory. No retry is automatic.",
            ),
        };
        Self { scope, saved, message }
    }
}

impl EndpointState {
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(super) fn cancel(&mut self) {
        self.confirmation = None;
    }
    pub(super) fn invalidate(&mut self) {
        self.cancel();
        self.notice = None;
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
        }
    }

    fn sync(&mut self, home: &HorizonHome, config: &RemoteProviderConfig, selected: Option<&RemoteEnvironmentSummary>) {
        if self
            .confirmation
            .as_ref()
            .is_some_and(|value| !value.scope.matches(home, config, selected))
        {
            self.cancel();
        }
        if let Some(pending) = &mut self.pending
            && !pending.scope.matches(home, config, selected)
        {
            pending.discard = true;
        }
        if self.notice.as_ref().is_some_and(|value| {
            value.scope.home != *home
                || value.scope.config != *config
                || selected.is_none()
                || (selected != Some(&value.scope.expected) && value.saved.as_ref() != selected)
        }) {
            self.notice = None;
        }
    }

    fn request(
        &mut self,
        action: Action,
        home: &HorizonHome,
        config: &RemoteProviderConfig,
        selected: &RemoteEnvironmentSummary,
    ) -> Option<Scope> {
        self.sync(home, config, Some(selected));
        if self.is_pending() {
            return None;
        }
        match action {
            Action::Cancel => self.cancel(),
            Action::Confirm => {
                return self
                    .confirmation
                    .take()
                    .filter(|value| value.acknowledged)
                    .map(|value| value.scope);
            }
            Action::Request if supported(selected) => {
                self.notice = None;
                self.confirmation = Some(Confirmation {
                    scope: Scope {
                        home: home.clone(),
                        config: config.clone(),
                        expected: selected.clone(),
                    },
                    acknowledged: false,
                });
            }
            Action::Request => {}
        }
        None
    }

    fn action(
        &mut self,
        action: Action,
        home: &HorizonHome,
        config: &RemoteProviderConfig,
        selected: &RemoteEnvironmentSummary,
        ctx: &Context,
    ) {
        if let Some(scope) = self.request(action, home, config, selected) {
            let (tx, rx) = mpsc::sync_channel(1);
            let request = scope.clone();
            let wake = WakeOnDrop(ctx.clone());
            match std::thread::Builder::new()
                .name("remote-endpoint-refresh".into())
                .spawn(move || {
                    let _wake = wake;
                    let _ = tx.send(execute(&request));
                }) {
                Ok(_) => {
                    self.pending = Some(Pending {
                        scope,
                        rx,
                        discard: false,
                    });
                }
                Err(_) => self.notice = Some(Notice::finish(scope, Err(Error::Worker))),
            }
        }
        ctx.request_repaint();
    }

    fn drain(&mut self) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.pending = Some(pending);
                return false;
            }
            Err(TryRecvError::Disconnected) => Err(Error::Worker),
        };
        if !pending.discard {
            self.notice = Some(Notice::finish(pending.scope, result));
        }
        true
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        selected: &RemoteEnvironmentSummary,
        idle: bool,
        action: &mut InventoryAction,
    ) {
        paint::show(ui, self, selected, idle, action);
    }
}

fn execute(scope: &Scope) -> Result<ConfiguredEndpointRefresh, Error> {
    let store = CloudWorkflowStore::open_existing_without_migration(&scope.home).map_err(|_| Error::Storage)?;
    refresh_configured_runpod_connection(
        &store,
        &RemoteSshIdentityStore::new(&scope.home),
        &scope.config,
        &scope.expected,
    )
    .map_err(Error::Core)
}

impl RemoteEnvironments {
    pub(super) fn guard_endpoint_action(&mut self, action: InventoryAction) -> InventoryAction {
        if (self.endpoint.is_pending()
            && !matches!(
                action,
                InventoryAction::None | InventoryAction::Close | InventoryAction::Select(_)
            ))
            || (matches!(action, InventoryAction::Endpoint(_)) && !self.deletion_idle())
        {
            return InventoryAction::None;
        }
        if !matches!(
            action,
            InventoryAction::None | InventoryAction::Select(_) | InventoryAction::Endpoint(_)
        ) {
            self.endpoint.cancel();
        }
        action
    }

    pub(super) fn endpoint_action(
        &mut self,
        action: InventoryAction,
        home: &HorizonHome,
        config: &RemoteProviderConfig,
        ctx: &Context,
    ) {
        let InventoryAction::Endpoint(action) = action else {
            return;
        };
        if !self.deletion_idle() || self.endpoint.is_pending() {
            return;
        }
        let Some(selected) = self
            .page
            .as_ref()
            .and_then(|page| self.selected.and_then(|index| page.rows.get(index)))
            .map(|row| row.summary.clone())
        else {
            return;
        };
        self.invalidate_session_views();
        self.stop.cancel_confirmation();
        self.delete.cancel();
        self.endpoint.action(action, home, config, &selected, ctx);
    }

    pub(super) fn drain_endpoint(&mut self, home: &HorizonHome, config: &RemoteProviderConfig) {
        let selected = self
            .page
            .as_ref()
            .filter(|_| self.open)
            .and_then(|page| self.selected.and_then(|index| page.rows.get(index)))
            .map(|row| &row.summary);
        self.endpoint.sync(home, config, selected);
        if self.endpoint.drain() {
            self.observation.invalidate();
            self.refresh_when_idle |= self.open;
        }
    }
}

#[cfg(test)]
mod tests;
