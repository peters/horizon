//! One explicit local connection, invalidated before any queued result is adopted.

mod views;

use super::{Context, HorizonHome, InventoryAction, RemoteEnvironmentSummary, WakeOnDrop};
use horizon_core::{
    Board, PanelId, PreparedRemotePanelHandoff,
    cloud_run::CloudWorkflowStore,
    remote_panel_attachment::{ConfiguredRemotePanelAttachRequest, attach_configured_remote_panel},
    remote_provider_config::RemoteProviderConfig,
    remote_ssh_identity::RemoteSshIdentityStore,
};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Default)]
pub(super) struct ReconnectState {
    views: Option<Vec<views::View>>,
    pending: Option<PendingConnection>,
    notice: Option<String>,
    repaint_context: Option<Context>,
}

struct PendingConnection {
    rx: Receiver<Result<PreparedRemotePanelHandoff, String>>,
    expected: RemoteEnvironmentSummary,
    config: RemoteProviderConfig,
    owner: String,
    target: PanelId,
    discard: bool,
}

struct ClientContext<'a> {
    home: &'a HorizonHome,
    config: &'a RemoteProviderConfig,
    selected: Option<&'a RemoteEnvironmentSummary>,
    owner: Option<&'a str>,
}

impl ReconnectState {
    pub(super) fn invalidate(&mut self) {
        let changed = self.views.take().is_some()
            | self.notice.take().is_some()
            | self.pending.as_ref().is_some_and(|pending| !pending.discard);
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
        }
        if changed && let Some(ctx) = &self.repaint_context {
            ctx.request_repaint();
        }
    }

    fn list(&mut self, client: &ClientContext<'_>, board: &Board, ctx: &Context) {
        if self.pending.is_some() {
            return;
        }
        self.repaint_context = Some(ctx.clone());
        match views::list(board, client.selected, client.owner) {
            Ok(views) => {
                self.views = Some(views);
                self.notice = None;
            }
            Err(message) => {
                self.views = None;
                self.notice = Some(message.into());
            }
        }
        ctx.request_repaint();
    }

    fn start(&mut self, client: &ClientContext<'_>, board: &Board, target: PanelId, ctx: &Context) {
        if self.pending.is_some()
            || !self
                .views
                .as_ref()
                .is_some_and(|views| views.iter().any(|view| view.id == target))
        {
            return;
        }
        let (Some(expected), Some(owner)) = (client.selected, client.owner) else {
            self.invalidate();
            return;
        };
        let request = match views::request(board, expected, owner, target, ctx) {
            Ok(request) => request,
            Err(message) => {
                self.notice = Some(message.into());
                ctx.request_repaint();
                return;
            }
        };
        let (tx, rx) = mpsc::sync_channel(1);
        let expected = expected.clone();
        let config = client.config.clone();
        let home = client.home.clone();
        let owner = owner.to_owned();
        let pending = PendingConnection {
            rx,
            expected: expected.clone(),
            config: config.clone(),
            owner: owner.clone(),
            target,
            discard: false,
        };
        let wake = WakeOnDrop(ctx.clone());
        let worker = std::thread::Builder::new()
            .name("remote-panel-reconnection".into())
            .spawn(move || {
                let _wake = wake;
                let result = connect(&home, &config, &expected, &owner, &request);
                let _ = tx.send(result);
            });
        self.notice = None;
        self.repaint_context = Some(ctx.clone());
        match worker {
            Ok(_) => self.pending = Some(pending),
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
        if pending.discard
            || client.owner != Some(pending.owner.as_str())
            || client.selected != Some(&pending.expected)
            || *client.config != pending.config
        {
            tracing::debug!(
                prepared = result.is_ok(),
                "Discarding a stale remote panel reconnection result"
            );
            return None;
        }
        let result = result.and_then(|handoff| {
            board
                .adopt_remote_panel_connection(&pending.owner, pending.target, handoff)
                .map_err(|error| error.to_string())
        });
        let adopted = result.is_ok().then_some(pending.target);
        self.notice = Some(match result {
            Ok(()) => "Local transport opened. Check the terminal; remote readiness is not certified.".into(),
            Err(message) => message,
        });
        adopted
    }
}

fn worker_failure() -> &'static str {
    "Reconnection could not start or finish. You can retry without restarting the remote task."
}

fn connect(
    home: &HorizonHome,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
    owner: &str,
    view: &views::Request,
) -> Result<PreparedRemotePanelHandoff, String> {
    let store = CloudWorkflowStore::open(home)
        .map_err(|_| "The saved environment could not be safely read. Refresh before reconnecting.".to_string())?;
    let identities = RemoteSshIdentityStore::new(home);
    let attempt = attach_configured_remote_panel(
        &store,
        &identities,
        config,
        ConfiguredRemotePanelAttachRequest {
            expected,
            client_session_id: owner,
            panel_id: &view.local_id,
            terminal: view.terminal,
        },
    )
    .map_err(|error| error.to_string())?;
    PreparedRemotePanelHandoff::prepare(&store, attempt).map_err(|error| error.to_string())
}

pub(super) fn show(ui: &mut egui::Ui, state: &ReconnectState, enabled: bool, action: &mut InventoryAction) {
    #[cfg(test)]
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new("reconnect-painted-pending-test"), state.pending.is_some()));
    ui.separator();
    ui.strong("Reconnect session panels");
    ui.label("Open existing views in their owning session. No worker or remote task will be created.");
    if ui
        .add_enabled(
            enabled && state.pending.is_none(),
            egui::Button::new("Show session panels"),
        )
        .clicked()
    {
        *action = InventoryAction::ListReconnectViews;
    }
    if let Some(pending) = &state.pending {
        ui.label(if pending.discard {
            "Waiting for the discarded connection attempt to finish…"
        } else {
            "Connecting to the existing remote task…"
        });
    }
    if let Some(views) = &state.views {
        if views.is_empty() {
            ui.label("No matching saved views in this session. Creating or opening other sessions is separate.");
        }
        for view in views {
            ui.push_id(view.id.0, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&view.label);
                    if ui
                        .add_enabled(enabled && state.pending.is_none(), egui::Button::new("Reconnect"))
                        .clicked()
                    {
                        *action = InventoryAction::Reconnect(view.id);
                    }
                });
            });
        }
    }
    if let Some(notice) = &state.notice {
        ui.label(notice);
    }
}

impl super::HorizonApp {
    pub(super) fn remote_reconnect_action(&mut self, action: InventoryAction, ctx: &Context) {
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
        if state.open && state.pending.is_none() && !state.observation.is_pending() && !state.stop.is_pending() {
            match action {
                InventoryAction::ListReconnectViews => {
                    state.stop.cancel_confirmation();
                    state.reconnect.list(&client, &self.board, ctx);
                }
                InventoryAction::Reconnect(target) => {
                    state.stop.cancel_confirmation();
                    state.reconnect.start(&client, &self.board, target, ctx);
                }
                _ => {}
            }
        }
        if let Some(target) = state.reconnect.drain(&client, &mut self.board, ctx) {
            self.panel_render_caches.terminal_grid_cache.remove(&target);
        }
    }
}

#[cfg(test)]
mod tests;
