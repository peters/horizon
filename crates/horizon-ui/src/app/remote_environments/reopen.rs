//! Explicit saved-panel loading, inert reopening and read-only task checks.

mod inspection;
mod paint;

use super::{Context, HorizonHome, InventoryAction, RemoteEnvironmentSummary, WakeOnDrop};
use horizon_core::{
    Board, PanelId, PreparedRemoteViewReopen, RemoteViewCatalog, cloud_run::CloudWorkflowStore,
    remote_provider_config::RemoteProviderConfig,
};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Default)]
pub(super) struct ReopenState {
    catalog: Option<CachedCatalog>,
    pending: Option<PendingReopen>,
    notice: Option<String>,
    repaint_context: Option<Context>,
}

struct CachedCatalog {
    catalog: RemoteViewCatalog,
    scope: RequestScope,
    rows: Vec<SavedView>,
}

struct SavedView {
    id: String,
    present: bool,
    inspection: inspection::TaskObservation,
}

struct PendingReopen {
    rx: Receiver<Result<Completion, String>>,
    scope: RequestScope,
    discard: bool,
    inspection: Option<String>,
}

enum Completion {
    Catalog(Box<RemoteViewCatalog>),
    View(Box<PreparedRemoteViewReopen>),
    Inspection(horizon_core::remote_worker_status::RemotePanelObservation),
}

#[derive(Clone)]
struct RequestScope {
    expected: RemoteEnvironmentSummary,
    owner: String,
    config: RemoteProviderConfig,
}

struct ClientContext<'a> {
    home: &'a HorizonHome,
    config: &'a RemoteProviderConfig,
    selected: Option<&'a RemoteEnvironmentSummary>,
    owner: Option<&'a str>,
}

impl RequestScope {
    fn current(client: &ClientContext<'_>) -> Result<Self, &'static str> {
        let owner = client
            .owner
            .ok_or("Open the owning persistent session before reopening its views.")?;
        let expected = client.selected.ok_or("Select a saved environment first.")?;
        if owner != expected.owning_session_id {
            return Err("This environment belongs to another session. Open that session before reopening its views.");
        }
        Ok(Self {
            expected: expected.clone(),
            owner: owner.into(),
            config: client.config.clone(),
        })
    }

    fn matches(&self, client: &ClientContext<'_>) -> bool {
        client.owner == Some(self.owner.as_str())
            && client.selected == Some(&self.expected)
            && *client.config == self.config
    }
}

impl ReopenState {
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn invalidate(&mut self) {
        let changed = self.catalog.take().is_some()
            | self.notice.take().is_some()
            | self.pending.as_ref().is_some_and(|pending| !pending.discard);
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
        }
        if changed && let Some(ctx) = &self.repaint_context {
            ctx.request_repaint();
        }
    }

    fn set_notice(&mut self, message: impl Into<String>, ctx: &Context) {
        self.notice = Some(message.into());
        self.repaint_context = Some(ctx.clone());
        ctx.request_repaint();
    }

    fn load(&mut self, client: &ClientContext<'_>, ctx: &Context) {
        if self.is_pending() {
            return;
        }
        let scope = match RequestScope::current(client) {
            Ok(scope) => scope,
            Err(message) => {
                self.catalog = None;
                self.set_notice(message, ctx);
                return;
            }
        };
        let expected = scope.expected.clone();
        let owner = scope.owner.clone();
        self.catalog = None;
        self.spawn(client.home, scope, ctx, move |store| {
            RemoteViewCatalog::load(store, &owner, &expected)
                .map(Box::new)
                .map(Completion::Catalog)
                .map_err(|error| error.to_string())
        });
    }

    fn reopen(&mut self, client: &ClientContext<'_>, board: &Board, index: usize, ctx: &Context) {
        if self.is_pending() {
            return;
        }
        let Some(cached) = &self.catalog else {
            return;
        };
        if !cached.scope.matches(client) {
            self.invalidate();
            self.set_notice("The session or saved selection changed. Show saved panels again.", ctx);
            return;
        }
        let Some(row) = cached.rows.get(index) else {
            return;
        };
        let request = match board.request_remote_view_reopen(&cached.scope.owner, &cached.catalog, &row.id) {
            Ok(request) => request,
            Err(error) => {
                self.set_notice(error.to_string(), ctx);
                return;
            }
        };
        let scope = cached.scope.clone();
        self.spawn(client.home, scope, ctx, move |store| {
            request
                .prepare(store)
                .map(Box::new)
                .map(Completion::View)
                .map_err(|error| error.to_string())
        });
    }

