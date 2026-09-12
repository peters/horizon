//! Consumed setup confirmation and retained coordinates; never repository or task execution.
mod form;
mod paint;

use super::{Context, HorizonHome, InventoryAction, WakeOnDrop};
use horizon_core::{
    remote_provider_config::RemoteProviderConfig,
    remote_workspace_setup::{self as api, PreparedRemoteWorkspaceSetup, RemoteWorkspaceSetupLocator},
};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Clone, Copy)]
pub(super) enum Action {
    New,
    Review,
    Edit,
    Confirm,
    Cancel,
    CheckSelected,
    CheckAttempt(usize),
}

#[derive(Default)]
pub(super) struct SetupState {
    form: Option<form::Form>,
    scope: Option<Scope>,
    review: Option<paint::Review>,
    consent: bool,
    pending: Option<Pending>,
    attempts: Vec<RemoteWorkspaceSetupLocator>,
    notice: Option<String>,
    unknown: bool,
    available: bool,
    selected: Option<RemoteWorkspaceSetupLocator>,
    selected_home: Option<std::path::PathBuf>,
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
    receiver: Receiver<Result<Completion, String>>,
    scope: Scope,
    creation_locator: Option<RemoteWorkspaceSetupLocator>,
    discard: bool,
    started: std::time::Instant,
    decision_seconds: u64,
}
enum Work {
    Preview(Box<api::RemoteWorkspaceSetupDraft>),
    Submit(Box<PreparedRemoteWorkspaceSetup>, api::RemoteWorkspaceSetupConsent),
    Check(RemoteWorkspaceSetupLocator),
}
enum Completion {
    Preview(Box<PreparedRemoteWorkspaceSetup>),
    Submitted(Box<api::ConfiguredWorkspaceSetupAttempt>),
    Checked(Box<api::ConfiguredWorkspaceSetupObservation>),
}

fn confirmation(prepared: &PreparedRemoteWorkspaceSetup) -> api::RemoteWorkspaceSetupConsent {
    let image = prepared.spec().target.image.clone();
    match (prepared.azure_profile(), prepared.network_volume()) {
        (Some(profile), None) => api::RemoteWorkspaceSetupConsent::Azure {
            image,
            profile: profile.clone(),
        },
        (_, Some(volume)) => api::RemoteWorkspaceSetupConsent::RunPodHps {
            image,
            volume: volume.clone(),
        },
        (None, None) => api::RemoteWorkspaceSetupConsent::LocalDocker { image },
    }
}

