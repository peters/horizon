//! Lazy, single-flight saved inventory loading for the Remote Environments overview.

mod observation;
mod paint;
mod reconnect;
mod reopen;
mod repository;
mod setup;
mod stop;

use std::sync::mpsc::{self, Receiver, TryRecvError};

use egui::Context;
use horizon_core::{HorizonHome, cloud_run::CloudWorkflowStore, remote_workspace::RemoteEnvironmentSummary};

use super::HorizonApp;

#[derive(Default)]
pub(super) struct RemoteEnvironments {
    open: bool,
    page: Option<InventoryPage>,
    page_cursor: Option<String>,
    selected: Option<usize>,
    failure: Option<LoadFailure>,
    pending: Option<PendingLoad>,
    refresh_when_idle: bool,
    observation: observation::ObservationState,
    stop: stop::StopState,
    reconnect: reconnect::ReconnectState,
    reopen: reopen::ReopenState,
    repository: repository::RepositoryState,
    setup: setup::SetupState,
    refresh_after_stop: bool,
}

struct InventoryPage {
    rows: Vec<paint::InventoryRow>,
    next_cursor: Option<String>,
}

struct PendingLoad {
    rx: Receiver<Result<InventoryPage, LoadError>>,
    cursor: Option<String>,
    discard: bool,
}

struct LoadFailure {
    error: LoadError,
    cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoadError {
    OpenStore,
    ReadPage,
    WorkerUnavailable,
}

#[derive(Clone, Copy, Default)]
enum InventoryAction {
    #[default]
    None,
    Close,
    Refresh,
    First,
    Next,
    Retry,
    Select(usize),
    Observe,
    RequestStop,
    ConfirmStop,
    CheckStop,
    CancelStop,
    RequestStart,
    ConfirmStart,
    ListReconnectViews,
    Reconnect(horizon_core::PanelId),
    ListReopenPanels,
    ReopenView(usize),
    InspectTask(usize),
    PrepareTaskStart(usize),
    ConfirmTaskStart,
    CancelTaskStart,
    PrepareRepository,
    ConfirmRepository,
    CancelRepository,
    InspectRepository,
    WorkspaceSetup(setup::Action),
}

struct WakeOnDrop(Context);

impl Drop for WakeOnDrop {
    fn drop(&mut self) {
        self.0.request_repaint();
    }
}

impl RemoteEnvironments {
    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn invalidate_session_views(&mut self) {
        self.setup.invalidate();
        self.repository.invalidate();
        self.reconnect.invalidate();
        self.reopen.invalidate();
    }

    pub(super) fn open(&mut self, home: &HorizonHome, ctx: &Context) {
        if self.open {
            return;
        }
        self.open = true;
        self.invalidate_session_views();
        self.page = None;
        self.page_cursor = None;
        self.selected = None;
        self.failure = None;
        self.observation.invalidate();
        self.stop.invalidate();
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
            self.refresh_when_idle = true;
        } else {
            self.start_load(home, ctx, None);
        }
        ctx.request_repaint();
    }