    fn spawn(
        &mut self,
        home: &HorizonHome,
        scope: RequestScope,
        ctx: &Context,
        work: impl FnOnce(&CloudWorkflowStore) -> Result<Completion, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::sync_channel(1);
        let home = home.clone();
        let wake = WakeOnDrop(ctx.clone());
        let worker = std::thread::Builder::new()
            .name("remote-view-reopening".into())
            .spawn(move || {
                let _wake = wake;
                let result = CloudWorkflowStore::open_read_only(&home)
                    .map_err(|_| "The saved environment could not be safely read. Refresh and retry.".to_string())
                    .and_then(|store| work(&store));
                let _ = tx.send(result);
            });
        self.notice = None;
        self.repaint_context = Some(ctx.clone());
        match worker {
            Ok(_) => {
                self.pending = Some(PendingReopen {
                    rx,
                    scope,
                    discard: false,
                    inspection: None,
                });
            }
            Err(_) => self.notice = Some(worker_failure().into()),
        }
        ctx.request_repaint();
    }

    fn drain(&mut self, client: &ClientContext<'_>, board: &mut Board, ctx: &Context) -> Option<PanelId> {
        let pending = self.pending.take()?;
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.pending = Some(pending);
                return None;
            }
            Err(TryRecvError::Disconnected) => Err(worker_failure().into()),
        };
        ctx.request_repaint();
        if pending.discard || !pending.scope.matches(client) {
            tracing::debug!(
                prepared = matches!(&result, Ok(Completion::View(_))),
                catalog_loaded = matches!(&result, Ok(Completion::Catalog(_))),
                task_checked = matches!(&result, Ok(Completion::Inspection(_))),
                "Discarding a stale remote view reopening result"
            );
            return None;
        }
        if let Some(panel) = pending.inspection {
            self.accept_inspection(&panel, result);
            return None;
        }
        match result {
            Ok(Completion::Catalog(catalog)) => {
                let catalog = *catalog;
                self.catalog = Some(CachedCatalog {
                    rows: catalog
                        .panel_ids()
                        .iter()
                        .map(|id| SavedView {
                            id: id.clone(),
                            present: catalog.view_is_present(board, id),
                            inspection: inspection::TaskObservation::default(),
                        })
                        .collect(),
                    catalog,
                    scope: pending.scope,
                });
                self.notice = None;
            }
            Ok(Completion::View(prepared)) => match board.adopt_reopened_remote_view(&pending.scope.owner, *prepared) {
                Ok(id) => {
                    if let Some(cached) = &mut self.catalog {
                        for row in &mut cached.rows {
                            row.present = cached.catalog.view_is_present(board, &row.id);
                        }
                    }
                    self.notice =
                        Some("View reopened and disconnected. Use Show session panels, then Reconnect.".into());
                    return Some(id);
                }
                Err(error) => self.notice = Some(error.to_string()),
            },
            Err(message) => self.notice = Some(message),
            Ok(Completion::Inspection(_)) => {
                self.notice = Some("The task result did not match its request. Check the task again.".into());
            }
        }
        None
    }
}

fn worker_failure() -> &'static str {
    "The saved-panel worker could not start or finish. You can retry now."
}

pub(super) fn show(ui: &mut egui::Ui, state: &ReopenState, enabled: bool, action: &mut InventoryAction) {
    paint::show(ui, state, enabled, action);
}

impl super::HorizonApp {
    pub(super) fn remote_reopen_action(&mut self, action: InventoryAction, ctx: &Context) {
        let state = &mut self.remote_environments;
        let selected = state
            .page
            .as_ref()
            .filter(|_| state.open)
            .and_then(|page| state.selected.and_then(|index| page.rows.get(index)))
            .map(|row| &row.summary);
        let client = ClientContext {
            home: self.session_store.home(),
            config: &self.template_config.remote,
            selected,
            owner: self
                .active_session
                .as_ref()
                .filter(|session| session.persistent)
                .map(|session| session.session_id.as_str()),
        };
        if state.open
            && state.pending.is_none()
            && !state.observation.is_pending()
            && !state.stop.is_pending()
            && !state.reconnect.is_pending()
        {
            match action {
                InventoryAction::ListReopenPanels => {
                    state.stop.cancel_confirmation();
                    state.reopen.load(&client, ctx);
                }
                InventoryAction::ReopenView(index) => {
                    state.stop.cancel_confirmation();
                    state.reopen.reopen(&client, &self.board, index, ctx);
                }
                InventoryAction::InspectTask(index) => {
                    state.stop.cancel_confirmation();
                    state.reopen.inspect_task(&client, index, ctx);
                }
                _ => {}
            }
        }
        if state.reopen.drain(&client, &mut self.board, ctx).is_some() {
            state.reconnect.invalidate();
            self.mark_runtime_dirty();
        }
    }
}

#[cfg(test)]
mod tests;