impl SetupState {
    pub(super) fn is_active(&self) -> bool {
        self.form.is_some() || self.pending.is_some()
    }
    pub(super) fn has_content(&self) -> bool {
        self.is_active() || !self.attempts.is_empty() || self.notice.is_some() || self.unknown
    }
    pub(super) fn invalidate(&mut self) {
        self.form = None;
        self.scope = None;
        self.review = None;
        self.consent = false;
        self.notice = None;
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
        }
    }
    fn sync(&mut self, current: Option<Current<'_>>) {
        if self.scope.as_ref().is_some_and(|scope| !scope.matches(current)) {
            self.invalidate();
        }
        let Some(pending) = self.pending.take() else { return };
        let creating = pending.creation_locator.is_some();
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.pending = Some(pending);
                return;
            }
            Err(TryRecvError::Disconnected) => Err(if creating {
                "Setup response was lost. Check the original coordinates; do not resubmit."
            } else {
                "Request review or check could not finish. No creation was requested."
            }
            .into()),
        };
        let refused = matches!(&result, Ok(Completion::Submitted(attempt))
            if pending.creation_locator.as_ref() == Some(&attempt.locator)
                && attempt.result.as_ref().is_err_and(|error| matches!(error,
                    api::ConfiguredWorkspaceSetupError::UnsupportedPlatform
                    | api::ConfiguredWorkspaceSetupError::UnsupportedProvider
                    | api::ConfiguredWorkspaceSetupError::InvalidRequest
                    | api::ConfiguredWorkspaceSetupError::InvalidProfile
                    | api::ConfiguredWorkspaceSetupError::ContextChanged
                    | api::ConfiguredWorkspaceSetupError::ConsentMismatch
                    | api::ConfiguredWorkspaceSetupError::SaveConflict
                    | api::ConfiguredWorkspaceSetupError::CredentialUnavailable)));
        if pending.discard || !pending.scope.matches(current) {
            self.unknown |= creating && !refused;
            return;
        }
        match result {
            Ok(Completion::Preview(prepared)) => {
                self.review = Some(paint::Review::new(*prepared, &pending.scope.config));
            }
            Ok(Completion::Submitted(attempt)) => {
                if pending.creation_locator.as_ref() != Some(&attempt.locator) {
                    self.unknown = true;
                    self.notice = Some("Setup response did not match. Retain the original coordinates.".into());
                    return;
                }
                self.unknown |= attempt.result.is_err() && !refused;
                self.notice = Some(attempt.result.map_or_else(|error| error.to_string(), |_| {
                    "Setup returned an allocation. Refresh saved inventory and Check setup; repository and tasks are not prepared.".into()
                }));
            }
            Ok(Completion::Checked(observed)) => {
                self.notice = Some(
                    match *observed {
                        api::ConfiguredWorkspaceSetupObservation::Missing => {
                            "No saved record found. This check did not create one."
                        }
                        api::ConfiguredWorkspaceSetupObservation::SavedOnly(_) => {
                            "Saved, but unallocated. No automatic allocation or repair."
                        }
                        api::ConfiguredWorkspaceSetupObservation::Interrupted(_) => {
                            "Setup interrupted before recovery prerequisites. Identity was not repaired or replaced."
                        }
                        api::ConfiguredWorkspaceSetupObservation::Observed(_) => {
                            "Original setup observed. This is not worker, repository or task readiness."
                        }
                    }
                    .into(),
                );
            }
            Err(error) => {
                self.unknown |= creating;
                self.notice = Some(error);
            }
        }
    }
    fn launch(&mut self, scope: Scope, work: Work, ctx: &Context) {
        let decision_seconds = match &work {
            Work::Submit(prepared, _) if prepared.azure_profile().is_some() => 300,
            _ => 180,
        };
        let creation_locator = match &work {
            Work::Submit(prepared, _) => Some(prepared.locator().clone()),
            _ => None,
        };
        let captured = scope.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let wake = WakeOnDrop(ctx.clone());
        let started = std::thread::Builder::new()
            .name("remote-workspace-setup".into())
            .spawn(move || {
                let _wake = wake;
                let home = HorizonHome::from_root(captured.home.clone());
                let result = match work {
                    Work::Preview(draft) => {
                        api::preview_configured_remote_workspace(&home, &captured.config, &captured.owner, *draft)
                            .map(Box::new)
                            .map(Completion::Preview)
                            .map_err(|error| error.to_string())
                    }
                    Work::Submit(prepared, consent) => Ok(Completion::Submitted(Box::new(
                        api::submit_configured_remote_workspace(
                            &home,
                            &captured.config,
                            &captured.owner,
                            *prepared,
                            consent,
                        ),
                    ))),
                    Work::Check(locator) => {
                        api::check_configured_remote_workspace_setup(&home, &captured.config, &captured.owner, &locator)
                            .map(Box::new)
                            .map(Completion::Checked)
                            .map_err(|error| error.to_string())
                    }
                };
                let _ = sender.send(result);
            });
        self.notice = None;
        match started {
            Ok(_) => {
                self.pending = Some(Pending {
                    receiver,
                    scope,
                    creation_locator,
                    discard: false,
                    started: std::time::Instant::now(),
                    decision_seconds,
                });
            }
            Err(_) => self.notice = Some("Setup operation could not start. No retry was scheduled.".into()),
        }
        ctx.request_repaint();
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
        let scope = Scope {
            home: home.root().to_path_buf(),
            owner: owner.into(),
            config: config.clone(),
        };
        if matches!(action, Action::New) {
            self.invalidate();
            self.form = Some(form::Form::new(config));
            self.scope = Some(scope);
            return;
        }
        let locator = match action {
            Action::CheckSelected => self.selected.clone(),
            Action::CheckAttempt(index) => self.attempts.get(index).cloned(),
            _ => None,
        };
        if matches!(action, Action::CheckSelected | Action::CheckAttempt(_)) {
            if let Some(locator) = locator.filter(|locator| {
                RemoteWorkspaceSetupLocator::new(home, owner, &locator.workspace_local_id)
                    .is_ok_and(|expected| expected == *locator)
            }) {
                self.invalidate();
                self.launch(scope, Work::Check(locator), ctx);
            } else {
                self.notice = Some("Open the original home and owning session to check this setup.".into());
            }
            return;
        }
        if self.scope.as_ref() != Some(&scope) {
            return;
        }
        if matches!(action, Action::Edit) {
            self.review = None;
            self.consent = false;
            self.notice = None;
            return;
        }
        let work = match action {
            Action::Confirm if self.consent => {
                let Some(review) = self.review.take() else { return };
                let prepared = review.prepared;
                let consent = confirmation(&prepared);
                self.attempts.push(prepared.locator().clone());
                self.invalidate();
                Work::Submit(Box::new(prepared), consent)
            }
            Action::Review => {
                self.review = None;
                self.consent = false;
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
                match draft {
                    Ok(draft) => Work::Preview(Box::new(draft)),
                    Err(error) => {
                        self.notice = Some(error.into());
                        return;
                    }
                }
            }
            _ => return,
        };
        self.launch(scope, work, ctx);
    }
}