    fn close(&mut self) {
        self.open = false;
        self.invalidate_session_views();
        self.observation.invalidate();
        self.stop.invalidate();
        self.refresh_after_stop = false;
        self.refresh_when_idle = false;
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
        }
    }

    fn start_load(&mut self, home: &HorizonHome, ctx: &Context, cursor: Option<String>) {
        if !self.open || self.pending.is_some() {
            return;
        }
        self.invalidate_session_views();
        self.observation.invalidate();
        self.stop.cancel_confirmation();
        let (tx, rx) = mpsc::sync_channel(1);
        let home = home.clone();
        let requested_cursor = cursor.clone();
        let wake = WakeOnDrop(ctx.clone());
        let worker = std::thread::Builder::new()
            .name("remote-environment-inventory".into())
            .spawn(move || {
                let _wake = wake;
                let result = load_page(&home, requested_cursor.as_deref());
                let _ = tx.send(result);
            });
        self.failure = None;
        match worker {
            Ok(_) => {
                self.pending = Some(PendingLoad {
                    rx,
                    cursor,
                    discard: false,
                });
            }
            Err(_) => self.accept_result(cursor, Err(LoadError::WorkerUnavailable)),
        }
    }

    fn drain_result(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.pending = Some(pending);
                return;
            }
            Err(TryRecvError::Disconnected) => Err(LoadError::WorkerUnavailable),
        };
        if self.open && !pending.discard {
            self.accept_result(pending.cursor, result);
        }
    }

    fn accept_result(&mut self, cursor: Option<String>, result: Result<InventoryPage, LoadError>) {
        self.invalidate_session_views();
        self.observation.invalidate();
        self.stop.cancel_confirmation();
        match result {
            Ok(page) => {
                let previous = self
                    .page
                    .as_ref()
                    .and_then(|page| self.selected.and_then(|index| page.rows.get(index)));
                self.selected = previous
                    .and_then(|previous| {
                        page.rows
                            .iter()
                            .position(|row| row.summary.workspace_local_id == previous.summary.workspace_local_id)
                    })
                    .or_else(|| (!page.rows.is_empty()).then_some(0));
                self.page = Some(page);
                self.page_cursor = cursor;
                self.failure = None;
            }
            Err(error) => self.failure = Some(LoadFailure { error, cursor }),
        }
    }

    fn apply(&mut self, action: InventoryAction, home: &HorizonHome, ctx: &Context) {
        if !matches!(
            action,
            InventoryAction::None
                | InventoryAction::Select(_)
                | InventoryAction::PrepareRepository
                | InventoryAction::ConfirmRepository
                | InventoryAction::CancelRepository
                | InventoryAction::InspectRepository
        ) {
            self.repository.invalidate();
        }
        if self.open
            && self.pending.is_none()
            && matches!(
                action,
                InventoryAction::Refresh | InventoryAction::First | InventoryAction::Next | InventoryAction::Retry
            )
        {
            self.stop.invalidate();
        }
        match action {
            InventoryAction::None
            | InventoryAction::WorkspaceSetup(_)
            | InventoryAction::PrepareRepository
            | InventoryAction::ConfirmRepository
            | InventoryAction::CancelRepository
            | InventoryAction::InspectRepository
            | InventoryAction::Observe
            | InventoryAction::RequestStop
            | InventoryAction::CheckStop
            | InventoryAction::ConfirmStop
            | InventoryAction::RequestStart
            | InventoryAction::ConfirmStart => {}
            InventoryAction::ListReconnectViews | InventoryAction::Reconnect(_) => self.reopen.invalidate(),
            InventoryAction::ListReopenPanels
            | InventoryAction::ReopenView(_)
            | InventoryAction::InspectTask(_)
            | InventoryAction::PrepareTaskStart(_)
            | InventoryAction::ConfirmTaskStart
            | InventoryAction::CancelTaskStart => {
                self.reconnect.invalidate();
            }
            InventoryAction::CancelStop => self.stop.cancel_confirmation(),
            InventoryAction::Close => self.close(),
            InventoryAction::Select(index) => {
                if self.selected != Some(index) && self.page.as_ref().is_some_and(|page| index < page.rows.len()) {
                    self.invalidate_session_views();
                    self.observation.invalidate();
                    self.stop.invalidate();
                    self.selected = Some(index);
                }
            }
            InventoryAction::Refresh => self.start_load(home, ctx, self.page_cursor.clone()),
            InventoryAction::First => self.start_load(home, ctx, None),
            InventoryAction::Next => {
                if let Some(cursor) = self.page.as_ref().and_then(|page| page.next_cursor.clone()) {
                    self.start_load(home, ctx, Some(cursor));
                }
            }
            InventoryAction::Retry => {
                if let Some(failure) = &self.failure {
                    self.start_load(home, ctx, failure.cursor.clone());
                }
            }
        }
    }

    pub(super) fn invalidate_provider_state(&mut self) {
        self.invalidate_session_views();
        self.observation.invalidate();
        self.stop.invalidate();
    }

    fn start_observation(
        &mut self,
        home: &HorizonHome,
        config: &horizon_core::remote_provider_config::RemoteProviderConfig,
        ctx: &Context,
    ) {
        if !self.open || self.pending.is_some() || self.stop.is_pending() {
            return;
        }
        if let Some(row) = self
            .page
            .as_ref()
            .and_then(|page| self.selected.and_then(|index| page.rows.get(index)))
        {
            self.observation.start(home, config, &row.summary, ctx);
        }
    }

    fn stop_action(
        &mut self,
        action: InventoryAction,
        home: &HorizonHome,
        config: &horizon_core::remote_provider_config::RemoteProviderConfig,
        ctx: &Context,
    ) {
        if !self.open || self.pending.is_some() || self.observation.is_pending() {
            return;
        }
        let Some(summary) = self
            .page
            .as_ref()
            .and_then(|page| self.selected.and_then(|index| page.rows.get(index)))
            .map(|row| row.summary.clone())
        else {
            return;
        };
        match action {
            InventoryAction::RequestStop => {
                self.invalidate_session_views();
                self.stop.prepare(&summary, config, ctx);
            }
            InventoryAction::ConfirmStop | InventoryAction::ConfirmStart
                if self.stop.start(home, config, &summary, ctx) =>
            {
                self.invalidate_session_views();
                self.observation.invalidate();
            }
            InventoryAction::CheckStop if self.stop.check(home, config, &summary, ctx) => {
                self.invalidate_session_views();
                self.observation.invalidate();
            }
            InventoryAction::RequestStart => {
                self.invalidate_session_views();
                self.stop.prepare_start(&summary, config, ctx);
            }
            _ => {}
        }
    }

    fn drain_stop(&mut self, home: &HorizonHome, ctx: &Context) {
        if self.stop.drain_result() {
            self.observation.invalidate();
            self.refresh_after_stop = self.open;
        }
        if self.refresh_after_stop && self.pending.is_none() {
            self.refresh_after_stop = false;
            self.start_load(home, ctx, self.page_cursor.clone());
        }
    }
}

