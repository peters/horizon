//! Explicit, single-flight deletion UI; provider authority stays in the core façade.

mod paint;
mod result;

use std::sync::mpsc::{self, Receiver, TryRecvError};

use horizon_core::{
    cloud_run::CloudWorkflowStore,
    remote_environment_delete::{
        ConfiguredEnvironmentDeleteError, ConfiguredEnvironmentDeletion,
        confirm_configured_remote_environment_deletion, delete_configured_remote_environment,
        retry_configured_remote_environment_deletion,
    },
    remote_provider_config::RemoteProviderConfig,
};

use super::{Context, HorizonHome, InventoryAction, RemoteEnvironmentSummary, RemoteEnvironments, WakeOnDrop};
use result::{Notice, supported};

#[derive(Clone, Copy)]
pub(super) enum Action {
    Request,
    Retry,
    Confirm,
    Check,
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Delete,
    Check,
    Retry,
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

struct Request {
    scope: Scope,
    operation: Operation,
}

struct Confirmation {
    request: Request,
    acknowledged: bool,
}

struct Pending {
    request: Request,
    rx: Receiver<Result<ConfiguredEnvironmentDeletion, Error>>,
    discard: bool,
}

#[derive(Default)]
pub(super) struct DeleteState {
    confirmation: Option<Confirmation>,
    pending: Option<Pending>,
    notice: Option<Notice>,
    context: Option<Context>,
}

#[derive(Debug)]
enum Error {
    Storage,
    Worker,
    ResultMismatch,
    Core(ConfiguredEnvironmentDeleteError),
}

impl DeleteState {
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn pending_label(&self) -> &'static str {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.request.operation == Operation::Check)
        {
            "Checking saved Delete. No Delete is sent; verified completion may be saved. Other management requests wait for this check."
        } else {
            "An explicit Delete request is pending. Closing this overview does not cancel it; other management requests wait for completion."
        }
    }

    pub(super) fn cancel(&mut self) {
        if self.confirmation.take().is_some()
            && let Some(ctx) = &self.context
        {
            ctx.request_repaint();
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.cancel();
        let changed = self.notice.take().is_some() || self.pending.as_ref().is_some_and(|pending| !pending.discard);
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
        }
        if changed && let Some(ctx) = &self.context {
            ctx.request_repaint();
        }
    }

    fn sync(&mut self, home: &HorizonHome, config: &RemoteProviderConfig, selected: Option<&RemoteEnvironmentSummary>) {
        if self
            .confirmation
            .as_ref()
            .is_some_and(|confirmation| !confirmation.request.scope.matches(home, config, selected))
        {
            self.cancel();
        }
        if let Some(pending) = &mut self.pending
            && !pending.request.scope.matches(home, config, selected)
        {
            pending.discard = true;
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| !notice.matches(home, config, selected))
        {
            self.notice = None;
        }
    }

    fn request(
        &mut self,
        action: Action,
        home: &HorizonHome,
        config: &RemoteProviderConfig,
        selected: &RemoteEnvironmentSummary,
    ) -> Option<Request> {
        self.sync(home, config, Some(selected));
        if self.is_pending() {
            return None;
        }
        if matches!(action, Action::Cancel) {
            self.cancel();
            return None;
        }
        if matches!(action, Action::Confirm) {
            return self
                .confirmation
                .take()
                .filter(|confirmation| confirmation.acknowledged)
                .map(|confirmation| confirmation.request);
        }
        let operation = match action {
            Action::Request => Operation::Delete,
            Action::Retry => Operation::Retry,
            Action::Check => Operation::Check,
            Action::Confirm | Action::Cancel => return None,
        };
        if !supported(selected, operation) {
            return None;
        }
        self.notice = None;
        let request = Request {
            scope: Scope {
                home: home.clone(),
                config: config.clone(),
                expected: selected.clone(),
            },
            operation,
        };
        if operation == Operation::Check {
            self.confirmation = None;
            Some(request)
        } else {
            self.confirmation = Some(Confirmation {
                request,
                acknowledged: false,
            });
            None
        }
    }

    fn action(
        &mut self,
        action: Action,
        home: &HorizonHome,
        config: &RemoteProviderConfig,
        selected: &RemoteEnvironmentSummary,
        ctx: &Context,
    ) {
        self.context = Some(ctx.clone());
        if let Some(request) = self.request(action, home, config, selected) {
            let (tx, rx) = mpsc::sync_channel(1);
            let scope = request.scope.clone();
            let operation = request.operation;
            let wake = WakeOnDrop(ctx.clone());
            let worker = std::thread::Builder::new()
                .name("remote-environment-delete".into())
                .spawn(move || {
                    let _wake = wake;
                    let _ = tx.send(execute(&scope, operation));
                });
            match worker {
                Ok(_) => {
                    self.pending = Some(Pending {
                        request,
                        rx,
                        discard: false,
                    });
                }
                Err(_) => self.notice = Some(Notice::finish(request, Err(Error::Worker))),
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
            self.notice = Some(Notice::finish(pending.request, result));
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

fn execute(scope: &Scope, operation: Operation) -> Result<ConfiguredEnvironmentDeletion, Error> {
    let store = CloudWorkflowStore::open_existing_without_migration(&scope.home).map_err(|_| Error::Storage)?;
    match operation {
        Operation::Delete => delete_configured_remote_environment(&store, &scope.config, &scope.expected),
        Operation::Check => confirm_configured_remote_environment_deletion(&store, &scope.config, &scope.expected),
        Operation::Retry => retry_configured_remote_environment_deletion(&store, &scope.config, &scope.expected),
    }
    .map_err(Error::Core)
}

impl RemoteEnvironments {
    pub(super) fn deletion_idle(&self) -> bool {
        self.open
            && self.pending.is_none()
            && !self.delete.is_pending()
            && !self.observation.is_pending()
            && !self.stop.is_pending()
            && !self.setup.is_active()
            && !self.repository.is_pending()
            && !self.reconnect.is_pending()
            && !self.reopen.is_pending()
    }

    pub(super) fn guard_delete_action(&mut self, action: InventoryAction) -> InventoryAction {
        if (self.delete.is_pending()
            && !matches!(
                action,
                InventoryAction::None | InventoryAction::Close | InventoryAction::Select(_)
            ))
            || (matches!(action, InventoryAction::Delete(_)) && !self.deletion_idle())
        {
            return InventoryAction::None;
        }
        if !matches!(
            action,
            InventoryAction::None | InventoryAction::Select(_) | InventoryAction::Delete(_)
        ) {
            self.delete.cancel();
        }
        action
    }

    pub(super) fn delete_action(
        &mut self,
        action: InventoryAction,
        home: &HorizonHome,
        config: &RemoteProviderConfig,
        ctx: &Context,
    ) {
        let InventoryAction::Delete(action) = action else {
            return;
        };
        if !self.deletion_idle() {
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
        self.delete.action(action, home, config, &selected, ctx);
    }

    pub(super) fn drain_delete(&mut self, home: &HorizonHome, config: &RemoteProviderConfig) {
        let selected = self
            .page
            .as_ref()
            .filter(|_| self.open)
            .and_then(|page| self.selected.and_then(|index| page.rows.get(index)))
            .map(|row| &row.summary);
        self.delete.sync(home, config, selected);
        if self.delete.drain() {
            self.observation.invalidate();
            self.refresh_when_idle |= self.open;
        }
    }
}

#[cfg(test)]
mod tests;