impl super::RemoteEnvironments {
    fn setup_idle(&self) -> bool {
        self.pending.is_none()
            && !self.observation.is_pending()
            && !self.stop.is_pending()
            && !self.reconnect.is_pending()
            && !self.reopen.is_pending()
            && !self.repository.is_pending()
    }
}
impl super::HorizonApp {
    pub(super) fn remote_workspace_setup_action(&mut self, action: InventoryAction, ctx: &Context) {
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
        state.setup.sync(current);
        state.setup.available = current.is_some() && state.setup_idle();
        let selected = current.and_then(|(home, owner, _)| {
            state
                .page
                .as_ref()
                .and_then(|page| state.selected.and_then(|index| page.rows.get(index)))
                .filter(|row| {
                    row.summary.owning_session_id == owner
                        && matches!(
                            row.summary.provider,
                            horizon_core::cloud_run::CloudProvider::LocalDocker
                                | horizon_core::cloud_run::CloudProvider::RunPod
                                | horizon_core::cloud_run::CloudProvider::Azure
                        )
                })
                .map(|row| (home, owner, row.summary.workspace_local_id.as_str()))
        });
        let same = selected
            .zip(state.setup.selected.as_ref())
            .is_some_and(|((home, owner, id), saved)| {
                saved.owning_session_id == owner
                    && saved.workspace_local_id == id
                    && state.setup.selected_home.as_deref() == Some(home.root())
            });
        if !same {
            state.setup.selected =
                selected.and_then(|(home, owner, id)| RemoteWorkspaceSetupLocator::new(home, owner, id).ok());
            state.setup.selected_home = selected.map(|(home, _, _)| home.root().to_path_buf());
        }
        let InventoryAction::WorkspaceSetup(action) = action else {
            return;
        };
        let Some((home, owner, config)) = current else { return };
        if !state.setup_idle() {
            return;
        }
        state.stop.cancel_confirmation();
        state.reconnect.invalidate();
        state.reopen.invalidate();
        state.repository.invalidate();
        state.setup.action(action, home, owner, config, ctx);
    }
}

#[cfg(test)]
mod tests;