fn load_page(home: &HorizonHome, cursor: Option<&str>) -> Result<InventoryPage, LoadError> {
    let store = CloudWorkflowStore::open_read_only(home).map_err(|_| LoadError::OpenStore)?;
    let page = store
        .list_remote_environment_page(cursor)
        .map_err(|_| LoadError::ReadPage)?;
    Ok(InventoryPage {
        rows: page
            .records
            .iter()
            .map(|record| paint::InventoryRow::new(record.environment_summary()))
            .collect(),
        next_cursor: page.next_cursor,
    })
}

impl HorizonApp {
    /// Render before root input routing, then consume the entire modal frame's input,
    /// including dismissal. Remote inventory completion never requires idle polling.
    pub(super) fn render_remote_environments(&mut self, ctx: &Context) -> Option<egui::InputState> {
        self.remote_environments.drain_result();
        self.remote_environments.observation.drain_result();
        self.remote_environments.drain_stop(self.session_store.home(), ctx);
        if self.remote_environments.refresh_when_idle && self.remote_environments.pending.is_none() {
            self.remote_environments.refresh_when_idle = false;
            self.remote_environments
                .start_load(self.session_store.home(), ctx, None);
        }
        let was_open = self.remote_environments.open;
        if was_open {
            self.handle_speech_input(ctx);
            self.remote_repository_action(InventoryAction::None, ctx);
            self.remote_workspace_setup_action(InventoryAction::None, ctx);
            let mut action = paint::show(ctx, &mut self.remote_environments);
            if self.remote_environments.setup.is_active()
                && !matches!(
                    action,
                    InventoryAction::None | InventoryAction::Close | InventoryAction::WorkspaceSetup(_)
                )
            {
                action = InventoryAction::None;
            }
            if self.remote_environments.repository.is_pending()
                && !matches!(
                    action,
                    InventoryAction::None | InventoryAction::Close | InventoryAction::Select(_)
                )
            {
                action = InventoryAction::None;
            }
            if matches!(action, InventoryAction::Observe) {
                self.remote_environments.start_observation(
                    self.session_store.home(),
                    &self.template_config.remote,
                    ctx,
                );
            }
            if matches!(
                action,
                InventoryAction::RequestStop
                    | InventoryAction::ConfirmStop
                    | InventoryAction::CheckStop
                    | InventoryAction::RequestStart
                    | InventoryAction::ConfirmStart
            ) {
                self.remote_environments.stop_action(
                    action,
                    self.session_store.home(),
                    &self.template_config.remote,
                    ctx,
                );
            }
            self.remote_environments.apply(action, self.session_store.home(), ctx);
            self.remote_reconnect_action(action, ctx);
            self.remote_reopen_action(action, ctx);
            self.remote_repository_action(action, ctx);
            self.remote_workspace_setup_action(action, ctx);
            let modal_input = ctx.input(Clone::clone);
            self.suppress_root_viewport_interaction(ctx);
            return Some(modal_input);
        }
        self.remote_reconnect_action(InventoryAction::None, ctx);
        self.remote_reopen_action(InventoryAction::None, ctx);
        self.remote_repository_action(InventoryAction::None, ctx);
        self.remote_workspace_setup_action(InventoryAction::None, ctx);
        None
    }

    pub(super) fn restore_remote_environment_input(ctx: &Context, saved: egui::InputState) {
        ctx.input_mut(|input| {
            input.pointer = saved.pointer;
            input.keys_down = saved.keys_down;
            input.modifiers = saved.modifiers;
        });
    }
}

#[cfg(test)]
mod tests;
