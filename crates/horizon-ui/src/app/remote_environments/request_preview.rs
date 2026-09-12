//! Transient local request review. This module cannot submit, recover or create a worker.
mod form;
mod paint;

use super::{Context, HorizonHome, InventoryAction, WakeOnDrop};
use horizon_core::{
    remote_provider_config::RemoteProviderConfig,
    remote_workspace_setup::{PreparedRemoteWorkspaceSetup, preview_configured_remote_workspace},
};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Clone, Copy)]
pub(super) enum Action {
    New,
    Review,
    Edit,
    Cancel,
}

#[derive(Default)]
pub(super) struct PreviewState {
    form: Option<form::Form>,
    scope: Option<Scope>,
    review: Option<paint::Review>,
    pending: Option<Pending>,
    notice: Option<String>,
    available: bool,
}
#[derive(Clone, PartialEq)]
struct Scope {
    home: std::path::PathBuf,
    owner: String,
    config: RemoteProviderConfig,
}
type Current<'a> = (&'a HorizonHome, &'a str, &'a RemoteProviderConfig);
impl Scope {
    fn matches(&self, current: Option<Current<'_>>) -> bool {
        current.is_some_and(|(home, owner, config)| {
            self.home == home.root() && self.owner == owner && self.config == *config
        })
    }
}
struct Pending {
    receiver: Receiver<Result<PreparedRemoteWorkspaceSetup, String>>,
    scope: Scope,
    discard: bool,
}

impl PreviewState {
    pub(super) fn is_active(&self) -> bool {
        self.form.is_some() || self.pending.is_some() || self.notice.is_some()
    }
    pub(super) fn invalidate(&mut self) {
        self.form = None;
        self.scope = None;
        self.review = None;
        self.notice = None;
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
        }
    }
    fn sync(&mut self, current: Option<Current<'_>>) {
        if self.scope.as_ref().is_some_and(|scope| !scope.matches(current)) {
            self.invalidate();
        }
        let Some(pending) = self.pending.take() else {
            return;
        };
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.pending = Some(pending);
                return;
            }
            Err(TryRecvError::Disconnected) => Err("Request review could not finish. Nothing has been created.".into()),
        };
        if pending.discard || !pending.scope.matches(current) {
            return;
        }
        match result {
            Ok(prepared) => self.review = Some(paint::Review::new(prepared, &pending.scope.config)),
            Err(error) => self.notice = Some(error),
        }
    }
    fn action(
        &mut self,
        action: Action,
        home: &HorizonHome,
        owner: &str,
        config: &RemoteProviderConfig,
        ctx: &Context,
    ) {
        if matches!(action, Action::Cancel) {
            self.invalidate();
            return;
        }
        if self.pending.is_some() {
            return;
        }
        if matches!(action, Action::New) {
            self.invalidate();
            self.form = Some(form::Form::new(config));
            self.scope = Some(Scope {
                home: home.root().to_path_buf(),
                owner: owner.into(),
                config: config.clone(),
            });
            return;
        }
        let Some(scope) = self
            .scope
            .as_ref()
            .filter(|scope| scope.matches(Some((home, owner, config))))
        else {
            return;
        };
        if matches!(action, Action::Edit) {
            self.review = None;
            self.notice = None;
            return;
        }
        let deadline = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|time| i64::try_from(time.as_millis()).ok())
            .and_then(|time| time.checked_add(30 * 60 * 1000));
        let draft = self
            .form
            .as_ref()
            .zip(deadline)
            .ok_or("Request form or clock is unavailable.")
            .and_then(|(form, deadline)| form.draft(deadline));
        let draft = match draft {
            Ok(draft) => draft,
            Err(error) => {
                self.notice = Some(error.into());
                return;
            }
        };
        let scope = scope.clone();
        let captured = scope.clone();
        let home = home.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let wake = WakeOnDrop(ctx.clone());
        let started = std::thread::Builder::new()
            .name("remote-request-preview".into())
            .spawn(move || {
                let _wake = wake;
                let result = preview_configured_remote_workspace(&home, &captured.config, &captured.owner, draft)
                    .map_err(|error| error.to_string());
                let _ = sender.send(result);
            });
        self.review = None;
        self.notice = None;
        match started {
            Ok(_) => {
                self.pending = Some(Pending {
                    receiver,
                    scope,
                    discard: false,
                });
            }
            Err(_) => self.notice = Some("Request review could not start. Nothing has been created.".into()),
        }
        ctx.request_repaint();
    }
}

impl super::RemoteEnvironments {
    fn request_preview_idle(&self) -> bool {
        self.pending.is_none()
            && !self.observation.is_pending()
            && !self.stop.is_pending()
            && !self.reconnect.is_pending()
            && !self.reopen.is_pending()
            && !self.repository.is_pending()
    }
}
impl super::HorizonApp {
    pub(super) fn remote_workspace_request_action(&mut self, action: InventoryAction, ctx: &Context) {
        let state = &mut self.remote_environments;
        let current = self
            .active_session
            .as_ref()
            .filter(|session| cfg!(target_os = "linux") && state.open && session.persistent)
            .map(|session| {
                (
                    self.session_store.home(),
                    session.session_id.as_str(),
                    &self.template_config.remote,
                )
            });
        state.request_preview.sync(current);
        state.request_preview.available = current.is_some() && state.request_preview_idle();
        let InventoryAction::RequestPreview(action) = action else {
            return;
        };
        let Some((home, owner, config)) = current else {
            return;
        };
        if !state.request_preview_idle() {
            return;
        }
        state.stop.cancel_confirmation();
        state.reconnect.invalidate();
        state.reopen.invalidate();
        state.repository.invalidate();
        state.request_preview.action(action, home, owner, config, ctx);
    }
}

#[cfg(test)]
mod tests;
