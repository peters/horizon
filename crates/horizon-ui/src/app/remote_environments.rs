//! Lazy, single-flight saved inventory loading for the Remote Environments overview.

mod observation;
mod paint;
mod reconnect;
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
    CancelStop,
    ListReconnectViews,
    Reconnect(horizon_core::PanelId),
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
        self.reconnect.invalidate();
    }

    pub(super) fn open(&mut self, home: &HorizonHome, ctx: &Context) {
        if self.open {
            return;
        }
        self.open = true;
        self.reconnect.invalidate();
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
        self.reconnect.invalidate();
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
        self.reconnect.invalidate();
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
        self.reconnect.invalidate();
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
            | InventoryAction::Observe
            | InventoryAction::RequestStop
            | InventoryAction::ConfirmStop
            | InventoryAction::ListReconnectViews
            | InventoryAction::Reconnect(_) => {}
            InventoryAction::CancelStop => self.stop.cancel_confirmation(),
            InventoryAction::Close => self.close(),
            InventoryAction::Select(index) => {
                if self.selected != Some(index) && self.page.as_ref().is_some_and(|page| index < page.rows.len()) {
                    self.reconnect.invalidate();
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
        self.reconnect.invalidate();
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
                self.reconnect.invalidate();
                self.stop.prepare(&summary, config, ctx);
            }
            InventoryAction::ConfirmStop if self.stop.start(home, config, &summary, ctx) => {
                self.reconnect.invalidate();
                self.observation.invalidate();
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
    let store = CloudWorkflowStore::open(home).map_err(|_| LoadError::OpenStore)?;
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
            let action = paint::show(ctx, &self.remote_environments);
            if matches!(action, InventoryAction::Observe) {
                self.remote_environments.start_observation(
                    self.session_store.home(),
                    &self.template_config.remote,
                    ctx,
                );
            }
            if matches!(action, InventoryAction::RequestStop | InventoryAction::ConfirmStop) {
                self.remote_environments.stop_action(
                    action,
                    self.session_store.home(),
                    &self.template_config.remote,
                    ctx,
                );
            }
            self.remote_environments.apply(action, self.session_store.home(), ctx);
            self.remote_reconnect_action(action, ctx);
            let modal_input = ctx.input(Clone::clone);
            self.suppress_root_viewport_interaction(ctx);
            return Some(modal_input);
        }
        self.remote_reconnect_action(InventoryAction::None, ctx);
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
